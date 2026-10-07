//! System integration helpers for the "System" page: starting and kickstarting the
//! daemon, permission probes and the native menu bar state. Blocking functions here run
//! on the background executor.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::ipc::{Client, IpcError};

pub const LAUNCH_AGENT_LABEL: &str = "dev.rubeen.mbar";

pub const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const SCREEN_RECORDING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `~/Library/LaunchAgents/dev.rubeen.mbar.plist`, the legacy agent written by
/// `make install-agent` (onboarding boots it out and removes it).
pub fn launch_agent_path() -> Option<PathBuf> {
    home_dir().map(|h| {
        h.join("Library/LaunchAgents")
            .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
    })
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Locates an executable called `name`: next to this binary first (both are built into
/// the same `target/` or installed side by side), then `$PATH`, then the usual install
/// directories (a GUI app launched from Finder has a minimal `$PATH`).
pub fn find_executable(name: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join(name)));
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        candidates.push(Path::new(dir).join(name));
    }
    if let Some(h) = home_dir() {
        candidates.push(h.join(".cargo/bin").join(name));
        candidates.push(h.join(".local/bin").join(name));
    }
    candidates.into_iter().find(|p| is_executable(p))
}

/// The daemon binary for `bar_name`: a binary (or symlink) of that name, since the
/// daemon derives its bar name from `argv[0]`; `mbar` for the default bar.
pub fn find_daemon_binary(bar_name: &str) -> Option<PathBuf> {
    if bar_name != mbar_ipc::DEFAULT_BAR_NAME {
        if let Some(p) = find_executable(bar_name) {
            return Some(p);
        }
    }
    find_executable("mbar")
}

/// Starts the daemon detached from this process (own process group, no stdio).
pub fn start_daemon(bar_name: &str) -> io::Result<PathBuf> {
    use std::os::unix::process::CommandExt;
    let bin = find_daemon_binary(bar_name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not find the `mbar` binary (looked next to mbar-ui, in $PATH, /opt/homebrew/bin, /usr/local/bin, ~/.cargo/bin)",
        )
    })?;
    let mut child = Command::new(&bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    // Reap it when it exits so it never lingers as a zombie while the UI runs.
    std::thread::Builder::new()
        .name("mbar-ui-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(bin)
}

/// Restarts the daemon through launchd (the bundled login item `dev.rubeen.mbar`), e.g.
/// after a permission grant that only takes effect in a new process.
pub fn kickstart_daemon() -> io::Result<()> {
    let uid = current_uid();
    let status = Command::new("/bin/launchctl")
        .args([
            "kickstart",
            "-k",
            &format!("gui/{uid}/{LAUNCH_AGENT_LABEL}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("launchctl exited with {status}")))
    }
}

/// The daemon version from `--query stats` output (`version` field, added together with
/// app distribution). `None` for unparsable output or an older daemon without the field.
pub fn daemon_version_from_stats(json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json).ok()?["version"]
        .as_str()
        .map(str::to_string)
}

/// Whether the running daemon differs from the installed bundle's version. A daemon
/// without a version (`None`) predates the field and is therefore older than the bundle.
pub fn needs_daemon_restart(running: Option<&str>, bundle: &str) -> bool {
    running != Some(bundle)
}

/// The `mbar.app` this binary runs from (`…/mbar.app/Contents/MacOS/mbar-ui`), with its
/// `Info.plist` values; `None` outside a bundle (e.g. a `cargo run` or Linux build).
/// Canonicalized like the daemon's lookup: `current_exe` on macOS is the path that was
/// exec'd, so a start via a symlink would otherwise not be recognised as the bundle.
pub fn current_bundle() -> Option<mbar_app::bundle::AppBundle> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let root = mbar_app::bundle::bundle_root_from_exe(&exe)?;
    mbar_app::bundle::read_bundle(&root)
}

/// Restarts the daemon through launchd when it answers but reports a version other than
/// the installed bundle's (e.g. after a Sparkle update while the daemon kept running).
/// A stopped daemon is left alone: onboarding and the System page handle that. Blocking;
/// run it on the background executor. Returns whether a restart was requested.
///
/// Only for the default bar: the launchd job `dev.rubeen.mbar` runs that one, so a
/// mismatch on another bar (`mbar-ui --bar-name x`) must not restart the default daemon.
pub fn sync_daemon_version(client: &Client, bundle_version: &str) -> bool {
    if client.bar_name() != mbar_ipc::DEFAULT_BAR_NAME {
        return false;
    }
    let Ok(stats) = client.send_strs(&["--query", "stats"]) else {
        return false;
    };
    let running = daemon_version_from_stats(&stats);
    needs_daemon_restart(running.as_deref(), bundle_version) && kickstart_daemon().is_ok()
}

/// The real user id of this process (the launchd `gui/<uid>` domain).
pub fn current_uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}

/// Opens a URL (e.g. a System Settings pane) with `open` (macOS) / `xdg-open`.
pub fn open_url(url: &str) -> io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Missing,
    /// Cannot be determined (daemon not running, query unsupported, ...).
    Unknown(String),
}

/// The permissions belong to the daemon process, so they are probed through it:
/// `--query menus` needs Accessibility, `--query default_menu_items` needs Screen
/// Recording (it answers with a fixed error message without it).
pub fn classify_accessibility(rsp: &Result<String, IpcError>) -> Permission {
    match rsp {
        Ok(_) => Permission::Granted,
        Err(IpcError::Daemon(m)) if m.to_lowercase().contains("accessibility") => {
            Permission::Missing
        }
        Err(e) => Permission::Unknown(e.to_string()),
    }
}

pub fn classify_screen_recording(rsp: &Result<String, IpcError>) -> Permission {
    match rsp {
        Ok(_) => Permission::Granted,
        Err(IpcError::Daemon(m)) if m.to_lowercase().contains("screen recording") => {
            Permission::Missing
        }
        Err(e) => Permission::Unknown(e.to_string()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub accessibility: Permission,
    pub screen_recording: Permission,
}

pub fn probe_permissions(client: &Client) -> Permissions {
    Permissions {
        accessibility: classify_accessibility(&client.send_strs(&["--query", "menus"])),
        screen_recording: classify_screen_recording(
            &client.send_strs(&["--query", "default_menu_items"]),
        ),
    }
}

/// Parses `defaults read` boolean output (`1`/`0`, `true`/`false`, `YES`/`NO`).
pub fn parse_defaults_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// macOS "Automatically hide and show the menu bar" (`_HIHideMenuBar`). `None` when it
/// cannot be read (not macOS, never set reads as `false`).
pub fn menubar_autohide() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("defaults")
        .args(["read", "NSGlobalDomain", "_HIHideMenuBar"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        // The key does not exist until the setting was changed once.
        return Some(false);
    }
    parse_defaults_bool(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_agent_path_under_home() {
        if let Some(p) = launch_agent_path() {
            assert!(p.ends_with("Library/LaunchAgents/dev.rubeen.mbar.plist"));
        }
    }

    #[test]
    fn permission_classification() {
        assert_eq!(
            classify_accessibility(&Ok("[]".into())),
            Permission::Granted
        );
        assert_eq!(
            classify_accessibility(&Err(IpcError::Daemon(
                "Query (menus): Accessibility permission not given".into()
            ))),
            Permission::Missing
        );
        assert!(matches!(
            classify_accessibility(&Err(IpcError::NotRunning)),
            Permission::Unknown(_)
        ));
        assert_eq!(
            classify_screen_recording(&Err(IpcError::Daemon(
                "Query (default_menu_items): Screen Recording Permissions not given. Restart SketchyBar after granting permissions.".into()
            ))),
            Permission::Missing
        );
        assert_eq!(
            classify_screen_recording(&Ok(String::new())),
            Permission::Granted
        );
    }

    #[test]
    fn defaults_bool() {
        assert_eq!(parse_defaults_bool("1\n"), Some(true));
        assert_eq!(parse_defaults_bool("0"), Some(false));
        assert_eq!(parse_defaults_bool("YES"), Some(true));
        assert_eq!(parse_defaults_bool("maybe"), None);
    }

    #[test]
    fn uid_matches_id_command() {
        let out = Command::new("id").arg("-u").output().unwrap();
        let id: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert_eq!(current_uid(), id);
    }

    #[test]
    fn daemon_version_parsing() {
        assert_eq!(
            daemon_version_from_stats("{\n\t\"items\": 3,\n\t\"version\": \"0.2.0\"\n}\n")
                .as_deref(),
            Some("0.2.0")
        );
        assert_eq!(daemon_version_from_stats("{\"items\": 3}"), None);
        assert!(needs_daemon_restart(Some("0.1.0"), "0.2.0"));
        assert!(!needs_daemon_restart(Some("0.2.0"), "0.2.0"));
        // An old daemon without the field is older than any bundle with this feature.
        assert!(needs_daemon_restart(None, "0.2.0"));
    }

    #[test]
    fn version_sync_ignores_other_bars() {
        // A non-default bar is not run by `dev.rubeen.mbar`: it is never queried (and so
        // never triggers a kickstart of the default bar's daemon).
        let bar_name = format!("mbar-ui-test-sync-{}", std::process::id());
        let path = mbar_ipc::socket_path(&bar_name);
        let _ = std::fs::remove_file(&path);
        let server = mbar_ipc::socket::Server::bind(&path).unwrap();
        let queried = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let q = queried.clone();
        server
            .spawn(move |req| {
                q.store(true, std::sync::atomic::Ordering::SeqCst);
                req.respond("{\"version\": \"0.0.1\"}\n");
            })
            .unwrap();
        assert!(!sync_daemon_version(
            &Client::new(bar_name.clone()),
            "9.9.9"
        ));
        assert!(!queried.load(std::sync::atomic::Ordering::SeqCst));
        // Control: the fake daemon answers, so the guard (not a dead socket) skipped it.
        assert!(Client::new(bar_name)
            .send_strs(&["--query", "stats"])
            .is_ok());
        assert!(queried.load(std::sync::atomic::Ordering::SeqCst));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn finds_shell() {
        // `sh` exists on every Unix host.
        assert!(find_executable("sh").is_some());
        assert!(find_executable("definitely-not-a-binary-mbar-ui").is_none());
    }
}
