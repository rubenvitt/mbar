#![allow(dead_code)]
//! Review regression RT-3, end to end: the headless main loop must pace animation frames
//! (D12, 60 Hz) instead of busy-spinning while an animation runs (harness trimmed from
//! `tests/review_driver.rs`).

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");
const USER: &str = "mbartest";
const TIMEOUT: Duration = Duration::from_secs(15);

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    bin: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        // Keep it short: Unix socket paths are limited to ~100 bytes.
        let root = PathBuf::from("/tmp").join(format!(
            "mbap-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let (home, tmp, bin) = (root.join("home"), root.join("tmp"), root.join("bin"));
        for d in [&home, &tmp, &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::os::unix::fs::symlink(EXE, bin.join("mbar")).unwrap();
        std::os::unix::fs::symlink(EXE, bin.join("sketchybar")).unwrap();
        Sandbox {
            root,
            home,
            tmp,
            bin,
        }
    }

    fn config_dir(&self) -> PathBuf {
        let d = self.home.join(".config/mbar");
        std::fs::create_dir_all(&d).unwrap();
        d
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

    fn socket(&self) -> PathBuf {
        self.tmp.join(format!("mbar_{USER}_mbar.socket"))
    }

    /// Starts `mbar --headless` and waits until it answers `--query bar`.
    fn daemon(&self) -> Daemon {
        let child = self
            .command(Path::new(EXE))
            .arg("--headless")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut d = Daemon { child };
        let start = Instant::now();
        loop {
            if let Some(status) = d.child.try_wait().unwrap() {
                let out = d.take_output();
                panic!("daemon exited early ({status}):\n{out}");
            }
            if self.socket().exists() {
                let out = self.mbar(&["--query", "bar"]);
                if out.status.success() && stdout(&out).starts_with('{') {
                    return d;
                }
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "daemon did not become ready:\n{}",
                d.kill_and_output()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn mbar(&self, args: &[&str]) -> Output {
        self.command(Path::new(EXE))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// Polls `--query <item>` until `pred` holds.
    fn wait_for(&self, item: &str, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let start = Instant::now();
        loop {
            let out = self.mbar(&["--query", item]);
            if out.status.success() {
                if let Ok(v) = serde_json::from_str::<Value>(&stdout(&out)) {
                    if pred(&v) {
                        return v;
                    }
                }
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "timeout waiting for {item}: {what}; last: {} {}",
                stdout(&out),
                stderr(&out)
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
    fn take_output(&mut self) -> String {
        let mut s = String::new();
        if let Some(mut o) = self.child.stdout.take() {
            let _ = o.read_to_string(&mut s);
        }
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut s);
        }
        s
    }

    fn kill_and_output(&mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.take_output()
    }

    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "daemon did not exit:\n{}",
                self.kill_and_output()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
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

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn frames(sb: &Sandbox) -> u64 {
    let out = sb.mbar(&["--query", "stats"]);
    let v: Value =
        serde_json::from_str(&stdout(&out)).unwrap_or_else(|e| panic!("{e}: {}", stdout(&out)));
    v["frames"]
        .as_u64()
        .or_else(|| v["frames"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or_else(|| panic!("no frames in stats: {v}"))
}

/// A 600-frame (10 s) animation of 0 -> 60000 changes the value every millisecond, so every
/// loop iteration renders a frame (frames without changes are not counted): about 60 per
/// second when paced, thousands when the loop spins.
#[test]
fn headless_animation_frames_are_paced() {
    let sb = Sandbox::new();
    std::fs::write(sb.config_dir().join("mbarrc"), "").unwrap();
    let _d = sb.daemon();
    let out = sb.mbar(&[
        "--add",
        "item",
        "a",
        "left",
        "--animate",
        "linear",
        "600",
        "--set",
        "a",
        "y_offset=60000",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let before = frames(&sb);
    let t = Instant::now();
    std::thread::sleep(Duration::from_millis(1000));
    let after = frames(&sb);
    let secs = t.elapsed().as_secs_f64();
    // Still animating (the value is between start and target).
    let v = sb.wait_for("a", "mid-animation", |_| true);
    let y = v["geometry"]["y_offset"].as_i64().unwrap();
    assert!(y > 0 && y < 60000, "animation not running: y_offset={y}");
    let rate = (after - before) as f64 / secs;
    assert!(
        rate < 150.0,
        "{} frames in {secs:.2} s: the headless loop is not paced",
        after - before
    );
    assert!(rate > 10.0, "animation frames stalled: {rate:.1}/s");
}
