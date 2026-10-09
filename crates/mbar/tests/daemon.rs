//! End-to-end tests of the `mbar` binary: a headless daemon in an isolated `HOME` /
//! `TMPDIR`, driven by client invocations of the same binary (also through the
//! `sketchybar` and `borders` symlinks).

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");
const USER: &str = "mbartest";
const TIMEOUT: Duration = Duration::from_secs(15);

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// An isolated environment: `$ROOT/{home,tmp,bin}`; `bin` holds `mbar`, `sketchybar` and
/// `borders` symlinks to the binary under test and is first in `PATH` (for config
/// scripts).
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
            "mbit-{}-{}",
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
        std::os::unix::fs::symlink(EXE, bin.join("borders")).unwrap();
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
            .env("MBAR_ALLOW_ROOT", "1")
            .env("MBAR_TEST_BASE", "base");
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

    fn run(&self, program: &Path, args: &[&str]) -> Output {
        self.command(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn mbar(&self, args: &[&str]) -> Output {
        self.run(Path::new(EXE), args)
    }

    fn sketchybar(&self, args: &[&str]) -> Output {
        self.run(&self.bin.join("sketchybar"), args)
    }

    fn borders(&self, args: &[&str]) -> Output {
        self.run(&self.bin.join("borders"), args)
    }

    /// `--query <item>` parsed as JSON.
    fn query(&self, item: &str) -> Value {
        let out = self.mbar(&["--query", item]);
        assert!(out.status.success(), "query {item}: {}", stderr(&out));
        serde_json::from_str(&stdout(&out))
            .unwrap_or_else(|e| panic!("query {item}: {e}: {}", stdout(&out)))
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
            assert!(start.elapsed() < TIMEOUT, "daemon did not exit");
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

// ---------------------------------------------------------------------------- client

#[test]
fn client_without_daemon_is_silent() {
    let sb = Sandbox::new();
    let out = sb.mbar(&["--query", "bar"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    let out = sb.mbar(&["-m"]);
    assert!(out.status.success());
}

#[test]
fn version_and_help() {
    let sb = Sandbox::new();
    let out = sb.sketchybar(&["--version"]);
    assert_eq!(stdout(&out), "sketchybar-v2.24.0\n");
    let out = sb.mbar(&["-v"]);
    assert!(stdout(&out).starts_with("mbar-v"));
    let out = sb.mbar(&["--help"]);
    assert!(out.status.success());
    assert!(stdout(&out).starts_with(&format!("Usage: {EXE} [options]\n")));
    let out = sb.mbar(&["--config"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stdout(&out),
        "[!] Error: Too few arguments for argument 'config'.\n"
    );
    let out = sb.mbar(&["-c", "/nonexistent/rc"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stdout(&out),
        "[!] Error: Specified config file path invalid.\n"
    );
}

#[test]
fn client_requires_user() {
    let sb = Sandbox::new();
    let out = sb
        .command(Path::new(EXE))
        .env_remove("USER")
        .args(["--query", "bar"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "sketchybar-msg: 'env USER' not set! abort..\n"
    );
}

// ---------------------------------------------------------------------------- daemon

#[test]
fn add_set_query() {
    let sb = Sandbox::new();
    let _d = sb.daemon();

    let out = sb.mbar(&["--add", "item", "foo", "left"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = sb.mbar(&["--set", "foo", "label=hello", "icon=X", "width=50"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let v = sb.query("foo");
    assert_eq!(v["name"], "foo");
    assert_eq!(v["type"], "item");
    assert_eq!(v["geometry"]["position"], "left");
    assert_eq!(v["geometry"]["width"], 50);
    assert_eq!(label(&v), "hello");
    assert_eq!(v["icon"]["value"], "X");

    // `-m` prefix and batched commands in one message.
    let out = sb.mbar(&[
        "-m", "--add", "item", "bar1", "right", "--set", "bar1", "label=a", "--set", "foo",
        "label=b",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(label(&sb.query("bar1")), "a");
    assert_eq!(label(&sb.query("foo")), "b");

    // `--query bar` lists the items.
    let out = sb.mbar(&["--query", "bar"]);
    let bar: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let items: Vec<&str> = bar["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(items, ["foo", "bar1"]);
}

#[test]
fn error_response_goes_to_stderr() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    let out = sb.mbar(&["--set", "missing", "label=x"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert_eq!(stderr(&out), "[!] Set: Item not found 'missing'\n");
}

#[test]
fn sketchybar_symlink_reaches_same_daemon() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    let out = sb.sketchybar(&[
        "--add", "item", "viasb", "right", "--set", "viasb", "label=sb",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(label(&sb.query("viasb")), "sb");
    let out = sb.sketchybar(&["--query", "viasb"]);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["geometry"]["position"], "right");
}

#[test]
fn second_daemon_refuses_to_start() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    let out = sb.mbar(&["--headless"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "mbar: could not acquire lock-file... already running?\n"
    );
}

#[test]
fn exit_stops_daemon() {
    let sb = Sandbox::new();
    let mut d = sb.daemon();
    let out = sb.mbar(&["--exit"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    assert!(d.wait_exit().success());
    assert!(!sb.socket().exists());
    // Nobody listens any more: silent exit 0.
    let out = sb.mbar(&["--query", "bar"]);
    assert!(out.status.success() && out.stdout.is_empty());
}

#[test]
fn scripts_get_a_fresh_env() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    let log = sb.root.join("script.log");
    let script = format!(
        "echo \"$NAME|$SENDER|$FOO|$MBAR_TEST_BASE|$BAR_NAME\" >> '{}'",
        log.display()
    );
    let set_script = format!("script={script}");
    let out = sb.mbar(&[
        "--add",
        "event",
        "ping",
        "--add",
        "item",
        "s",
        "left",
        "--set",
        "s",
        &set_script,
        "--subscribe",
        "s",
        "ping",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(sb.mbar(&["--trigger", "ping", "FOO=1"]).status.success());
    wait_lines(&log, 1);
    assert!(sb.mbar(&["--trigger", "ping"]).status.success());
    let lines = wait_lines(&log, 2);
    assert_eq!(lines[0], "s|ping|1|base|mbar");
    // D1: FOO from the first run must not leak into the second.
    assert_eq!(lines[1], "s|ping||base|mbar");
}

fn wait_lines(path: &Path, n: usize) -> Vec<String> {
    let start = Instant::now();
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        if lines.len() >= n {
            return lines;
        }
        assert!(
            start.elapsed() < TIMEOUT,
            "timeout waiting for {n} line(s) in {}: {lines:?}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------- configs

#[test]
fn shell_config_runs_and_reloads() {
    let sb = Sandbox::new();
    let dir = sb.config_dir();
    std::fs::write(dir.join("value"), "one").unwrap();
    // No executable bit: the daemon adds it (cli.md §10.2 step 4).
    std::fs::write(
        dir.join("mbarrc"),
        "#!/bin/sh\n\
         mbar --add item cfg right \\\n\
              --set cfg label=\"$BAR_NAME:$(basename \"$CONFIG_DIR\"):$(cat value)\"\n",
    )
    .unwrap();
    let _d = sb.daemon();
    sb.wait_for("cfg", "label from config", |v| label(v) == "mbar:mbar:one");

    // --reload re-runs the config on a fresh state.
    std::fs::write(dir.join("value"), "two").unwrap();
    let out = sb.mbar(&["--add", "item", "transient", "left", "--reload"]);
    assert!(out.status.success(), "{}", stderr(&out));
    sb.wait_for("cfg", "label after reload", |v| label(v) == "mbar:mbar:two");
    let out = sb.mbar(&["--query", "transient"]);
    assert_eq!(out.status.code(), Some(1), "item survived the reload");
}

#[test]
fn hotload_reloads_on_change() {
    let sb = Sandbox::new();
    let dir = sb.config_dir();
    std::fs::write(dir.join("value"), "one").unwrap();
    std::fs::write(
        dir.join("sketchybarrc"),
        "sketchybar --add item hl right --set hl label=\"$(cat value)\"\n",
    )
    .unwrap();
    let _d = sb.daemon();
    sb.wait_for("hl", "initial label", |v| label(v) == "one");
    assert!(sb.mbar(&["--hotload", "on"]).status.success());
    // The watcher takes its baseline within one period after enabling.
    std::thread::sleep(Duration::from_millis(2500));
    std::fs::write(dir.join("value"), "two").unwrap();
    sb.wait_for("hl", "label after hotload", |v| label(v) == "two");
}

#[test]
fn lua_config_handlers_exec_and_delay() {
    let sb = Sandbox::new();
    let dir = sb.config_dir();
    // init.lua wins over mbarrc in the same directory.
    std::fs::write(dir.join("mbarrc"), "mbar --add item wrong left\n").unwrap();
    std::fs::write(
        dir.join("init.lua"),
        r#"
local mbar = require("mbar")
mbar.add("event", "ping")
local item = mbar.add("item", "lua", { position = "right", label = "start" })
item:subscribe("ping", function(env)
  item:set({ label = "got:" .. (env.MSG or "") })
end)
mbar.add("item", "dir", { label = CONFIG_DIR:match("[^/]+$") })
mbar.exec("echo exec-out", function(result)
  mbar.set("dir", { icon = (result:gsub("%s+$", "")) })
end)
mbar.delay(0.1, function() mbar.set("lua", { icon = "delayed" }) end)
"#,
    )
    .unwrap();
    let _d = sb.daemon();
    let v = sb.wait_for("lua", "delay callback", |v| v["icon"]["value"] == "delayed");
    assert_eq!(label(&v), "start");
    assert!(v["scripting"]["script"]
        .as_str()
        .unwrap_or("")
        .starts_with("lua:"));
    sb.wait_for("dir", "exec callback", |v| {
        label(v) == "mbar" && v["icon"]["value"] == "exec-out"
    });
    assert_eq!(sb.mbar(&["--query", "wrong"]).status.code(), Some(1));

    let out = sb.mbar(&["--trigger", "ping", "MSG=pong"]);
    assert!(out.status.success(), "{}", stderr(&out));
    sb.wait_for("lua", "handler result", |v| label(v) == "got:pong");
}

// ---------------------------------------------------------------------------- borders

/// Borders design §2: `borders <args>` reaches the running default bar and exits 0
/// (whatever the daemon answers); without a valid argument it reports the running
/// instance instead of starting one.
#[test]
fn borders_symlink_reaches_running_daemon() {
    let sb = Sandbox::new();
    let mut d = sb.daemon();
    let out = sb.borders(&["width=5.0"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(out.stdout.is_empty(), "{}", stdout(&out));

    let already = "A borders instance is already running and no valid arguments where \
                   provided. To modify properties of the running instance provide them as \
                   arguments.\n";
    let out = sb.borders(&[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "{}", stdout(&out));
    assert_eq!(stderr(&out), already);
    let out = sb.borders(&["bogus"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "[?] Borders: Invalid argument 'bogus'\n");
    assert_eq!(stderr(&out), already);

    // Still the one daemon, untouched by the probes.
    let out = sb.mbar(&["--query", "bar"]);
    assert!(out.status.success() && stdout(&out).starts_with('{'));
    assert!(
        d.child.try_wait().unwrap().is_none(),
        "{}",
        d.kill_and_output()
    );
}

/// End to end: `borders` and `mbar --borders` change the configuration that
/// `--query borders` reports; errors of the `--borders` domain follow mbar's `[!]` rule.
#[test]
fn borders_configuration_round_trip() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    let v = sb.query("borders");
    assert_eq!(v["drawing"], "off");
    assert_eq!(v["width"], 4.0);

    let out = sb.borders(&[
        "active_color=gradient(top_left=0xff112233,bottom_right=0xff445566)",
        "inactive_color=0xff494d64",
        "width=6.5",
        "style=square",
        "blacklist=Safari,kitty",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = sb.query("borders");
    assert_eq!(v["drawing"], "on");
    assert_eq!(
        v["active_color"],
        "gradient(top_left=0xff112233,bottom_right=0xff445566)"
    );
    assert_eq!(v["inactive_color"], "0xff494d64");
    assert_eq!(v["width"], 6.5);
    assert_eq!(v["style"], "square");
    assert_eq!(v["blacklist"], serde_json::json!(["Safari", "kitty"]));

    let out = sb.mbar(&["--borders", "apply-to=42", "active_color=glow(0xff00ff00)"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v = sb.query("borders");
    assert_eq!(v["overrides"][0]["window"], 42);
    assert_eq!(v["overrides"][0]["active_color"], "glow(0xff00ff00)");
    assert_eq!(
        v["active_color"],
        "gradient(top_left=0xff112233,bottom_right=0xff445566)"
    );

    let out = sb.mbar(&["--borders", "drawing=off", "glow=1"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "[!] Borders: Invalid argument 'glow=1'\n");
    assert_eq!(sb.query("borders")["drawing"], "off");

    // `--reload` keeps the configuration (no config, no bordersrc in the sandbox): the
    // bar items are gone, the borders settings and overrides are not.
    let out = sb.mbar(&["--borders", "drawing=on"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let before = sb.query("borders");
    let out = sb.mbar(&["--add", "item", "transient", "left", "--reload"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = sb.mbar(&["--query", "transient"]);
    assert_eq!(out.status.code(), Some(1), "item survived the reload");
    let v = sb.query("borders");
    assert_eq!(v, before);
    assert_eq!(v["drawing"], "on");
    assert_eq!(v["width"], 6.5);
    assert_eq!(v["style"], "square");
    assert_eq!(v["overrides"][0]["window"], 42);
}

/// Borders design §3: the default bar runs `~/.config/borders/bordersrc` after its main
/// config, with the sandbox `bin` (the `borders` link) on `PATH`, and again on every
/// `--reload`. The executable bit is added by the daemon.
#[test]
fn bordersrc_runs_after_config_and_on_reload() {
    let sb = Sandbox::new();
    let marker = sb.root.join("marker");
    let dir = sb.config_dir();
    // Lua configs run synchronously, so the order of the marker lines is fixed.
    std::fs::write(
        dir.join("init.lua"),
        format!(
            "local f = assert(io.open('{}', 'a'))\nf:write('config\\n')\nf:close()\n",
            marker.display()
        ),
    )
    .unwrap();
    let brc = sb.home.join(".config/borders");
    std::fs::create_dir_all(&brc).unwrap();
    std::fs::write(
        brc.join("bordersrc"),
        format!(
            "#!/bin/sh\n\
             borders active_color=0xffe1e3e4 width=5.0\n\
             rc=$?\n\
             echo \"bordersrc:$BAR_NAME:$(command -v borders):$rc\" >> '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    // A `~/.bordersrc` loses against `~/.config/borders/bordersrc` (BR-CFG-02).
    std::fs::write(
        sb.home.join(".bordersrc"),
        format!("#!/bin/sh\necho wrong >> '{}'\n", marker.display()),
    )
    .unwrap();
    let _d = sb.daemon();
    let expected_rc = format!("bordersrc:mbar:{}:0", sb.bin.join("borders").display());
    let lines = wait_lines(&marker, 2);
    assert_eq!(lines, ["config".to_string(), expected_rc.clone()]);
    let v = sb.query("borders");
    assert_eq!(v["drawing"], "on");
    assert_eq!(v["width"], 5.0);

    let out = sb.mbar(&["--reload"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = wait_lines(&marker, 4);
    assert_eq!(
        lines,
        [
            "config".to_string(),
            expected_rc.clone(),
            "config".to_string(),
            expected_rc
        ]
    );
}

/// Without a main config the default bar still runs `~/.bordersrc` (a JankyBorders user
/// who never had a SketchyBar config).
#[test]
fn bordersrc_runs_without_main_config() {
    let sb = Sandbox::new();
    let marker = sb.root.join("marker");
    // No shebang and no executable bit: run like a shebang-less shell config.
    std::fs::write(
        sb.home.join(".bordersrc"),
        format!(
            "borders width=5.0\necho \"home:$?\" >> '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    let _d = sb.daemon();
    assert_eq!(wait_lines(&marker, 1), ["home:0"]);
}

// ---------------------------------------------------------------------------- --monitor

#[test]
fn monitor_streams_events() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    assert!(sb
        .mbar(&[
            "--add",
            "event",
            "ping",
            "--add",
            "item",
            "m",
            "left",
            "--subscribe",
            "m",
            "ping"
        ])
        .status
        .success());
    let mut mon = sb
        .command(Path::new(EXE))
        .args(["--monitor", "events"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let out = mon.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // The subscription is registered asynchronously: trigger until a line arrives.
    let start = Instant::now();
    let line = loop {
        assert!(sb.mbar(&["--trigger", "ping", "K=V"]).status.success());
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            break line;
        }
        assert!(start.elapsed() < TIMEOUT, "no monitor line received");
    };
    let _ = mon.kill();
    let _ = mon.wait();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    assert_eq!(v["type"], "event");
    assert_eq!(v["name"], "ping");
}

#[test]
fn monitor_streams_stats() {
    let sb = Sandbox::new();
    let _d = sb.daemon();
    let mut mon = sb
        .command(Path::new(EXE))
        .args(["--monitor", "stats"])
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
    // Snapshots follow the 1 s routine clock.
    let line = rx.recv_timeout(TIMEOUT).expect("no stats line");
    let _ = mon.kill();
    let _ = mon.wait();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    assert_eq!(v["type"], "stats");
    assert!(v["uptime_s"].is_number(), "{line}");
}
