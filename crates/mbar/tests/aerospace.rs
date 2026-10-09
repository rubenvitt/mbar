//! End-to-end tests of the AeroSpace integration
//! (`docs/superpowers/specs/2026-10-09-aerospace-design.md`): a headless daemon in an
//! isolated `HOME` / `TMPDIR` talks to a fake AeroSpace server on a Unix socket
//! (`MBAR_AEROSPACE_SOCKET`; `MBAR_AEROSPACE_CLI` points nowhere, so no real `aerospace`
//! is ever used).

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");
const USER: &str = "mbartest";
const TIMEOUT: Duration = Duration::from_secs(15);
const SERVER_VERSION: &str = "0.0.0-Fake e2e1234";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------- fake server

/// A fake AeroSpace: handshake (`u32 LE 1` both ways), then `u32 LE` length-prefixed
/// JSON frames. One-shot requests get a `ServerAnswer`; `subscribe` gets the initial
/// state and then the frames pushed with [`FakeAerospace::push`].
struct FakeAerospace {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<UnixStream>>>,
    subscribers: Arc<Mutex<Vec<UnixStream>>>,
    /// Arguments of every one-shot command except the subscription's empty version probe.
    commands: Arc<Mutex<Vec<Vec<String>>>>,
    accept: Option<JoinHandle<()>>,
}

/// AeroSpace's initial state after `subscribe` (it sends workspace, focus, monitor and
/// mode right away).
const INITIAL_EVENTS: [&str; 2] = [
    r#"{"_event":"focused-workspace-changed","prevWorkspace":"","workspace":"1"}"#,
    r#"{"_event":"mode-changed","mode":"main"}"#,
];

fn answer(args: &[String]) -> (i32, String, String) {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        [] => (1, String::new(), "no command given".into()),
        ["list-workspaces", "--all"] => (0, "1\n2\n3\n".into(), String::new()),
        ["list-windows", ..] => (
            0,
            r#"[{"app-name":"Finder","window-id":42},{"app-name":"Mail","window-id":7}]"#.into(),
            String::new(),
        ),
        ["fail"] => (2, String::new(), "boom".into()),
        _ => (0, String::new(), String::new()),
    }
}

impl FakeAerospace {
    fn start(path: &Path) -> FakeAerospace {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let conns: Arc<Mutex<Vec<UnixStream>>> = Arc::default();
        let subscribers: Arc<Mutex<Vec<UnixStream>>> = Arc::default();
        let commands: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
        let accept = {
            let (stop, conns, subscribers, commands) = (
                stop.clone(),
                conns.clone(),
                subscribers.clone(),
                commands.clone(),
            );
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            conns.lock().unwrap().push(stream.try_clone().unwrap());
                            let (subscribers, commands) = (subscribers.clone(), commands.clone());
                            thread::spawn(move || serve(stream, subscribers, commands));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
        };
        FakeAerospace {
            path: path.to_path_buf(),
            stop,
            conns,
            subscribers,
            commands,
            accept: Some(accept),
        }
    }

    fn subscribers(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }

    /// Waits until the daemon's subscription stream is registered.
    fn wait_subscribed(&self) {
        wait_until("a subscriber", || self.subscribers() > 0);
    }

    fn push(&self, event: &str) {
        for s in self.subscribers.lock().unwrap().iter_mut() {
            let _ = write_frame(s, event.as_bytes());
        }
    }

    fn commands(&self) -> Vec<Vec<String>> {
        self.commands.lock().unwrap().clone()
    }

    /// AeroSpace quits: stop listening, close every connection, remove the socket.
    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.accept.take() {
            t.join().unwrap();
        }
        for c in self.conns.lock().unwrap().drain(..) {
            let _ = c.shutdown(Shutdown::Both);
        }
        self.subscribers.lock().unwrap().clear();
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for FakeAerospace {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(
    mut s: UnixStream,
    subscribers: Arc<Mutex<Vec<UnixStream>>>,
    commands: Arc<Mutex<Vec<Vec<String>>>>,
) {
    let mut version = [0u8; 4];
    if s.read_exact(&mut version).is_err() || s.write_all(&1u32.to_le_bytes()).is_err() {
        return;
    }
    assert_eq!(u32::from_le_bytes(version), 1, "client protocol version");
    while let Some(frame) = read_frame(&mut s) {
        let req: Value = serde_json::from_slice(&frame).unwrap();
        let args: Vec<String> = req["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect();
        if args.first().map(String::as_str) == Some("subscribe") {
            for ev in INITIAL_EVENTS {
                if write_frame(&mut s, ev.as_bytes()).is_err() {
                    return;
                }
            }
            subscribers.lock().unwrap().push(s.try_clone().unwrap());
            // Like AeroSpace: keep the stream until the client goes away.
            let _ = s.read_to_end(&mut Vec::new());
            return;
        }
        if !args.is_empty() {
            commands.lock().unwrap().push(args.clone());
        }
        let (code, stdout, stderr) = answer(&args);
        let answer = json!({
            "exitCode": code,
            "stdout": stdout,
            "stderr": stderr,
            "serverVersionAndHash": SERVER_VERSION,
        });
        if write_frame(&mut s, answer.to_string().as_bytes()).is_err() {
            return;
        }
    }
}

fn write_frame(s: &mut UnixStream, payload: &[u8]) -> std::io::Result<()> {
    let mut buf = (payload.len() as u32).to_le_bytes().to_vec();
    buf.extend_from_slice(payload);
    s.write_all(&buf)
}

fn read_frame(s: &mut UnixStream) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).ok()?;
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    s.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn wait_until(what: &str, mut pred: impl FnMut() -> bool) {
    let start = Instant::now();
    while !pred() {
        assert!(start.elapsed() < TIMEOUT, "timeout waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------- sandbox

/// `$ROOT/{home,tmp,bin}` plus the fake AeroSpace socket `$ROOT/as.sock` (short: macOS
/// limits Unix socket paths to ~104 bytes).
struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    bin: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let root = PathBuf::from("/tmp").join(format!(
            "mbae-{}-{}",
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

    fn aerospace_socket(&self) -> PathBuf {
        self.root.join("as.sock")
    }

    fn server(&self) -> FakeAerospace {
        FakeAerospace::start(&self.aerospace_socket())
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
            .env("MBAR_AEROSPACE_SOCKET", self.aerospace_socket())
            .env("MBAR_AEROSPACE_CLI", self.root.join("no-aerospace"))
            .env("MBAR_AEROSPACE_BACKOFF_MS", "20");
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
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn mbar(&self, args: &[&str]) -> Output {
        self.command(Path::new(EXE))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn query(&self, what: &str) -> Value {
        let out = self.mbar(&["--query", what]);
        assert!(out.status.success(), "query {what}: {}", stderr(&out));
        serde_json::from_str(&stdout(&out))
            .unwrap_or_else(|e| panic!("query {what}: {e}: {}", stdout(&out)))
    }

    /// Polls `--query <what>` until `pred` holds.
    fn wait_for(&self, what: &str, desc: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let start = Instant::now();
        loop {
            let out = self.mbar(&["--query", what]);
            if out.status.success() {
                if let Ok(v) = serde_json::from_str::<Value>(&stdout(&out)) {
                    if pred(&v) {
                        return v;
                    }
                }
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "timeout waiting for {what}: {desc}; last: {} {}",
                stdout(&out),
                stderr(&out)
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_label(&self, item: &str, want: &str) -> Value {
        self.wait_for(item, &format!("label {want:?}"), |v| label(v) == want)
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

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn label(v: &Value) -> &str {
    v["label"]["value"].as_str().unwrap_or("")
}

fn argv(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

fn wait_lines(path: &Path, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
    let start = Instant::now();
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        if pred(&lines) {
            return lines;
        }
        assert!(
            start.elapsed() < TIMEOUT,
            "timeout waiting for lines in {}: {lines:?}",
            path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------- tests

/// `--query aerospace` alone starts the connection and then shows its status and the
/// initial state AeroSpace sends after `subscribe`.
#[test]
fn query_starts_the_connection() {
    let sb = Sandbox::new();
    let server = sb.server();
    let _d = sb.daemon();
    assert_eq!(server.subscribers(), 0, "connected before first use");

    let first = sb.query("aerospace");
    assert!(first.get("connected").is_some(), "{first}");
    let v = sb.wait_for("aerospace", "connected with state", |v| {
        v["connected"] == "on" && v["focused_workspace"] == "1" && v["mode"] == "main"
    });
    assert_eq!(v["transport"], "socket");
    assert_eq!(v["server_version"], SERVER_VERSION);
    assert_eq!(v["error"], "");
    server.wait_subscribed();
    assert_eq!(server.subscribers(), 1, "one subscription");

    server.push(r#"{"_event":"focused-monitor-changed","monitorId":2,"workspace":"5"}"#);
    let v = sb.wait_for("aerospace", "monitor event", |v| v["monitor"] == 2);
    assert_eq!(v["focused_workspace"], "5");
    assert_eq!(v["prev_workspace"], "1");
    // Asking again does not open a second subscription.
    sb.query("aerospace");
    thread::sleep(Duration::from_millis(100));
    assert_eq!(server.subscribers(), 1);
}

/// A shell config (the SketchyBar recipe without `exec-on-workspace-change`): the
/// subscribed script runs with `FOCUSED_WORKSPACE` / `PREV_WORKSPACE`, and
/// `provider=aerospace` labels follow the events.
#[test]
fn shell_config_scripts_and_provider() {
    let sb = Sandbox::new();
    let server = sb.server();
    let dir = sb.config_dir();
    let marker = sb.root.join("marker");
    let plugin = dir.join("plugin.sh");
    std::fs::write(
        &plugin,
        format!(
            "#!/bin/sh\necho \"$NAME $SENDER $FOCUSED_WORKSPACE $PREV_WORKSPACE\" >> '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&plugin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        dir.join("sketchybarrc"),
        "sketchybar --add event aerospace_workspace_change\n\
         sketchybar --add item space.ws left \\\n\
                    --subscribe space.ws aerospace_workspace_change \\\n\
                    --set space.ws script=\"$CONFIG_DIR/plugin.sh\" \\\n\
                    --add item ws right --set ws provider=aerospace \\\n\
                    --add item mode right --set mode provider=aerospace provider.args=mode\n",
    )
    .unwrap();
    let _d = sb.daemon();

    server.wait_subscribed();
    sb.wait_label("ws", "1");
    sb.wait_label("mode", "main");
    server.push(r#"{"_event":"focused-workspace-changed","prevWorkspace":"1","workspace":"2"}"#);
    wait_lines(&marker, |l| {
        l.iter()
            .any(|x| x == "space.ws aerospace_workspace_change 2 1")
    });
    sb.wait_label("ws", "2");
    server.push(r#"{"_event":"mode-changed","mode":"service"}"#);
    sb.wait_label("mode", "service");
    // A manual trigger (the old exec-on-workspace-change line) keeps working.
    let out = sb.mbar(&[
        "--trigger",
        "aerospace_workspace_change",
        "FOCUSED_WORKSPACE=9",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    wait_lines(&marker, |l| {
        l.iter()
            .any(|x| x.starts_with("space.ws aerospace_workspace_change 9"))
    });
}

/// A Lua config with `mbar.aerospace.on` (several handlers), `run` (the server records
/// the arguments) and `query` (JSON stdout decoded into a table; errors reported); the
/// handlers are registered again after `--reload`.
#[test]
fn lua_config_on_run_query_and_reload() {
    let sb = Sandbox::new();
    let server = sb.server();
    let dir = sb.config_dir();
    std::fs::write(
        dir.join("init.lua"),
        r#"
mbar.add("item", "ws", { label = "none" })
mbar.aerospace.on("workspace_change", function(env)
  mbar.set("ws", { label = env.FOCUSED_WORKSPACE .. "<" .. env.PREV_WORKSPACE })
end)
mbar.aerospace.on("aerospace_workspace_change", function(env)
  mbar.set("ws", { icon = "i" .. env.info.focused_workspace })
end)
mbar.add("item", "run", { label = "pending" })
mbar.aerospace.run({ "workspace", 3 })
mbar.aerospace.run({ "list-workspaces", "--all" }, function(r)
  mbar.set("run", { label = r.exit_code .. ":" .. (r.stdout:gsub("\n", ",")) })
end)
mbar.add("item", "query", { label = "pending" })
mbar.aerospace.query({ "list-windows", "--all", "--json" }, function(v, err)
  mbar.set("query", { label = err or (v[1]["app-name"] .. ":" .. v[1]["window-id"] .. ":" .. #v) })
end)
mbar.add("item", "qerr", { label = "pending" })
mbar.aerospace.query({ "fail" }, function(v, err)
  mbar.set("qerr", { label = tostring(v) .. ":" .. err })
end)
"#,
    )
    .unwrap();
    let _d = sb.daemon();

    sb.wait_label("run", "0:1,2,3,");
    sb.wait_label("query", "Finder:42:2");
    sb.wait_label("qerr", "nil:boom");
    let commands = server.commands();
    // The worker runs the commands in the order Lua issued them.
    assert_eq!(
        commands,
        vec![
            argv(&["workspace", "3"]),
            argv(&["list-workspaces", "--all"]),
            argv(&["list-windows", "--all", "--json"]),
            argv(&["fail"]),
        ]
    );

    // The carrier item exists, does not draw, and its handler gets the events.
    let carrier = sb.query("__mbar_aerospace");
    assert_eq!(carrier["geometry"]["drawing"], "off", "{carrier}");
    server.wait_subscribed();
    // The initial state (workspace 1) may or may not have reached the handlers yet.
    server.push(r#"{"_event":"focused-workspace-changed","prevWorkspace":"1","workspace":"2"}"#);
    let v = sb.wait_label("ws", "2<1");
    sb.wait_for("ws", "second handler", |v| v["icon"]["value"] == "i2");
    assert_eq!(label(&v), "2<1");

    // --reload: fresh items and Lua state; the connection stays, the handlers are back.
    let out = sb.mbar(&["--reload"]);
    assert!(out.status.success(), "{}", stderr(&out));
    sb.wait_label("ws", "none");
    sb.wait_label("run", "0:1,2,3,");
    assert_eq!(server.subscribers(), 1, "the connection survives --reload");
    server.push(r#"{"_event":"focused-workspace-changed","prevWorkspace":"2","workspace":"3"}"#);
    sb.wait_label("ws", "3<2");
    sb.wait_for("ws", "second handler after reload", |v| {
        v["icon"]["value"] == "i3"
    });
    let v = sb.query("aerospace");
    assert_eq!(v["connected"], "on");
    assert_eq!(v["focused_workspace"], "3");
}

/// A command while AeroSpace is not running reaches the callback as `exit_code = -1`.
#[test]
fn lua_run_without_server_reports_the_error() {
    let sb = Sandbox::new();
    let dir = sb.config_dir();
    std::fs::write(
        dir.join("init.lua"),
        r#"
mbar.add("item", "r", { label = "pending" })
mbar.aerospace.run({ "workspace", "1" }, function(r)
  mbar.set("r", { label = r.exit_code .. ":" .. (r.stderr ~= "" and "err" or "none") })
end)
"#,
    )
    .unwrap();
    let _d = sb.daemon();
    sb.wait_label("r", "-1:err");
    // The connection was requested and reports why it is down.
    let v = sb.wait_for("aerospace", "error status", |v| {
        v["connected"] == "off" && v["error"] != ""
    });
    assert_eq!(v["transport"], "none");
}

/// AeroSpace restarts: the status goes to disconnected, mbar reconnects by itself and
/// events flow again.
#[test]
fn reconnects_after_a_server_restart() {
    let sb = Sandbox::new();
    let mut server = sb.server();
    let _d = sb.daemon();
    sb.query("aerospace");
    sb.wait_for("aerospace", "connected", |v| v["connected"] == "on");
    server.wait_subscribed();

    server.stop();
    let v = sb.wait_for("aerospace", "disconnected", |v| v["connected"] == "off");
    assert_eq!(
        v["focused_workspace"], "1",
        "state is kept while disconnected"
    );

    let server = sb.server();
    sb.wait_for("aerospace", "reconnected", |v| {
        v["connected"] == "on" && v["server_version"] == SERVER_VERSION
    });
    server.wait_subscribed();
    server.push(r#"{"_event":"focused-workspace-changed","prevWorkspace":"1","workspace":"4"}"#);
    sb.wait_for("aerospace", "event after reconnect", |v| {
        v["focused_workspace"] == "4"
    });
}
