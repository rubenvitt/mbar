//! Review regressions of the daemon driver (`crates/mbar/src/driver.rs`), end to end: a
//! headless daemon in an isolated `HOME` / `TMPDIR`, driven by client invocations of the
//! same binary (harness trimmed from `tests/daemon.rs`).

use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
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
            "mbrv-{}-{}",
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

fn label(v: &Value) -> &str {
    v["label"]["value"].as_str().unwrap_or("")
}

fn stats(sb: &Sandbox) -> Value {
    let out = sb.mbar(&["--query", "stats"]);
    serde_json::from_str(&stdout(&out)).unwrap_or_else(|e| panic!("{e}: {}", stdout(&out)))
}

fn has_bash() -> bool {
    Command::new("bash")
        .args(["-c", "true"])
        .status()
        .is_ok_and(|s| s.success())
}

// ---------------------------------------------------------------------------- CLI-1

/// The stock rc has no shebang and uses bash arrays; it must not run under dash.
#[test]
fn shebang_less_rc_runs_with_bash() {
    if !has_bash() {
        eprintln!("bash not installed: skipped");
        return;
    }
    let sb = Sandbox::new();
    std::fs::write(
        sb.config_dir().join("sketchybarrc"),
        "sketchybar --bar height=40\n\
         names=( one two )\n\
         for i in \"${!names[@]}\"; do\n\
           sketchybar --add item \"${names[i]}\" left --set \"${names[i]}\" label=\"$i\"\n\
         done\n",
    )
    .unwrap();
    let _d = sb.daemon();
    sb.wait_for("two", "item from the array loop", |v| label(v) == "1");
    assert_eq!(label(&sb.wait_for("one", "first item", |_| true)), "0");
}

// ---------------------------------------------------------------------------- R2 / F4

/// A Lua handler that re-triggers its own event must not freeze IPC, and `--exit` still
/// works.
#[test]
fn self_triggering_lua_handler_keeps_daemon_responsive() {
    let sb = Sandbox::new();
    std::fs::write(
        sb.config_dir().join("init.lua"),
        r#"
local mbar = require("mbar")
mbar.add("event", "loop")
local n = 0
local it = mbar.add("item", "x", { position = "right" })
it:subscribe("loop", function()
  n = n + 1
  mbar.trigger("loop")
end)
"#,
    )
    .unwrap();
    let mut d = sb.daemon();
    assert!(sb.mbar(&["--trigger", "loop"]).status.success());
    std::thread::sleep(Duration::from_millis(300));
    for _ in 0..3 {
        let t = Instant::now();
        let out = sb.mbar(&["--query", "x"]);
        assert!(
            stdout(&out).starts_with('{'),
            "no reply while the handler loops: {} {}",
            stdout(&out),
            stderr(&out)
        );
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }
    assert!(sb.mbar(&["--exit"]).status.success());
    assert!(d.wait_exit().success());
}

// ---------------------------------------------------------------------------- F1

/// `mbar.delay` with an absurd delay neither kills the daemon nor fires at once.
#[test]
fn huge_lua_delay_keeps_daemon_alive() {
    let sb = Sandbox::new();
    std::fs::write(
        sb.config_dir().join("init.lua"),
        r#"
local mbar = require("mbar")
mbar.add("item", "a", { label = "hi" })
mbar.delay(1e19, function() mbar.set("a", { label = "fired" }) end)
mbar.delay(1e300, function() mbar.set("a", { label = "fired" }) end)
mbar.delay(0.05, function() mbar.set("a", { icon = "ok" }) end)
"#,
    )
    .unwrap();
    let mut d = sb.daemon();
    let v = sb.wait_for("a", "short delay fired", |v| v["icon"]["value"] == "ok");
    assert_eq!(label(&v), "hi");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(label(&sb.wait_for("a", "still there", |_| true)), "hi");
    assert!(d.child.try_wait().unwrap().is_none(), "daemon died");
}

// ---------------------------------------------------------------------------- F7

/// `CONFIG_DIR` is in the daemon's own environment (Lua `os.getenv`, `io.popen`).
#[test]
fn config_dir_in_daemon_environment() {
    let sb = Sandbox::new();
    let dir = sb.config_dir();
    std::fs::write(
        dir.join("init.lua"),
        r##"
local mbar = require("mbar")
local p = io.popen('printf %s "$CONFIG_DIR"')
local out = p:read("a")
p:close()
mbar.add("item", "a", { label = tostring(os.getenv("CONFIG_DIR")) .. "#" .. out })
"##,
    )
    .unwrap();
    let _d = sb.daemon();
    let v = sb.wait_for("a", "label", |v| !label(v).is_empty());
    let d = dir.display().to_string();
    assert_eq!(label(&v), format!("{d}#{d}"));
}

// ---------------------------------------------------------------------------- F9

/// `--monitor` that the daemon never executes (an empty argument ends the message) gets a
/// normal reply and the client exits instead of hanging.
#[test]
fn unexecuted_monitor_does_not_hang_client() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    assert!(sb.mbar(&["--add", "item", "a", "left"]).status.success());
    let mut child = sb
        .command(Path::new(EXE))
        .args(["--set", "a", "label=x", "", "--monitor"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if start.elapsed() > Duration::from_secs(4) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("client hangs on a --monitor the daemon did not run");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
}

/// Output of commands before `--monitor` is delivered as the first frame.
#[test]
fn monitor_keeps_earlier_query_output() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    assert!(sb.mbar(&["--add", "item", "a", "left"]).status.success());
    let mut mon = sb
        .command(Path::new(EXE))
        .args(["--query", "a", "--monitor", "stats"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let out = mon.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let first = rx.recv_timeout(TIMEOUT).expect("no output");
    assert_eq!(first, "{");
    let mut query = vec![first];
    let stats_line = loop {
        let line = rx.recv_timeout(TIMEOUT).expect("no stats line");
        if line.starts_with("{\"type\":\"stats\"") {
            break line;
        }
        query.push(line);
    };
    let _ = mon.kill();
    let _ = mon.wait();
    let v: Value = serde_json::from_str(&query.join("\n")).unwrap();
    assert_eq!(v["name"], "a");
    assert!(stats_line.contains("uptime_s"));
}

// ---------------------------------------------------------------------------- F11

/// `lua.avg_us`/`max_us` are measured, and exec/delay callbacks are counted.
#[test]
fn lua_stats_are_filled() {
    let sb = Sandbox::new();
    std::fs::write(
        sb.config_dir().join("init.lua"),
        r#"
local mbar = require("mbar")
local function work() local s = 0 for i = 1, 200000 do s = s + i end return s end
mbar.add("event", "ping")
local it = mbar.add("item", "a", { label = "start" })
it:subscribe("ping", function() work(); it:set({ label = "handled" }) end)
mbar.delay(0.01, function() work(); mbar.set("a", { icon = "delayed" }) end)
mbar.add("item", "b", { label = "start" })
mbar.exec("echo hi", function() work(); mbar.set("b", { label = "exec" }) end)
"#,
    )
    .unwrap();
    let _d = sb.daemon();
    sb.wait_for("a", "delay callback", |v| v["icon"]["value"] == "delayed");
    sb.wait_for("b", "exec callback", |v| label(v) == "exec");
    assert!(sb.mbar(&["--trigger", "ping"]).status.success());
    sb.wait_for("a", "handler", |v| label(v) == "handled");
    let lua = &stats(&sb)["lua"];
    assert!(lua["callbacks"].as_u64().unwrap() >= 3, "{lua}");
    assert!(lua["avg_us"].as_u64().unwrap() > 0, "{lua}");
    assert!(
        lua["max_us"].as_u64().unwrap() >= lua["avg_us"].as_u64().unwrap(),
        "{lua}"
    );
}

// ---------------------------------------------------------------------------- F12

/// The shell config's own exit does not count as a finished item script.
#[test]
fn config_exit_does_not_skew_running_scripts() {
    let sb = Sandbox::new();
    std::fs::write(
        sb.config_dir().join("mbarrc"),
        "#!/bin/sh\n\
         mbar --add item s left --set s script='sleep 4'\n\
         mbar --update\n",
    )
    .unwrap();
    let _d = sb.daemon();
    sb.wait_for("s", "item from config", |_| true);
    // Let the config exit be reaped and processed.
    let start = Instant::now();
    loop {
        let st = stats(&sb);
        if st["scripts"]["spawned"].as_u64() == Some(1) {
            break;
        }
        assert!(start.elapsed() < TIMEOUT, "script never spawned: {st}");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(500));
    let st = stats(&sb);
    assert_eq!(st["scripts"]["running"], 1, "{st}");
}
