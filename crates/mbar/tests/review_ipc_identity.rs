//! Regression tests for the daemon's name and socket identity:
//!
//! * CLI-6: a daemon started as `sketchybar` exports `BAR_NAME=sketchybar`
//!   (`cli.md` §1.1 step 3, `examples.md` §0) while still serving the `mbar` socket.
//! * F2: in a shared (world-writable) temp directory the socket and lock file live in a
//!   private `mbar-<uid>` subdirectory, and a subdirectory that is not private to the
//!   user makes the daemon refuse to start instead of using it.

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");
const USER: &str = "mbarident";
const TIMEOUT: Duration = Duration::from_secs(15);

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    bin: PathBuf,
}

impl Sandbox {
    /// `tmp_mode` is applied to the sandbox's `TMPDIR`.
    fn new(tmp_mode: u32) -> Sandbox {
        let root = PathBuf::from("/tmp").join(format!(
            "mbid-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let (home, tmp, bin) = (root.join("home"), root.join("tmp"), root.join("bin"));
        for d in [&home, &tmp, &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(tmp_mode)).unwrap();
        std::os::unix::fs::symlink(EXE, bin.join("mbar")).unwrap();
        std::os::unix::fs::symlink(EXE, bin.join("sketchybar")).unwrap();
        Sandbox {
            root,
            home,
            tmp,
            bin,
        }
    }

    fn command(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        c.env_clear()
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env("MBAR_LOCK_DIR", &self.tmp)
            .env("USER", USER)
            .env("MBAR_ALLOW_ROOT", "1");
        c
    }

    fn private_subdir(&self) -> PathBuf {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        self.tmp.join(format!("mbar-{uid}"))
    }

    fn spawn_daemon(&self, program: &Path) -> Daemon {
        let child = self
            .command(program)
            .arg("--headless")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Daemon { child }
    }

    /// Starts the daemon through `program` and waits until `mbar --query bar` answers.
    fn daemon(&self, program: &Path) -> Daemon {
        let mut d = self.spawn_daemon(program);
        let start = Instant::now();
        loop {
            if let Some(status) = d.child.try_wait().unwrap() {
                panic!("daemon exited early ({status}):\n{}", d.output());
            }
            let out = self.run(Path::new(EXE), &["--query", "bar"]);
            if out.status.success() && stdout(&out).starts_with('{') {
                return d;
            }
            assert!(start.elapsed() < TIMEOUT, "daemon did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn run(&self, program: &Path, args: &[&str]) -> Output {
        self.command(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn wait_label(&self, item: &str, want: &str) {
        let start = Instant::now();
        loop {
            let out = self.run(Path::new(EXE), &["--query", item]);
            let label = serde_json::from_str::<Value>(&stdout(&out))
                .ok()
                .and_then(|v| v["label"]["value"].as_str().map(str::to_string));
            if label.as_deref() == Some(want) {
                return;
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "label of {item} never became {want:?} (last {label:?})"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Daemon {
    child: Child,
}

impl Daemon {
    fn output(&mut self) -> String {
        use std::io::Read;
        let mut s = String::new();
        if let Some(mut o) = self.child.stdout.take() {
            let _ = o.read_to_string(&mut s);
        }
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut s);
        }
        s
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn daemon_started_as_sketchybar_exports_bar_name_sketchybar() {
    let sb = Sandbox::new(0o755);
    let sketchybar = sb.bin.join("sketchybar");
    // Same IPC identity: `mbar` clients reach a daemon started as `sketchybar`.
    let _d = sb.daemon(&sketchybar);
    let out = sb.run(
        &sketchybar,
        &[
            "--add",
            "item",
            "bn",
            "left",
            "--set",
            "bn",
            "script=sketchybar --set $NAME label=\"$BAR_NAME\"",
            "--update",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    sb.wait_label("bn", "sketchybar");
}

#[test]
fn daemon_started_as_mbar_exports_bar_name_mbar() {
    let sb = Sandbox::new(0o755);
    let _d = sb.daemon(Path::new(EXE));
    let out = sb.run(
        Path::new(EXE),
        &[
            "--add",
            "item",
            "bn",
            "left",
            "--set",
            "bn",
            "script=mbar --set $NAME label=\"$BAR_NAME\"",
            "--update",
        ],
    );
    assert!(out.status.success());
    sb.wait_label("bn", "mbar");
}

#[test]
fn shared_tmpdir_uses_private_subdirectory() {
    let sb = Sandbox::new(0o1777);
    let _d = sb.daemon(Path::new(EXE));
    let dir = sb.private_subdir();
    let meta = std::fs::symlink_metadata(&dir).unwrap();
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    let sock = dir.join(format!("mbar_{USER}_mbar.socket"));
    let sock_meta = std::fs::metadata(&sock).unwrap();
    assert_eq!(sock_meta.permissions().mode() & 0o777, 0o600);
    assert!(dir.join(format!("mbar_{USER}_mbar.lock")).exists());
    // Nothing at the old, squattable location.
    assert!(!sb.tmp.join(format!("mbar_{USER}_mbar.socket")).exists());
}

#[test]
fn insecure_private_subdirectory_is_refused() {
    let sb = Sandbox::new(0o1777);
    let dir = sb.private_subdir();
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let mut d = sb.spawn_daemon(Path::new(EXE));
    let start = Instant::now();
    let status = loop {
        if let Some(s) = d.child.try_wait().unwrap() {
            break s;
        }
        assert!(start.elapsed() < TIMEOUT, "daemon did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(1));
    assert!(
        d.output()
            .contains("mbar: could not create lock-file! abort.."),
        "unexpected output"
    );
    assert!(!dir.join(format!("mbar_{USER}_mbar.socket")).exists());
}

#[test]
fn planted_symlink_subdirectory_is_refused() {
    let sb = Sandbox::new(0o1777);
    let target = sb.root.join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&target, sb.private_subdir()).unwrap();
    let mut d = sb.spawn_daemon(Path::new(EXE));
    let start = Instant::now();
    let status = loop {
        if let Some(s) = d.child.try_wait().unwrap() {
            break s;
        }
        assert!(start.elapsed() < TIMEOUT, "daemon did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(1), "{}", d.output());
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
}
