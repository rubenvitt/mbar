//! Daemon startup (`cli.md` §1.1 step 5, §10.1): `USER` check, lock file, config
//! lookup, IPC socket, then the platform's main loop.

use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::driver::DriverConfig;
use crate::ipc::Listener;
use crate::DaemonOptions;

/// Everything a platform needs to run the daemon.
pub struct DaemonSetup {
    pub driver: DriverConfig,
    pub listener: Listener,
    pub socket_path: PathBuf,
    /// Directory watched by the hotloader (the config directory at startup).
    pub watch_dir: Option<PathBuf>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub headless: bool,
    /// Held for the daemon's lifetime (fcntl write lock).
    pub lock: File,
    /// Termination signal handling, installed before the platform starts; the platform
    /// forwards it into its main loop as `Event::Terminate`.
    pub signals: Option<crate::signals::Signals>,
}

pub fn run(bar_name: String, opts: DaemonOptions) -> ! {
    let user = std::env::var("USER").unwrap_or_default();
    if user.is_empty() {
        eprintln!("{bar_name}: 'env USER' not set! abort..");
        std::process::exit(1);
    }

    // `cli.md` §1.1: the lock file has a fixed per-user path that does not depend on
    // `$TMPDIR` (a private `mbar-<uid>` directory inside `/tmp`, see
    // `mbar_ipc::lock_dir`), so daemons started with different `$TMPDIR`s (service
    // manager vs. shell) still exclude each other.
    let lock_path = match mbar_ipc::prepare_lock_path(&user, &bar_name) {
        Ok(p) => p,
        Err(e) => {
            log::error!("lock directory: {e}");
            eprintln!("{bar_name}: could not create lock-file! abort..");
            std::process::exit(1);
        }
    };
    let lock = match acquire_lock(&lock_path) {
        Ok(f) => f,
        Err(LockError::Create) => {
            eprintln!("{bar_name}: could not create lock-file! abort..");
            std::process::exit(1);
        }
        Err(LockError::Busy) => {
            eprintln!("{bar_name}: could not acquire lock-file... already running?");
            std::process::exit(1);
        }
    };

    // The socket directory must be private to this user (a per-user `mbar-<uid>`
    // directory inside a shared `$TMPDIR` / `/tmp`).
    let socket_path = match mbar_ipc::prepare_socket_path(&bar_name) {
        Ok(p) => p,
        Err(e) => {
            log::error!("ipc: socket directory: {e}");
            eprintln!("{bar_name}: could not create lock-file! abort..");
            std::process::exit(1);
        }
    };
    let listener = match Listener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            log::error!("ipc: cannot bind {}: {e}", socket_path.display());
            eprintln!("{bar_name}: could not initialize daemon! abort..");
            std::process::exit(1);
        }
    };
    // From here on SIGTERM/SIGINT/SIGHUP shut the daemon down cleanly (socket removed,
    // script process groups terminated, menu-bar setting restored).
    let signals = match crate::signals::install() {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("cannot install signal handlers: {e}");
            None
        }
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_default();
    let config_path = opts
        .config
        .clone()
        .or_else(|| mbar_app::config::find_config(&bar_name, &xdg, &home));
    let watch_dir = config_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf);

    // `cli.md` §10.2 step 2: `CONFIG_DIR` lives in the daemon's own environment (Lua's
    // `os.getenv`, `io.popen`, `os.execute` see it). Set here, before any thread exists;
    // `Driver::run_config` only touches it again when a reload changes the directory.
    if let Some(dir) = config_path
        .as_deref()
        .filter(|p| p.is_file())
        .and_then(Path::parent)
    {
        std::env::set_var("CONFIG_DIR", dir);
    }

    // Running from mbar.app: scripts (and Lua's io.popen) find `sketchybar`/`mbar` in the
    // bundle first, whatever PATH launchd gave the agent. Set before any thread exists.
    // Canonicalized: on macOS `current_exe` is the path that was exec'd, so a start via
    // `Resources/bin/sketchybar` (or a symlink elsewhere) would otherwise not be found.
    let bundle = std::env::current_exe()
        .ok()
        .map(|exe| std::fs::canonicalize(&exe).unwrap_or(exe))
        .and_then(|exe| mbar_app::bundle::bundle_root_from_exe(&exe))
        .and_then(|root| mbar_app::bundle::read_bundle(&root));
    if let Some(b) = &bundle {
        let current = std::env::var("PATH").unwrap_or_default();
        std::env::set_var(
            "PATH",
            mbar_app::bundle::script_path(&b.bin_dir(), &current),
        );
    }

    // The bundle's agent plist cannot name a per-user log path; redirect stdout/stderr
    // to ~/Library/Logs/mbar.log like `make install-agent` does. A start from a terminal
    // keeps its output.
    #[cfg(target_os = "macos")]
    if bundle.is_some()
        && std::env::var_os("MBAR_NO_LOG_REDIRECT").is_none()
        && unsafe { libc::isatty(2) } == 0
    {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let path = std::path::Path::new(&home).join("Library/Logs/mbar.log");
            if let Ok(f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                use std::os::unix::io::AsRawFd;
                unsafe {
                    libc::dup2(f.as_raw_fd(), 1);
                    libc::dup2(f.as_raw_fd(), 2);
                }
            }
        }
    }

    // `BAR_NAME` was set in `main` before any thread existed.
    let base_env: Vec<(OsString, OsString)> = std::env::vars_os().collect();

    let setup = DaemonSetup {
        driver: DriverConfig {
            bar_name,
            home,
            config_path,
            base_env,
        },
        listener,
        socket_path,
        watch_dir,
        headless: opts.headless,
        lock,
        signals,
    };
    // Runs only where the platform has an update runner (`updater::spawn`).
    if let Some(b) = bundle {
        crate::updater::spawn(b, &setup.driver.home);
    }
    crate::platform::platform_main(setup)
}

#[derive(Debug, PartialEq, Eq)]
enum LockError {
    Create,
    Busy,
}

/// `open(O_CREAT|O_WRONLY, 0600)` + `fcntl(F_SETLK, F_WRLCK)` over the whole file.
fn acquire_lock(path: &Path) -> Result<File, LockError> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| LockError::Create)?;
    // SAFETY: `flock` is plain data; fcntl on a valid fd with a valid pointer.
    let rc = unsafe {
        let mut fl: libc::flock = std::mem::zeroed();
        fl.l_type = libc::F_WRLCK as _;
        fl.l_whence = libc::SEEK_SET as _;
        fl.l_start = 0;
        fl.l_len = 0;
        libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &fl)
    };
    if rc == -1 {
        return Err(LockError::Busy);
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_config_lookup_uses_the_ipc_default_bar() {
        let c = mbar_app::config::config_candidates(mbar_ipc::DEFAULT_BAR_NAME, "", "/h");
        assert!(c.iter().any(|p| p.ends_with(".config/sketchybar/init.lua")));
    }

    #[test]
    fn lock_errors() {
        // fcntl locks are per process; exclusivity between daemons is covered by the
        // integration tests (second daemon refuses to start).
        let p = std::env::temp_dir().join(format!("mbar-lock-{}.lock", std::process::id()));
        let _a = acquire_lock(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(
            acquire_lock(Path::new("/nonexistent-dir/x.lock")).unwrap_err(),
            LockError::Create
        );
    }
}
