//! Regression test for F10: the single-instance lock file must not depend on `$TMPDIR`.
//!
//! `cli.md` §1.1 puts the lock at a fixed `/tmp/<g_name>_<USER>.lock`. mbar used to put
//! it next to the socket, inside `$TMPDIR`, so a daemon started without `TMPDIR` (service
//! manager) and one started from a shell with `TMPDIR` set each took their own lock and
//! both ran. The lock now lives in the private `/tmp/mbar-<uid>/` directory whatever
//! `$TMPDIR` says (the `MBAR_LOCK_DIR` test hook is deliberately left unset here).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");
const TIMEOUT: Duration = Duration::from_secs(15);

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    user: String,
}

impl Sandbox {
    fn new() -> Sandbox {
        let pid = std::process::id();
        let root = PathBuf::from("/tmp").join(format!("mblock-{pid}"));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        for t in ["tmp-a", "tmp-b"] {
            let d = root.join(t);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        // A user name of its own keeps the real `/tmp/mbar-<uid>` lock file apart from
        // any other daemon of this uid.
        Sandbox {
            root,
            home,
            user: format!("mbarlock{pid}"),
        }
    }

    fn tmp(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn lock_file(&self) -> PathBuf {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        PathBuf::from("/tmp")
            .join(format!("mbar-{uid}"))
            .join(format!("mbar_{}_mbar.lock", self.user))
    }

    /// `tmpdir = None` starts the daemon without `TMPDIR` in its environment.
    fn command(&self, tmpdir: Option<&Path>) -> Command {
        let mut c = Command::new(EXE);
        c.env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("USER", &self.user)
            .env("MBAR_ALLOW_ROOT", "1")
            .arg("--headless")
            .stdin(Stdio::null());
        if let Some(t) = tmpdir {
            c.env("TMPDIR", t);
        }
        c
    }

    fn socket(&self, tmpdir: &Path) -> PathBuf {
        tmpdir.join(format!("mbar_{}_mbar.socket", self.user))
    }

    fn start_daemon(&self, tmpdir: &Path) -> Daemon {
        let mut d = Daemon(
            self.command(Some(tmpdir))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let sock = self.socket(tmpdir);
        let start = Instant::now();
        loop {
            assert!(d.0.try_wait().unwrap().is_none(), "daemon exited early");
            let rsp = mbar_ipc::socket::send(&sock, &mbar_ipc::encode_args(&["--query", "bar"]));
            if rsp.is_ok_and(|r| r.starts_with('{')) {
                return d;
            }
            assert!(start.elapsed() < TIMEOUT, "daemon did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Runs a second daemon to completion (it must refuse to start).
    fn second_daemon(&self, tmpdir: Option<&Path>) -> Output {
        let mut child = self
            .command(tmpdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                panic!("second daemon kept running next to the first one");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.wait_with_output().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_file(self.lock_file());
        // Only reached when a second daemon wrongly bound the `/tmp/mbar-<uid>` socket.
        if let Some(dir) = self.lock_file().parent() {
            let _ = std::fs::remove_file(dir.join(format!("mbar_{}_mbar.socket", self.user)));
        }
    }
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn assert_refused(out: &Output, what: &str) {
    assert_eq!(out.status.code(), Some(1), "{what}: exit status");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "mbar: could not acquire lock-file... already running?\n",
        "{what}: stderr"
    );
}

#[test]
fn daemons_with_different_tmpdirs_share_one_lock() {
    let sb = Sandbox::new();
    let (a, b) = (sb.tmp("tmp-a"), sb.tmp("tmp-b"));
    let _first = sb.start_daemon(&a);

    // The lock is in the TMPDIR-independent per-user directory, not in `$TMPDIR`.
    let lock = sb.lock_file();
    assert!(lock.exists(), "lock file at {}", lock.display());
    let lock_dir = lock.parent().unwrap();
    assert_eq!(
        std::fs::symlink_metadata(lock_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(!a.join(format!("mbar_{}_mbar.lock", sb.user)).exists());

    // Another TMPDIR (a shell with pam_tmpdir) ...
    assert_refused(&sb.second_daemon(Some(&b)), "different TMPDIR");
    assert!(!sb.socket(&b).exists());
    // ... and no TMPDIR at all (a service manager) are both refused.
    assert_refused(&sb.second_daemon(None), "TMPDIR unset");
}
