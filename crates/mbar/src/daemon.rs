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
    pub headless: bool,
    /// Held for the daemon's lifetime (fcntl write lock).
    pub lock: File,
}

pub fn run(bar_name: String, opts: DaemonOptions) -> ! {
    let user = std::env::var("USER").unwrap_or_default();
    if user.is_empty() {
        eprint!("{bar_name}: 'env USER' not set! abort..\n");
        std::process::exit(1);
    }

    let socket_path = mbar_ipc::socket_path(&bar_name);
    let lock_path = lock_path(&socket_path, &user, &bar_name);
    let lock = match acquire_lock(&lock_path) {
        Ok(f) => f,
        Err(LockError::Create) => {
            eprint!("{bar_name}: could not create lock-file! abort..\n");
            std::process::exit(1);
        }
        Err(LockError::Busy) => {
            eprint!("{bar_name}: could not acquire lock-file... already running?\n");
            std::process::exit(1);
        }
    };

    let listener = match Listener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            log::error!("ipc: cannot bind {}: {e}", socket_path.display());
            eprint!("{bar_name}: could not initialize daemon! abort..\n");
            std::process::exit(1);
        }
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_default();
    let config_path = opts
        .config
        .clone()
        .or_else(|| find_config(&bar_name, &xdg, &home));
    let watch_dir = config_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf);

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
    };
    crate::platform::platform_main(setup)
}

/// The lock file lives next to the socket (`$TMPDIR/mbar_<user>_<bar>.lock`, falling back
/// to `/tmp`), so independent `TMPDIR`s run independent daemons (tests).
fn lock_path(socket: &Path, user: &str, bar_name: &str) -> PathBuf {
    socket
        .parent()
        .unwrap_or(Path::new("/tmp"))
        .join(format!("mbar_{user}_{bar_name}.lock"))
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

/// Config lookup (`docs/ARCHITECTURE.md`, `docs/LUA.md`, `cli.md` §10.1): the first
/// existing regular file of
///
/// 1. `$XDG_CONFIG_HOME/<bar>/{init.lua,mbarrc,sketchybarrc}` (XDG set and non-empty),
/// 2. `$HOME/.config/<bar>/{init.lua,mbarrc,sketchybarrc}`,
/// 3. for the default bar name, the SketchyBar directories
///    `$XDG_CONFIG_HOME/sketchybar/…` and `$HOME/.config/sketchybar/…`
///    (`init.lua`, then `sketchybarrc`),
/// 4. `$HOME/.sketchybarrc`.
///
/// `init.lua` wins over the shell config in the same directory. An empty `HOME` skips
/// the `HOME` entries.
pub fn find_config(bar_name: &str, xdg: &str, home: &str) -> Option<PathBuf> {
    config_candidates(bar_name, xdg, home)
        .into_iter()
        .find(|p| p.is_file())
}

fn config_candidates(bar_name: &str, xdg: &str, home: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<(PathBuf, &[&str])> = Vec::new();
    const MBAR: &[&str] = &["init.lua", "mbarrc", "sketchybarrc"];
    const SKETCHYBAR: &[&str] = &["init.lua", "sketchybarrc"];
    if !xdg.is_empty() {
        dirs.push((Path::new(xdg).join(bar_name), MBAR));
    }
    if !home.is_empty() {
        dirs.push((Path::new(home).join(".config").join(bar_name), MBAR));
    }
    if bar_name == mbar_ipc::DEFAULT_BAR_NAME {
        if !xdg.is_empty() {
            dirs.push((Path::new(xdg).join("sketchybar"), SKETCHYBAR));
        }
        if !home.is_empty() {
            dirs.push((Path::new(home).join(".config/sketchybar"), SKETCHYBAR));
        }
    }
    let mut out: Vec<PathBuf> = dirs
        .into_iter()
        .flat_map(|(d, names)| names.iter().map(move |n| d.join(n)))
        .collect();
    if !home.is_empty() {
        out.push(Path::new(home).join(".sketchybarrc"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_order() {
        let c = config_candidates("mbar", "/x", "/h");
        let c: Vec<_> = c.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(
            c,
            [
                "/x/mbar/init.lua",
                "/x/mbar/mbarrc",
                "/x/mbar/sketchybarrc",
                "/h/.config/mbar/init.lua",
                "/h/.config/mbar/mbarrc",
                "/h/.config/mbar/sketchybarrc",
                "/x/sketchybar/init.lua",
                "/x/sketchybar/sketchybarrc",
                "/h/.config/sketchybar/init.lua",
                "/h/.config/sketchybar/sketchybarrc",
                "/h/.sketchybarrc",
            ]
        );
        let c = config_candidates("bottom", "", "");
        assert!(c.is_empty());
        let c = config_candidates("bottom", "", "/h");
        assert_eq!(c.len(), 4);
        assert_eq!(c[0], Path::new("/h/.config/bottom/init.lua"));
    }

    #[test]
    fn lookup_prefers_init_lua() {
        let home = std::env::temp_dir().join(format!("mbar-cfg-{}", std::process::id()));
        let dir = home.join(".config/mbar");
        std::fs::create_dir_all(&dir).unwrap();
        let sb = home.join(".config/sketchybar");
        std::fs::create_dir_all(&sb).unwrap();
        std::fs::write(sb.join("sketchybarrc"), "").unwrap();
        let h = home.to_str().unwrap();
        assert_eq!(find_config("mbar", "", h), Some(sb.join("sketchybarrc")));
        std::fs::write(dir.join("mbarrc"), "").unwrap();
        assert_eq!(find_config("mbar", "", h), Some(dir.join("mbarrc")));
        std::fs::write(dir.join("init.lua"), "").unwrap();
        assert_eq!(find_config("mbar", "", h), Some(dir.join("init.lua")));
        // A directory named like a config is skipped.
        std::fs::remove_file(dir.join("init.lua")).unwrap();
        std::fs::create_dir(dir.join("init.lua")).unwrap();
        assert_eq!(find_config("mbar", "", h), Some(dir.join("mbarrc")));
        std::fs::remove_dir_all(&home).unwrap();
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
