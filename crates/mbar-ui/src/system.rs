//! System integration helpers for the "System" page: starting the daemon, the launch
//! agent, permission probes and the native menu bar state. Blocking functions here run
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

/// PATH given to the daemon started by launchd (which otherwise only has the system
/// directories), so scripts find Homebrew tools like they do from a shell.
const LAUNCH_AGENT_PATH: &str = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `~/Library/LaunchAgents/dev.rubeen.mbar.plist`.
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

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// `ProgramArguments` for the launch agent: the absolute daemon path when known,
/// otherwise a login shell resolving `mbar` from the user's `$PATH`.
pub fn launch_agent_program(daemon: Option<&Path>) -> Vec<String> {
    match daemon {
        Some(p) => vec![p.to_string_lossy().into_owned()],
        None => vec!["/bin/sh".into(), "-lc".into(), "exec mbar".into()],
    }
}

pub fn launch_agent_plist(program: &[String]) -> String {
    let args: String = program
        .iter()
        .map(|a| format!("\t\t<string>{}</string>\n", xml_escape(a)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
{args}	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>{path}</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#,
        label = LAUNCH_AGENT_LABEL,
        path = LAUNCH_AGENT_PATH,
    )
}

pub fn launch_agent_installed() -> bool {
    launch_agent_path().map(|p| p.exists()).unwrap_or(false)
}

/// Writes the launch agent (takes effect at the next login).
pub fn install_launch_agent(bar_name: &str) -> io::Result<PathBuf> {
    let path = launch_agent_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "$HOME is not set"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let daemon = find_daemon_binary(bar_name);
    std::fs::write(
        &path,
        launch_agent_plist(&launch_agent_program(daemon.as_deref())),
    )?;
    Ok(path)
}

pub fn remove_launch_agent() -> io::Result<()> {
    match launch_agent_path() {
        Some(p) => match std::fs::remove_file(&p) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
        None => Ok(()),
    }
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
    fn plist_contents() {
        let p = launch_agent_plist(&launch_agent_program(Some(Path::new(
            "/opt/homebrew/bin/mbar",
        ))));
        assert!(p.contains("<string>dev.rubeen.mbar</string>"));
        assert!(p.contains("\t\t<string>/opt/homebrew/bin/mbar</string>\n\t</array>"));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        let fallback = launch_agent_plist(&launch_agent_program(None));
        assert!(fallback.contains("<string>/bin/sh</string>"));
        assert!(fallback.contains("<string>exec mbar</string>"));
        let esc = launch_agent_plist(&["/a&b/<mbar>".to_string()]);
        assert!(esc.contains("<string>/a&amp;b/&lt;mbar&gt;</string>"));
    }

    #[test]
    fn launch_agent_path_under_home() {
        if let Some(p) = launch_agent_path() {
            assert!(p.ends_with("Library/LaunchAgents/dev.rubeen.mbar.plist"));
        }
    }

    #[test]
    fn permission_classification() {
        assert_eq!(classify_accessibility(&Ok("[]".into())), Permission::Granted);
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
    fn finds_shell() {
        // `sh` exists on every Unix host.
        assert!(find_executable("sh").is_some());
        assert!(find_executable("definitely-not-a-binary-mbar-ui").is_none());
    }
}
