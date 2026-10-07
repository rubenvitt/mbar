//! Platform-independent daemon logic shared by every platform (headless now, macOS later):
//! owns the [`Runtime`], feeds it [`Event`]s, executes its [`Effect`]s and drives the Lua
//! engine.
//!
//! A platform's job is reduced to: run a main loop, post [`Event`]s from background
//! threads through a [`Post`], provide [`Resources`], call [`Driver::handle_event`] and
//! [`Driver::poll`] (at [`Driver::next_deadline`] at the latest), present the returned
//! [`FrameOutput`] and execute [`Driver::take_platform_requests`].
//!
//! Effects (`docs/ARCHITECTURE.md` "Data flow"):
//! * `Reply` → the pending IPC request's [`Responder`] (or a monitor subscription).
//! * `RunScript` → `sh -c` (D1 clean env, cwd config dir, 60 s timeout; reaper posts
//!   `Input::ScriptFinished`), or the Lua handler when the script is `lua:<id>`.
//! * `LuaCallback` → `LuaEngine::run_handler`.
//! * `RunConfig` → (re)load the config: shell config in a child, `init.lua` in a fresh
//!   `LuaEngine`.
//! * `Exit`, `Log`, `Monitor`, `Platform(SetHotload)` handled here; every other
//!   `PlatformRequest` goes to the platform.
//!
//! Lua re-entrancy (`docs/LUA.md`, `mbar_lua` crate docs): while the engine runs, the
//! engine is taken out of the driver; `Host::command` feeds the runtime synchronously and
//! every Lua call or config reload it causes is queued in `deferred` and executed by
//! [`Driver::drain`] after the engine call has returned.

use mbar_core::command::MonitorMode;
use mbar_core::platform::{Effect, FrameOutput, Input, PlatformRequest, ReplyToken, Resources};
use mbar_core::{Runtime, RuntimeConfig};
use mbar_lua::{Host, LuaEngine};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ipc::Responder;
use crate::logging::daemon_log;
use crate::scripts::{self, Spawn, SCRIPT_TIMEOUT};

/// Something that happened off the main thread.
#[derive(Debug)]
pub enum Event {
    /// Straight into `Runtime::handle` (OS events, script exits, provider samples, …).
    Input(Input),
    /// One IPC request (argv without argv[0]).
    Request {
        args: Vec<String>,
        responder: Responder,
    },
    /// A Lua `mbar.exec` child exited.
    LuaExecDone {
        generation: u64,
        id: u64,
        output: String,
    },
}

/// Posts an [`Event`] to the main loop (and wakes it). Called from any thread.
pub type Post = Arc<dyn Fn(Event) + Send + Sync>;

/// Static daemon setup.
#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub bar_name: String,
    pub home: String,
    /// Resolved config file (`None`: nothing found; the daemon runs without config).
    pub config_path: Option<PathBuf>,
    /// Daemon startup environment incl. `BAR_NAME` (D1 base of every child env).
    pub base_env: Vec<(OsString, OsString)>,
}

/// Work that must not run while the Lua engine is borrowed.
#[derive(Debug)]
enum Deferred {
    Handler {
        generation: u64,
        id: u64,
        env: Vec<(String, String)>,
    },
    ExecFinished {
        generation: u64,
        id: u64,
        output: String,
    },
    TimerFired {
        generation: u64,
        id: u64,
    },
    RunConfig(Option<String>),
}

struct MonitorSub {
    stream: UnixStream,
    mode: MonitorMode,
}

impl MonitorSub {
    fn wants(&self, stats: bool) -> bool {
        match self.mode {
            MonitorMode::All => true,
            MonitorMode::Events => !stats,
            MonitorMode::Stats => stats,
        }
    }
}

pub struct Driver {
    rt: Runtime,
    config_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    base_env: Vec<(OsString, OsString)>,
    post: Post,
    hotload: Arc<AtomicBool>,
    next_token: u64,
    pending: HashMap<ReplyToken, Responder>,
    pending_monitors: HashMap<ReplyToken, (Responder, MonitorMode)>,
    monitors: Vec<MonitorSub>,
    lua: Option<LuaEngine>,
    /// Bumped on every config (re)load; stale Lua exec/timer callbacks are dropped.
    lua_generation: u64,
    lua_timers: Vec<(Instant, u64, u64)>,
    deferred: VecDeque<Deferred>,
    platform_requests: Vec<PlatformRequest>,
    exit: bool,
}

impl Driver {
    pub fn new(cfg: DriverConfig, post: Post, hotload: Arc<AtomicBool>) -> Driver {
        let rt = Runtime::new(RuntimeConfig {
            bar_name: cfg.bar_name.clone(),
            home: cfg.home.clone(),
            config_path: cfg
                .config_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        });
        Driver {
            rt,
            config_dir: None,
            config_path: cfg.config_path,
            base_env: cfg.base_env,
            post,
            hotload,
            next_token: 1,
            pending: HashMap::new(),
            pending_monitors: HashMap::new(),
            monitors: Vec::new(),
            lua: None,
            lua_generation: 0,
            lua_timers: Vec::new(),
            deferred: VecDeque::new(),
            platform_requests: Vec::new(),
            exit: false,
        }
    }

    /// Read access for platforms (e.g. bar geometry for hit testing on macOS).
    #[allow(dead_code)]
    pub fn runtime(&self) -> &Runtime {
        &self.rt
    }

    /// `bar_manager_begin`, then the first config run. Call once the IPC server is up so
    /// the shell config's commands reach the daemon.
    pub fn start(&mut self, res: &mut dyn Resources) {
        let fx = self.rt.begin(res);
        self.apply(fx);
        self.deferred.push_back(Deferred::RunConfig(None));
        self.drain(res);
    }

    /// `--exit` was executed: the platform should leave its loop.
    pub fn exit_requested(&self) -> bool {
        self.exit
    }

    /// Platform requests other than hotload, in order.
    pub fn take_platform_requests(&mut self) -> Vec<PlatformRequest> {
        std::mem::take(&mut self.platform_requests)
    }

    pub fn handle_event(&mut self, ev: Event, res: &mut dyn Resources) {
        match ev {
            Event::Input(input) => {
                let fx = self.rt.handle(input, res);
                self.apply(fx);
            }
            Event::Request { args, responder } => self.request(args, responder, res),
            Event::LuaExecDone {
                generation,
                id,
                output,
            } => self.deferred.push_back(Deferred::ExecFinished {
                generation,
                id,
                output,
            }),
        }
        self.drain(res);
    }

    /// Fires due timers (runtime, Lua, monitor stats), runs queued work and renders when
    /// needed. Returns the frame for the platform to present.
    pub fn poll(&mut self, res: &mut dyn Resources) -> Option<FrameOutput> {
        let now = res.now();
        if !self.exit && self.rt.next_deadline().is_some_and(|d| d <= now) {
            let fx = self.rt.handle(Input::Timer, res);
            self.apply(fx);
        }
        if !self.lua_timers.is_empty() {
            let mut due: Vec<_> = Vec::new();
            self.lua_timers.retain(|t| {
                if t.0 <= now {
                    due.push(*t);
                    false
                } else {
                    true
                }
            });
            due.sort_by_key(|t| t.0);
            for (_, generation, id) in due {
                self.deferred
                    .push_back(Deferred::TimerFired { generation, id });
            }
        }
        self.drain(res);
        if !self.exit && self.rt.needs_frame() {
            Some(self.rt.frame(res.now(), res))
        } else {
            None
        }
    }

    /// When [`Driver::poll`] must run next (`None`: only on events).
    pub fn next_deadline(&self) -> Option<Instant> {
        let lua = self.lua_timers.iter().map(|t| t.0).min();
        [self.rt.next_deadline(), lua].into_iter().flatten().min()
    }

    // ------------------------------------------------------------------ requests

    fn token(&mut self) -> ReplyToken {
        let t = ReplyToken(self.next_token);
        self.next_token += 1;
        t
    }

    fn request(&mut self, args: Vec<String>, responder: Responder, res: &mut dyn Resources) {
        if self.exit {
            return;
        }
        let token = self.token();
        match monitor_mode(&args) {
            Some(mode) => {
                self.pending_monitors.insert(token, (responder, mode));
            }
            None => {
                self.pending.insert(token, responder);
            }
        }
        let fx = self.rt.handle(Input::Message { args, reply: token }, res);
        self.apply(fx);
        // The runtime does not reply to an accepted `--monitor` (the connection stays
        // open); an error reply has already been delivered by `apply`.
        if let Some((r, mode)) = self.pending_monitors.remove(&token) {
            self.start_monitor(r, mode, String::new());
        }
    }

    /// Runs a message synchronously and returns its reply (Lua `Host::command`, stats).
    fn command_sync(&mut self, args: Vec<String>, res: &mut dyn Resources) -> String {
        if self.exit {
            return String::new();
        }
        let token = self.token();
        let fx = self.rt.handle(Input::Message { args, reply: token }, res);
        let mut response = String::new();
        let mut rest = Vec::with_capacity(fx.len());
        for e in fx {
            match e {
                Effect::Reply { reply, text } if reply == token => response = text,
                e => rest.push(e),
            }
        }
        self.apply(rest);
        response
    }

    fn reply(&mut self, token: ReplyToken, text: String) {
        if let Some(r) = self.pending.remove(&token) {
            r.respond(&text);
        } else if let Some((r, mode)) = self.pending_monitors.remove(&token) {
            self.start_monitor(r, mode, text);
        } else {
            log::debug!("reply for unknown token {token:?}");
        }
    }

    // ------------------------------------------------------------------ effects

    fn apply(&mut self, effects: Vec<Effect>) {
        for e in effects {
            match e {
                Effect::Reply { reply, text } => self.reply(reply, text),
                Effect::RunScript { script, env, item } => match mbar_lua::parse_script(&script) {
                    Some(id) => self.deferred.push_back(Deferred::Handler {
                        generation: self.lua_generation,
                        id,
                        env,
                    }),
                    None => self.spawn_script(script, env, item),
                },
                Effect::Exit => self.exit = true,
                Effect::RunConfig { path } => self.deferred.push_back(Deferred::RunConfig(path)),
                Effect::Platform(PlatformRequest::SetHotload(on)) => {
                    self.hotload.store(on, Ordering::Relaxed);
                }
                Effect::Platform(req) => self.platform_requests.push(req),
                Effect::LuaCallback { handler, env } => {
                    self.deferred.push_back(Deferred::Handler {
                        generation: self.lua_generation,
                        id: handler,
                        env,
                    })
                }
                Effect::Log(msg) => daemon_log(&msg),
                Effect::Monitor(line) => {
                    let line = compact_json(&line);
                    self.broadcast(&line, is_stats_line(&line));
                }
            }
        }
    }

    /// Executes queued Lua calls and config runs until nothing is left.
    pub fn drain(&mut self, res: &mut dyn Resources) {
        while let Some(d) = self.deferred.pop_front() {
            if self.exit {
                self.deferred.clear();
                return;
            }
            match d {
                Deferred::Handler {
                    generation,
                    id,
                    env,
                } if generation == self.lua_generation => {
                    self.with_lua(res, |e, h| e.run_handler(id, &env, h));
                }
                Deferred::ExecFinished {
                    generation,
                    id,
                    output,
                } if generation == self.lua_generation => {
                    self.with_lua(res, |e, h| e.exec_finished(id, output, h));
                }
                Deferred::TimerFired { generation, id } if generation == self.lua_generation => {
                    self.with_lua(res, |e, h| e.timer_fired(id, h));
                }
                Deferred::RunConfig(path) => self.run_config(path, res),
                stale => log::debug!("dropping stale lua work {stale:?}"),
            }
        }
    }

    fn spawn_script(&mut self, script: String, env: Vec<(String, String)>, item: Option<String>) {
        let post = self.post.clone();
        let item2 = item.clone();
        let spec = Spawn {
            command: script,
            env,
            cwd: self.config_dir.clone(),
            capture: false,
        };
        let r = scripts::spawn(spec, &self.base_env, SCRIPT_TIMEOUT, move |pid, output| {
            post(Event::Input(Input::ScriptFinished {
                pid,
                item: item2,
                output,
            }))
        });
        if let Err(e) = r {
            log::warn!("failed to spawn script of {item:?}: {e}");
        }
    }

    // ------------------------------------------------------------------ config

    /// `exec_config_file` (`cli.md` §10.2). `path` replaces the stored config path.
    fn run_config(&mut self, path: Option<String>, res: &mut dyn Resources) {
        if let Some(p) = path {
            let p = std::fs::canonicalize(&p).unwrap_or_else(|_| PathBuf::from(p));
            self.rt.config.config_path = Some(p.to_string_lossy().into_owned());
            self.config_path = Some(p);
        }
        // Fresh Lua state on every (re)load; pending exec/timer callbacks become stale.
        self.lua = None;
        self.lua_generation += 1;
        self.lua_timers.clear();

        let Some(path) = self.config_path.clone() else {
            daemon_log("could not locate config file..");
            return;
        };
        if !path.is_file() {
            daemon_log(&format!("file '{}' does not exist..", path.display()));
            return;
        }
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        self.set_config_dir(&dir);

        if is_lua_config(&path) {
            match LuaEngine::new() {
                Ok(engine) => {
                    self.lua = Some(engine);
                    self.with_lua(res, |e, h| e.load_file(&path, h));
                }
                Err(e) => daemon_log(&format!("lua: cannot create state: {e}")),
            }
        } else {
            ensure_executable(&path);
            let post = self.post.clone();
            let spec = Spawn {
                command: scripts::shell_quote(&path.to_string_lossy()),
                env: Vec::new(),
                cwd: Some(dir),
                capture: false,
            };
            let r = scripts::spawn(spec, &self.base_env, SCRIPT_TIMEOUT, move |pid, _| {
                post(Event::Input(Input::ScriptFinished {
                    pid,
                    item: None,
                    output: None,
                }))
            });
            if r.is_err() {
                daemon_log(&format!("failed to execute file '{}'", path.display()));
            }
        }
    }

    /// `CONFIG_DIR` for every later child and the daemon's cwd (relative paths, `cli.md`
    /// §4.5.1).
    fn set_config_dir(&mut self, dir: &Path) {
        let key = OsString::from("CONFIG_DIR");
        self.base_env.retain(|(k, _)| *k != key);
        self.base_env.push((key, dir.as_os_str().to_owned()));
        if let Err(e) = std::env::set_current_dir(dir) {
            log::warn!("cannot chdir to {}: {e}", dir.display());
        }
        self.config_dir = Some(dir.to_path_buf());
    }

    // ------------------------------------------------------------------ Lua

    fn with_lua(
        &mut self,
        res: &mut dyn Resources,
        f: impl FnOnce(&mut LuaEngine, &mut dyn Host) -> mbar_lua::Result<()>,
    ) {
        let Some(mut engine) = self.lua.take() else {
            return;
        };
        let generation = self.lua_generation;
        let result = {
            let mut host = LuaHost { d: self, res };
            f(&mut engine, &mut host)
        };
        if let Err(e) = result {
            daemon_log(&format!("lua: {e}"));
        }
        if self.lua_generation == generation && self.lua.is_none() {
            self.lua = Some(engine);
        }
    }

    fn spawn_lua_exec(&mut self, cmd: String, callback: Option<u64>) {
        let post = self.post.clone();
        let generation = self.lua_generation;
        let spec = Spawn {
            command: cmd,
            env: Vec::new(),
            cwd: self.config_dir.clone(),
            capture: callback.is_some(),
        };
        let r = scripts::spawn(spec, &self.base_env, SCRIPT_TIMEOUT, move |_, output| {
            if let Some(id) = callback {
                post(Event::LuaExecDone {
                    generation,
                    id,
                    output: output.unwrap_or_default(),
                });
            }
        });
        if let Err(e) = r {
            log::warn!("lua: exec failed: {e}");
        }
    }

    // ------------------------------------------------------------------ --monitor

    fn start_monitor(&mut self, responder: Responder, mode: MonitorMode, text: String) {
        if mbar_ipc::is_error_response(&text) {
            responder.respond(&text);
            return;
        }
        match responder.into_stream() {
            Ok(mut stream) => {
                // The (usually empty) reply is the first frame: it acknowledges the
                // subscription and fixes the framed stream format for the reader.
                if mbar_ipc::socket::write_frame(&mut stream, text.as_bytes()).is_err() {
                    return;
                }
                let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                self.monitors.push(MonitorSub { stream, mode });
            }
            Err(r) => r.respond("[!] Monitor: only available over the Unix socket\n"),
        }
    }

    fn broadcast(&mut self, line: &str, stats: bool) {
        if self.monitors.is_empty() {
            return;
        }
        let mut frame = line.to_string();
        if !frame.ends_with('\n') {
            frame.push('\n');
        }
        self.monitors.retain_mut(|m| {
            !m.wants(stats)
                || mbar_ipc::socket::write_frame(&mut m.stream, frame.as_bytes()).is_ok()
        });
    }
}

/// `mbar_lua::Host` backed by the driver (the engine itself is taken out while it runs).
struct LuaHost<'a, 'r> {
    d: &'a mut Driver,
    res: &'a mut (dyn Resources + 'r),
}

impl Host for LuaHost<'_, '_> {
    fn command(&mut self, args: Vec<String>) -> String {
        self.d.command_sync(args, self.res)
    }

    fn spawn_shell(&mut self, cmd: String, callback: Option<u64>) {
        self.d.spawn_lua_exec(cmd, callback);
    }

    fn schedule(&mut self, delay: Duration, callback: u64) {
        let at = self.res.now() + delay;
        self.d
            .lua_timers
            .push((at, self.d.lua_generation, callback));
    }
}

/// `--monitor [events|stats|all]` anywhere in the message (default `all`).
fn monitor_mode(args: &[String]) -> Option<MonitorMode> {
    let i = args.iter().position(|a| a == "--monitor")?;
    Some(match args.get(i + 1).map(String::as_str) {
        Some("events") => MonitorMode::Events,
        Some("stats") => MonitorMode::Stats,
        _ => MonitorMode::All,
    })
}

fn is_lua_config(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "lua")
}

/// `chmod(mode | S_IXUSR)` (`cli.md` §10.2 step 4).
fn ensure_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(md) = std::fs::metadata(path) else {
        return;
    };
    let mut perm = md.permissions();
    if perm.mode() & 0o100 == 0 {
        perm.set_mode(perm.mode() | 0o100);
        if std::fs::set_permissions(path, perm).is_err() {
            daemon_log(&format!(
                "could not set the executable permission bit for '{}'",
                path.display()
            ));
        }
    }
}

/// Removes whitespace outside JSON strings (one line per monitor message).
fn compact_json(s: &str) -> String {
    let s = s.trim();
    if !(s.starts_with('{') || s.starts_with('[')) {
        return s.replace('\n', " ");
    }
    let mut out = String::with_capacity(s.len());
    let (mut in_str, mut esc) = (false, false);
    for c in s.chars() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    out
}

fn is_stats_line(line: &str) -> bool {
    line.contains("\"type\":\"stats\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn monitor_modes() {
        assert_eq!(monitor_mode(&s(&["--query", "bar"])), None);
        assert_eq!(monitor_mode(&s(&["--monitor"])), Some(MonitorMode::All));
        assert_eq!(
            monitor_mode(&s(&["--monitor", "events"])),
            Some(MonitorMode::Events)
        );
        assert_eq!(
            monitor_mode(&s(&["--monitor", "stats"])),
            Some(MonitorMode::Stats)
        );
        assert_eq!(
            monitor_mode(&s(&["--monitor", "all"])),
            Some(MonitorMode::All)
        );
    }

    #[test]
    fn json_compaction() {
        assert_eq!(
            compact_json("{\n\t\"a b\": \"x\\\" y\",\n\t\"c\": [1, 2]\n}\n"),
            "{\"a b\":\"x\\\" y\",\"c\":[1,2]}"
        );
        assert!(is_stats_line("{\"type\":\"stats\",\"frames\":3}"));
        assert!(!is_stats_line("{\"type\":\"event\",\"name\":\"x\"}"));
    }

    #[test]
    fn lua_config_detection() {
        assert!(is_lua_config(Path::new("/a/init.lua")));
        assert!(!is_lua_config(Path::new("/a/mbarrc")));
    }
}
