//! Review regressions of child processes (`crates/mbar/src/scripts.rs`) through Lua
//! `mbar.exec`, end to end: a headless daemon in an isolated `HOME` / `TMPDIR` (harness
//! trimmed from `tests/review_driver.rs`). The unit tests in `scripts.rs` cover the timeout
//! and capture details with short timeouts.

use serde_json::Value;
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
}

impl Sandbox {
    fn new() -> Sandbox {
        // Keep it short: Unix socket paths are limited to ~100 bytes.
        let root = PathBuf::from("/tmp").join(format!(
            "mbrs-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let (home, tmp) = (root.join("home"), root.join("tmp"));
        for d in [&home, &tmp] {
            std::fs::create_dir_all(d).unwrap();
        }
        Sandbox { root, home, tmp }
    }

    fn write_init_lua(&self, src: &str) {
        let d = self.home.join(".config/mbar");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("init.lua"), src).unwrap();
    }

    fn command(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
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

    fn daemon(&self) -> Daemon {
        let child = self
            .command(Path::new(EXE))
            .arg("--headless")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut d = Daemon { child };
        let start = Instant::now();
        loop {
            if let Some(status) = d.child.try_wait().unwrap() {
                panic!("daemon exited early ({status})");
            }
            if self.socket().exists() {
                let out = self.mbar(&["--query", "bar"]);
                if out.status.success() && stdout(&out).starts_with('{') {
                    return d;
                }
            }
            assert!(start.elapsed() < TIMEOUT, "daemon did not become ready");
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

    /// Polls `--query <item>` until `pred` holds; returns the item and the time it took.
    fn wait_for(&self, item: &str, what: &str, pred: impl Fn(&Value) -> bool) -> Duration {
        let start = Instant::now();
        loop {
            let out = self.mbar(&["--query", item]);
            if let Ok(v) = serde_json::from_str::<Value>(&stdout(&out)) {
                if pred(&v) {
                    return start.elapsed();
                }
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "timeout waiting for {item}: {what}; last: {}",
                stdout(&out)
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

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn label(v: &Value) -> &str {
    v["label"]["value"].as_str().unwrap_or("")
}

/// F5: the callback fires when the shell exits, not when a background process that
/// inherited the pipe exits.
#[test]
fn review_f5_exec_callback_not_held_by_background_child() {
    let sb = Sandbox::new();
    sb.write_init_lua(
        r#"
local mbar = require("mbar")
mbar.add("item", "a", { label = "hi" })
mbar.exec("sleep 8 & echo done", function(r) mbar.set("a", { label = "cb:" .. r }) end)
"#,
    );
    let _d = sb.daemon();
    let took = sb.wait_for("a", "exec callback", |v| label(v) == "cb:done\n");
    assert!(took < Duration::from_secs(4), "{took:?}");
}

/// R6 / F5: captured output is capped (4 MiB) instead of buffering everything.
#[test]
fn review_r6_exec_capture_is_capped() {
    let sb = Sandbox::new();
    sb.write_init_lua(
        r#"
local mbar = require("mbar")
mbar.add("item", "a", { label = "hi" })
mbar.exec("head -c 100000000 /dev/zero", function(_, raw)
  mbar.set("a", { label = tostring(#raw) })
end)
"#,
    );
    let _d = sb.daemon();
    sb.wait_for("a", "capped exec output", |v| label(v) == "4194304");
}
