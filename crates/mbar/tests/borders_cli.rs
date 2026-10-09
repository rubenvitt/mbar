//! The `borders` argv0 mode (borders design §2, `docs/spec/borders.md` §2.1) without a
//! daemon, run against the real binary through a `borders -> mbar` symlink: version and
//! help output, JankyBorders' invalid-argument lines on stdout, and the "not running"
//! exits (never a daemon start). The cases with a running daemon are in `daemon.rs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");

const NOT_RUNNING: &str =
    "borders: mbar is not running. mbar draws the window borders; start mbar.app.\n";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        // `/tmp`, not `temp_dir()`: macOS' `/var/folders/…` would push the daemon's
        // socket path past the ~104-byte `sun_path` limit (as in `daemon.rs`).
        let root = PathBuf::from("/tmp").join(format!(
            "mbcli-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["home", "tmp", "bin"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::os::unix::fs::symlink(EXE, root.join("bin/borders")).unwrap();
        Sandbox { root }
    }

    fn borders(&self) -> PathBuf {
        self.root.join("bin/borders")
    }

    /// `borders <args>`; `timeout_ms` sets `MBAR_BORDERS_CONNECT_TIMEOUT_MS`.
    fn run(&self, args: &[&str], timeout_ms: Option<u64>) -> Output {
        let mut c = Command::new(self.borders());
        c.args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("MBAR_LOCK_DIR", self.root.join("tmp"))
            .env("USER", "mbarborderscli")
            .env("MBAR_ALLOW_ROOT", "1");
        if let Some(ms) = timeout_ms {
            c.env("MBAR_BORDERS_CONNECT_TIMEOUT_MS", ms.to_string());
        }
        c.output().unwrap()
    }

    /// Nothing in the sandbox's `TMPDIR` / lock directory: no daemon was started.
    fn assert_no_daemon(&self) {
        let entries: Vec<_> = std::fs::read_dir(self.root.join("tmp"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(entries.is_empty(), "daemon state created: {entries:?}");
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// BR-CLI-01: `-v`/`--version` as `argv[1]` print JankyBorders' version.
#[test]
fn version() {
    let sb = Sandbox::new();
    for flag in ["-v", "--version"] {
        let out = sb.run(&[flag, "width=5.0"], None);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(stdout(&out), "borders-v1.9.0\n");
        assert!(out.stderr.is_empty(), "{}", stderr(&out));
    }
    sb.assert_no_daemon();
}

/// BR-CLI-02 plus the pointer to mbar's migration guide.
#[test]
fn help() {
    let sb = Sandbox::new();
    for flag in ["-h", "--help"] {
        let out = sb.run(&[flag], None);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(
            stdout(&out),
            "Refer to the man page for help: man borders\n\
             borders is provided by mbar: see docs/MIGRATING.md\n"
        );
        assert!(out.stderr.is_empty(), "{}", stderr(&out));
    }
}

/// Without any argument `borders` never becomes a daemon: it reports that mbar is not
/// running, at once (no retry: there is nothing to send).
#[test]
fn no_arguments_without_daemon() {
    let sb = Sandbox::new();
    let start = Instant::now();
    let out = sb.run(&[], None);
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "{}", stdout(&out));
    assert_eq!(stderr(&out), NOT_RUNNING);
    sb.assert_no_daemon();
}

/// Only invalid arguments: JankyBorders' lines on stdout, then the no-valid-arguments
/// path (BR-CLI-03).
#[test]
fn invalid_arguments_only() {
    let sb = Sandbox::new();
    let out = sb.run(&["bogus", "-v", "active_color=blue", ""], None);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stdout(&out),
        "[?] Borders: Invalid argument 'bogus'\n\
         [?] Borders: Invalid argument '-v'\n\
         [?] Borders: Invalid color argument color=blue\n\
         [?] Borders: Invalid argument ''\n"
    );
    assert_eq!(stderr(&out), NOT_RUNNING);
    sb.assert_no_daemon();
}

/// Valid arguments and no daemon: the client keeps trying for the connect timeout
/// (`MBAR_BORDERS_CONNECT_TIMEOUT_MS`, 5 s by default), then gives up with exit 1.
/// The invalid arguments' lines are printed first.
#[test]
fn valid_arguments_without_daemon_retry_then_fail() {
    let sb = Sandbox::new();
    let start = Instant::now();
    let out = sb.run(&["width=5.0", "bogus"], Some(600));
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(600), "gave up after {took:?}");
    assert!(took < Duration::from_secs(4), "took {took:?}");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "[?] Borders: Invalid argument 'bogus'\n");
    assert_eq!(stderr(&out), NOT_RUNNING);
    sb.assert_no_daemon();
}

/// The client waits for a daemon that comes up within the connect timeout (window
/// managers start `borders` before the login item started mbar).
#[test]
fn valid_arguments_wait_for_the_daemon() {
    let sb = Sandbox::new();
    let mut client = Command::new(sb.borders());
    client
        .arg("width=5.0")
        .env_clear()
        .env("HOME", sb.root.join("home"))
        .env("TMPDIR", sb.root.join("tmp"))
        .env("MBAR_LOCK_DIR", sb.root.join("tmp"))
        .env("USER", "mbarborderscli")
        .env("MBAR_ALLOW_ROOT", "1")
        .env("MBAR_BORDERS_CONNECT_TIMEOUT_MS", "15000")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let client = client.spawn().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let mut daemon = Command::new(Path::new(EXE))
        .arg("--headless")
        .env_clear()
        .env("HOME", sb.root.join("home"))
        .env("TMPDIR", sb.root.join("tmp"))
        .env("MBAR_LOCK_DIR", sb.root.join("tmp"))
        .env("USER", "mbarborderscli")
        .env("MBAR_ALLOW_ROOT", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let out = client.wait_with_output().unwrap();
    let _ = daemon.kill();
    let _ = daemon.wait();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(out.stdout.is_empty(), "{}", stdout(&out));
}
