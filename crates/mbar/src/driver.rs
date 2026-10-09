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
//! * `Reply` → the pending IPC request's [`Responder`]; `MonitorStart` → a `--monitor`
//!   subscription on that request's connection.
//! * `RunScript` → `sh -c` (D1 clean env, cwd config dir, 60 s timeout; reaper posts
//!   `Input::ScriptFinished`), or the Lua handler when the script is `lua:<id>`.
//! * `LuaCallback` → `LuaEngine::run_handler`.
//! * `RunConfig` → (re)load the config: shell config in a child (bash when it has no
//!   shebang), `init.lua` in a fresh `LuaEngine`; then, for the default bar,
//!   JankyBorders' `bordersrc` like a shell config (borders design §3).
//! * `Exit`, `Log`, `Monitor`, `Platform(SetHotload)` handled here; every other
//!   `PlatformRequest` goes to the platform.
//! * `Platform(StartAerospace)` handled here on every platform (aerospace design
//!   §Binary): one `mbar_aerospace::subscribe` whose callbacks post `Input::Aerospace` /
//!   `Input::AerospaceStatus`; it lives until the driver exits. Lua `mbar.aerospace.run`
//!   / `query` commands run in order on one worker thread ([`AerospaceWorker`]) and come
//!   back as [`Event::LuaAerospaceDone`].
//!
//! Lua re-entrancy (`docs/LUA.md`, `mbar_lua` crate docs): while the engine runs, the
//! engine is taken out of the driver; `Host::command` feeds the runtime synchronously and
//! every Lua call or config reload it causes is queued in `deferred` and executed by
//! [`Driver::drain`] after the engine call has returned. A drain is bounded (the work queued
//! before it started, then at most [`DRAIN_BUDGET`]), so Lua handlers that keep triggering
//! each other cannot starve IPC, timers and frames; the rest runs on the next loop iteration
//! ([`Driver::next_deadline`] is "now" while work is queued).

use mbar_core::command::MonitorMode;
use mbar_core::platform::{Effect, FrameOutput, Input, PlatformRequest, ReplyToken, Resources};
use mbar_core::{Runtime, RuntimeConfig};
use mbar_lua::{AerospaceResult, Host, LuaEngine};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ipc::{encode_frame, Responder};
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
    /// A Lua `mbar.aerospace.run` / `query` command with a callback finished.
    LuaAerospaceDone {
        generation: u64,
        id: u64,
        result: AerospaceResult,
    },
    /// `SIGTERM`/`SIGINT`/`SIGHUP` (`crate::signals`): shut down like `--exit`.
    Terminate { signal: i32 },
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
    AerospaceFinished {
        generation: u64,
        id: u64,
        result: AerospaceResult,
    },
    RunConfig(Option<String>),
}

/// Wall-clock time one [`Driver::drain`] may keep running work queued during the drain.
const DRAIN_BUDGET: Duration = Duration::from_millis(5);

/// Frames a `--monitor` subscriber may lag behind before it is dropped.
const MONITOR_QUEUE: usize = 1024;
/// A subscriber whose socket accepts nothing for this long is dropped.
const MONITOR_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// One `--monitor` connection. Its socket is written by its own thread
/// ([`monitor_writer`]), so a subscriber that stops reading never blocks the main loop.
struct MonitorSub {
    tx: SyncSender<Arc<[u8]>>,
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
    /// IPC bar name; marks blocking `io.popen`/`os.execute` children of Lua
    /// (`mbar_lua::SYNC_SHELL_ENV`).
    bar_name: String,
    config_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    base_env: Vec<(OsString, OsString)>,
    post: Post,
    hotload: Arc<AtomicBool>,
    next_token: u64,
    pending: HashMap<ReplyToken, Responder>,
    monitors: Vec<MonitorSub>,
    lua: Option<LuaEngine>,
    /// Bumped on every config (re)load; stale Lua exec/timer callbacks are dropped.
    lua_generation: u64,
    lua_timers: Vec<(Instant, u64, u64)>,
    deferred: VecDeque<Deferred>,
    platform_requests: Vec<PlatformRequest>,
    exit: bool,
    /// Latest `Resources::now` seen (deadline for queued work).
    last_now: Option<Instant>,
    /// The AeroSpace event stream, started by the first `PlatformRequest::StartAerospace`
    /// (kept across `--reload`, stopped on exit).
    aerospace: Option<mbar_aerospace::Subscription>,
    /// Runs Lua `mbar.aerospace` commands (started on first use).
    aerospace_worker: Option<AerospaceWorker>,
    /// `Input::Timer`s handed to the runtime (tests).
    #[cfg(test)]
    timer_inputs: u64,
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
            bar_name: cfg.bar_name,
            config_dir: None,
            config_path: cfg.config_path,
            base_env: cfg.base_env,
            post,
            hotload,
            next_token: 1,
            pending: HashMap::new(),
            monitors: Vec::new(),
            lua: None,
            lua_generation: 0,
            lua_timers: Vec::new(),
            deferred: VecDeque::new(),
            platform_requests: Vec::new(),
            exit: false,
            last_now: None,
            aerospace: None,
            aerospace_worker: None,
            #[cfg(test)]
            timer_inputs: 0,
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
        self.last_now = Some(res.now());
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
        self.last_now = Some(res.now());
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
            Event::LuaAerospaceDone {
                generation,
                id,
                result,
            } => self.deferred.push_back(Deferred::AerospaceFinished {
                generation,
                id,
                result,
            }),
            Event::Terminate { signal } => self.terminate(signal),
        }
        self.drain(res);
    }

    /// A termination signal: the `--exit` path of the runtime (mach helpers get `"k"`),
    /// then the platform leaves its loop and `platform_main` cleans up.
    fn terminate(&mut self, signal: i32) {
        if self.exit {
            return;
        }
        daemon_log(&format!(
            "received {}, shutting down",
            crate::signals::name(signal)
        ));
        let fx = self.rt.exit();
        self.apply(fx);
        // Should the runtime have exited already, the signal still ends the loop.
        self.exit = true;
        self.stop_aerospace();
    }

    /// Fires due timers (runtime, Lua), runs queued work and renders when
    /// needed. Returns the frame for the platform to present.
    pub fn poll(&mut self, res: &mut dyn Resources) -> Option<FrameOutput> {
        let now = res.now();
        self.last_now = Some(now);
        if !self.exit {
            // Render and animation deadlines are served by `frame` below; `Input::Timer`
            // only when the routine clock or the wake re-post is due.
            if self.rt.timer_due(now) {
                #[cfg(test)]
                {
                    self.timer_inputs += 1;
                }
                let fx = self.rt.handle(Input::Timer, res);
                self.apply(fx);
            } else if self.rt.animating() && self.rt.needs_frame() {
                // An animation frame is an event of its own in SketchyBar, which polls the
                // active display first (`events.md` §5.3 Q5).
                let fx = self.rt.poll_display(res);
                self.apply(fx);
            }
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

    /// When [`Driver::poll`] must run next (`None`: only on events). "Now" while Lua work
    /// is left over from a bounded [`Driver::drain`].
    /// Used by display-link platforms (macOS); headless uses [`Driver::next_deadline_paced`].
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadline(self.rt.next_deadline())
    }

    /// [`Driver::next_deadline`] for a platform without a display link (headless): running
    /// animations ask for the next frame one `frame_interval` after the last one instead of
    /// "now" ([`mbar_core::runtime::Runtime::next_deadline_paced`]), so the loop sleeps
    /// between animation frames rather than spinning.
    pub fn next_deadline_paced(&self, frame_interval: Duration) -> Option<Instant> {
        self.deadline(self.rt.next_deadline_paced(frame_interval))
    }

    fn deadline(&self, rt: Option<Instant>) -> Option<Instant> {
        if !self.deferred.is_empty() {
            return Some(self.last_now.unwrap_or_else(Instant::now));
        }
        let lua = self.lua_timers.iter().map(|t| t.0).min();
        [rt, lua].into_iter().flatten().min()
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
        self.pending.insert(token, responder);
        // The reply arrives as `Effect::Reply`, or as `Effect::MonitorStart` when the
        // runtime executed `--monitor` (the connection then stays open).
        let fx = self.rt.handle(Input::Message { args, reply: token }, res);
        self.apply(fx);
    }

    /// Runs a message synchronously and returns its reply (Lua `Host::command`).
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
                Effect::Exit => {
                    self.exit = true;
                    self.stop_aerospace();
                }
                Effect::RunConfig { path } => self.deferred.push_back(Deferred::RunConfig(path)),
                Effect::Platform(PlatformRequest::SetHotload(on)) => {
                    self.hotload.store(on, Ordering::Relaxed);
                }
                Effect::Platform(PlatformRequest::StartAerospace) => self.start_aerospace(),
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
                Effect::MonitorStart { reply, mode, text } => {
                    match self.pending.remove(&reply) {
                        Some(r) => self.start_monitor(r, mode, text),
                        // `--monitor` from Lua (`Host::command`): nothing to stream to.
                        None => log::debug!("--monitor without a connection ({reply:?})"),
                    }
                    self.sync_monitor_flags();
                }
            }
        }
    }

    /// Executes queued Lua calls and config runs: everything queued before the call, then
    /// work queued meanwhile until [`DRAIN_BUDGET`] is used up. Leftovers run on the next
    /// call (see [`Driver::next_deadline`]), so self-triggering handlers cannot freeze the
    /// main loop.
    pub fn drain(&mut self, res: &mut dyn Resources) {
        let batch = self.deferred.len();
        let start = Instant::now();
        let mut done = 0usize;
        while let Some(d) = self.deferred.pop_front() {
            if self.exit {
                self.deferred.clear();
                return;
            }
            done += 1;
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
                Deferred::AerospaceFinished {
                    generation,
                    id,
                    result,
                } if generation == self.lua_generation => {
                    self.with_lua(res, |e, h| e.aerospace_finished(id, result, h));
                }
                Deferred::RunConfig(path) => self.run_config(path, res),
                stale => log::debug!("dropping stale lua work {stale:?}"),
            }
            if done >= batch && start.elapsed() >= DRAIN_BUDGET {
                break;
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

    /// `exec_config_file` (`cli.md` §10.2), then JankyBorders' `bordersrc` (borders design
    /// §3). `path` replaces the stored config path.
    fn run_config(&mut self, path: Option<String>, res: &mut dyn Resources) {
        self.run_main_config(path, res);
        self.run_bordersrc();
    }

    /// Runs `bordersrc` when this is the default bar and one exists, in addition to (and
    /// started after) the main config, whether or not that one exists. Like a shell
    /// config: made executable, `sh -c` with the quoted path, the daemon's environment
    /// (the bundle's `bin` first on `PATH`, so its `borders …` lines reach this daemon
    /// through the link), killed after 60 s. A shell config runs concurrently with it.
    /// Skipped (and logged) while JankyBorders itself runs, see [`bordersrc_spawn`].
    fn run_bordersrc(&mut self) {
        let found = mbar_app::config::find_bordersrc(&self.rt.config.home);
        let foreign = || mbar_ipc::service_registered(JANKYBORDERS_SERVICE);
        let Some((path, spec)) = bordersrc_spawn(&self.bar_name, found, foreign) else {
            return;
        };
        ensure_executable(&path);
        // Not a runtime script either (see `run_main_config`).
        let r = scripts::spawn(spec, &self.base_env, SCRIPT_TIMEOUT, |pid, _| {
            log::debug!("bordersrc (pid {pid}) exited");
        });
        if r.is_err() {
            daemon_log(&format!("failed to execute file '{}'", path.display()));
        }
    }

    /// The main config (`exec_config_file`, `cli.md` §10.2).
    fn run_main_config(&mut self, path: Option<String>, res: &mut dyn Resources) {
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
                Ok(mut engine) => {
                    engine.set_bar_name(Some(&self.bar_name));
                    self.lua = Some(engine);
                    self.with_lua(res, |e, h| e.load_file(&path, h));
                }
                Err(e) => daemon_log(&format!("lua: cannot create state: {e}")),
            }
        } else {
            ensure_executable(&path);
            let spec = Spawn {
                command: config_command(&path),
                env: Vec::new(),
                cwd: Some(dir),
                capture: false,
            };
            // The runtime never counted the config as a script (`note_script_spawn`), so its
            // exit is not reported (it would skew `scripts.running` in `--query stats`).
            let r = scripts::spawn(spec, &self.base_env, SCRIPT_TIMEOUT, |pid, _| {
                log::debug!("config (pid {pid}) exited");
            });
            if r.is_err() {
                daemon_log(&format!("failed to execute file '{}'", path.display()));
            }
        }
    }

    /// `CONFIG_DIR` for every later child and the daemon's own environment (`cli.md` §10.2
    /// step 2: Lua's `os.getenv`/`io.popen`/`os.execute` see it), and the daemon's cwd
    /// (relative paths, `cli.md` §4.5.1).
    fn set_config_dir(&mut self, dir: &Path) {
        // Normally already set by `daemon::run` before any thread existed; only a reload
        // with a config in another directory changes it here.
        if std::env::var_os("CONFIG_DIR").as_deref() != Some(dir.as_os_str()) {
            std::env::set_var("CONFIG_DIR", dir);
        }
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
        let before = engine.stats();
        let result = {
            let mut host = LuaHost { d: self, res };
            f(&mut engine, &mut host)
        };
        let after = engine.stats();
        if after.callbacks > before.callbacks {
            // Handlers, `mbar.exec` and `mbar.delay` callbacks (`lua` in `--query stats`).
            self.rt.record_lua_callbacks(
                after.callbacks - before.callbacks,
                after.total_us.saturating_sub(before.total_us),
                after.max_us,
            );
        }
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

    // ------------------------------------------------------------------ AeroSpace

    /// `PlatformRequest::StartAerospace`: subscribes to AeroSpace's event stream (once;
    /// the runtime emits the request once, a repeated one is ignored). Events and status
    /// changes go through the main loop like every other input.
    fn start_aerospace(&mut self) {
        if self.aerospace.is_some() || self.exit {
            return;
        }
        let (on_event, on_status) = (self.post.clone(), self.post.clone());
        self.aerospace = Some(mbar_aerospace::subscribe(
            move |ev| on_event(Event::Input(Input::Aerospace(ev))),
            move |status| on_status(Event::Input(Input::AerospaceStatus(status))),
        ));
    }

    /// Stops the event stream and the command worker (exit).
    fn stop_aerospace(&mut self) {
        self.aerospace = None;
        self.aerospace_worker = None;
    }

    /// Lua `mbar.aerospace.run` / `query`: makes sure the connection is started
    /// (`Runtime::request_aerospace`), then queues the command on the worker. With a
    /// callback, the result comes back as [`Event::LuaAerospaceDone`] for this config
    /// generation.
    fn lua_aerospace(&mut self, args: Vec<String>, callback: Option<u64>) {
        if self.exit {
            return;
        }
        if let Some(fx) = self.rt.request_aerospace() {
            self.apply(vec![fx]);
        }
        if self.aerospace_worker.is_none() {
            self.aerospace_worker = AerospaceWorker::spawn(self.post.clone());
        }
        let job = AerospaceJob {
            args,
            reply: callback.map(|id| (self.lua_generation, id)),
        };
        let sent = match &self.aerospace_worker {
            Some(w) => w.tx.send(job).map_err(|e| e.0),
            None => Err(job),
        };
        if let Err(job) = sent {
            // No worker thread: report the failure like a transport error.
            self.aerospace_worker = None;
            if let Some((generation, id)) = job.reply {
                (self.post)(Event::LuaAerospaceDone {
                    generation,
                    id,
                    result: AerospaceResult {
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: "cannot start the AeroSpace command thread".into(),
                    },
                });
            }
        }
    }

    // ------------------------------------------------------------------ --monitor
    //
    // Contract with mbar-ui (`StreamDecoder`): the connection stays open after the request;
    // the daemon writes length-prefixed frames (same framing as replies), each holding
    // newline-terminated JSON lines (`docs/EXTENSIONS.md`). The first frame is the reply
    // (the output of the message, empty for a plain `--monitor`; `[!] …` = error, then the
    // connection closes). Event and stats lines come from the runtime (`Effect::Monitor`).
    // Each subscriber has a writer thread fed through a bounded queue: the main loop never
    // blocks on a socket. Subscribers that are gone, stalled (nothing accepted for 1 s) or
    // more than `MONITOR_QUEUE` frames behind are dropped.

    fn start_monitor(&mut self, responder: Responder, mode: MonitorMode, text: String) {
        if mbar_ipc::is_error_response(&text) {
            responder.respond(&text);
            return;
        }
        match responder.into_stream() {
            Ok(stream) => {
                let (tx, rx) = mpsc::sync_channel::<Arc<[u8]>>(MONITOR_QUEUE);
                // The reply is the first frame: it acknowledges the subscription and fixes
                // the framed stream format for the reader.
                let _ = tx.try_send(encode_frame(text.as_bytes()).into());
                let spawned = std::thread::Builder::new()
                    .name("mbar-monitor".into())
                    .spawn(move || monitor_writer(stream, rx));
                match spawned {
                    Ok(_) => self.monitors.push(MonitorSub { tx, mode }),
                    Err(e) => log::warn!("monitor: cannot spawn writer: {e}"),
                }
            }
            Err(r) => r.respond("[!] Monitor: only available over the Unix socket\n"),
        }
    }

    /// Tells the runtime which monitor streams still have subscribers.
    fn sync_monitor_flags(&mut self) {
        let events = self.monitors.iter().any(|m| m.wants(false));
        let stats = self.monitors.iter().any(|m| m.wants(true));
        self.rt.set_monitor(events, stats);
    }

    fn broadcast(&mut self, line: &str, stats: bool) {
        if self.monitors.is_empty() {
            return;
        }
        let mut text = line.to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let frame: Arc<[u8]> = encode_frame(text.as_bytes()).into();
        let before = self.monitors.len();
        self.monitors.retain(|m| {
            if !m.wants(stats) {
                return true;
            }
            match m.tx.try_send(frame.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    log::debug!("monitor: dropping a subscriber that does not keep up");
                    false
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
        if self.monitors.len() != before {
            // Some subscribers went away: stop producing lines nobody reads.
            self.sync_monitor_flags();
        }
    }
}

/// Writes queued frames to one `--monitor` connection until the driver drops the
/// subscription or a write fails (client gone, or nothing accepted for
/// [`MONITOR_WRITE_TIMEOUT`]).
fn monitor_writer(mut stream: UnixStream, rx: Receiver<Arc<[u8]>>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(MONITOR_WRITE_TIMEOUT));
    while let Ok(frame) = rx.recv() {
        if stream.write_all(&frame).is_err() {
            return;
        }
    }
}

/// One Lua `mbar.aerospace` command; `reply` is `(generation, callback id)`.
struct AerospaceJob {
    args: Vec<String>,
    reply: Option<(u64, u64)>,
}

/// The thread that runs Lua `mbar.aerospace` commands (blocking socket / CLI calls, up to
/// `mbar_aerospace::ANSWER_TIMEOUT` each) one after another, so the main loop never waits
/// for AeroSpace and commands reach it in the order Lua issued them. Ends when the
/// driver drops the sender.
struct AerospaceWorker {
    tx: mpsc::Sender<AerospaceJob>,
}

impl AerospaceWorker {
    fn spawn(post: Post) -> Option<AerospaceWorker> {
        let (tx, rx) = mpsc::channel::<AerospaceJob>();
        let spawned = std::thread::Builder::new()
            .name("mbar-aerospace-run".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let result = aerospace_result(mbar_aerospace::run(&job.args));
                    match job.reply {
                        Some((generation, id)) => post(Event::LuaAerospaceDone {
                            generation,
                            id,
                            result,
                        }),
                        None if result.exit_code != 0 => log::warn!(
                            "lua: aerospace {:?} failed ({}): {}",
                            job.args,
                            result.exit_code,
                            result.stderr.trim()
                        ),
                        None => {}
                    }
                }
            });
        match spawned {
            Ok(_) => Some(AerospaceWorker { tx }),
            Err(e) => {
                log::warn!("lua: cannot spawn the AeroSpace command thread: {e}");
                None
            }
        }
    }
}

/// A command's answer as the Lua callback sees it; a transport error is `exit_code = -1`
/// with the error text in `stderr`.
fn aerospace_result(r: Result<mbar_aerospace::Answer, mbar_aerospace::Error>) -> AerospaceResult {
    match r {
        Ok(a) => AerospaceResult {
            exit_code: a.exit_code,
            stdout: a.stdout,
            stderr: a.stderr,
        },
        Err(e) => AerospaceResult {
            exit_code: -1,
            stdout: String::new(),
            stderr: e.to_string(),
        },
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
        // A delay beyond what `Instant` can represent never fires (and must not panic).
        match self.res.now().checked_add(delay) {
            Some(at) => self
                .d
                .lua_timers
                .push((at, self.d.lua_generation, callback)),
            None => log::debug!("lua: mbar.delay({delay:?}) never fires"),
        }
    }

    fn aerospace(&mut self, args: Vec<String>, callback: Option<u64>) {
        self.d.lua_aerospace(args, callback);
    }
}

fn is_lua_config(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "lua")
}

/// True if the file starts with `#!`.
fn has_shebang(path: &Path) -> bool {
    let mut head = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_or(true, |_| head == *b"#!")
}

/// The `sh -c` command line that runs a shell config (`cli.md` §10.2 step 5). The stock rc
/// has no shebang and uses bash arrays: it only works on macOS because `/bin/sh` is bash
/// there. On Linux `/bin/sh` is usually dash, which would run (and fail on) such a file
/// after `ENOEXEC`, so a shebang-less config runs with bash when it is installed
/// (`examples.md` §1.1.3).
fn config_command(path: &Path) -> String {
    let quoted = scripts::shell_quote(&path.to_string_lossy());
    if has_shebang(path) {
        quoted
    } else {
        format!(
            "if command -v bash >/dev/null 2>&1; then exec bash {quoted}; else exec {quoted}; fi"
        )
    }
}

/// JankyBorders' bootstrap service: registered while a (Homebrew) `borders` daemon runs.
const JANKYBORDERS_SERVICE: &str = "git.felix.borders";

/// Whether and how the bar `bar_name` runs the `bordersrc` found by
/// `mbar_app::config::find_bordersrc` (borders design §3): only the default bar does,
/// with the same command line as a shell config ([`config_command`], path quoted) and
/// the file's directory as working directory. Not while a foreign JankyBorders runs
/// (`jankyborders_running`, asked only when there is a `bordersrc` to run): its
/// `borders …` lines would reach this daemon through the bundled link and both would
/// draw borders.
fn bordersrc_spawn(
    bar_name: &str,
    bordersrc: Option<PathBuf>,
    jankyborders_running: impl FnOnce() -> bool,
) -> Option<(PathBuf, Spawn)> {
    if bar_name != mbar_ipc::DEFAULT_BAR_NAME {
        return None;
    }
    let path = bordersrc?;
    if jankyborders_running() {
        daemon_log(&format!(
            "bordersrc not run: JankyBorders is running ({JANKYBORDERS_SERVICE}); stop it \
             with brew services stop borders or finish the borders step in mbar.app setup"
        ));
        return None;
    }
    let spec = Spawn {
        command: config_command(&path),
        env: Vec::new(),
        cwd: path.parent().map(Path::to_path_buf),
        capture: false,
    };
    Some((path, spec))
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

    /// Borders design §3: only the default bar runs `bordersrc`, quoted like a shell
    /// config, from its own directory.
    #[test]
    fn bordersrc_runs_for_the_default_bar_only() {
        let dir = std::env::temp_dir().join(format!("mbar-brc-drv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rc = dir.join("it's bordersrc");
        std::fs::write(&rc, "#!/bin/sh\nborders width=5.0\n").unwrap();

        let none = || false;
        let unasked = || -> bool { panic!("JankyBorders check without a bordersrc to run") };
        assert!(bordersrc_spawn("mbar", None, unasked).is_none());
        assert!(bordersrc_spawn("bottom_bar", Some(rc.clone()), unasked).is_none());
        // A foreign JankyBorders runs: no spawn (it would draw borders too).
        assert!(bordersrc_spawn("mbar", Some(rc.clone()), || true).is_none());
        let (path, spec) = bordersrc_spawn("mbar", Some(rc.clone()), none).unwrap();
        assert_eq!(path, rc);
        assert_eq!(
            spec.command,
            scripts::shell_quote(&rc.to_string_lossy()),
            "shebang: the quoted path itself"
        );
        assert!(
            spec.command.contains(r"it'\''s bordersrc"),
            "{}",
            spec.command
        );
        assert_eq!(spec.cwd.as_deref(), Some(dir.as_path()));
        assert!(spec.env.is_empty() && !spec.capture);

        // Without a shebang it runs like a shebang-less shell config.
        std::fs::write(&rc, "borders width=5.0\n").unwrap();
        let (_, spec) = bordersrc_spawn("mbar", Some(rc.clone()), none).unwrap();
        assert_eq!(spec.command, config_command(&rc));
        assert!(spec.command.contains("exec bash"), "{}", spec.command);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ------------------------------------------------------------ driver harness

    use mbar_core::platform::HeadlessResources;
    use std::sync::Mutex;

    fn driver() -> (Driver, HeadlessResources) {
        let post: Post = Arc::new(|_| {});
        let cfg = DriverConfig {
            bar_name: "mbar".into(),
            home: "/nonexistent".into(),
            config_path: None,
            base_env: Vec::new(),
        };
        let mut d = Driver::new(cfg, post, Arc::new(AtomicBool::new(false)));
        let mut res = HeadlessResources {
            now: Instant::now(),
            ..HeadlessResources::default()
        };
        d.start(&mut res);
        (d, res)
    }

    /// Sends one request; returns its reply (callback transport).
    fn req(d: &mut Driver, res: &mut HeadlessResources, args: &[&str]) -> Option<String> {
        let out = Arc::new(Mutex::new(None));
        let out2 = out.clone();
        let responder = Responder::Callback(Box::new(move |t| *out2.lock().unwrap() = Some(t)));
        d.handle_event(
            Event::Request {
                args: s(args),
                responder,
            },
            res,
        );
        let _ = d.poll(res);
        let r = out.lock().unwrap().take();
        r
    }

    /// Sends one request over a socket pair; returns the client end.
    fn req_socket(d: &mut Driver, res: &mut HeadlessResources, args: &[&str]) -> UnixStream {
        let (server, client) = UnixStream::pair().unwrap();
        // Set the timeout before the request runs: once the driver has replied and
        // dropped its end, macOS rejects `setsockopt` on the disconnected socket (EINVAL).
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        d.handle_event(
            Event::Request {
                args: s(args),
                responder: Responder::Socket(server),
            },
            res,
        );
        let _ = d.poll(res);
        client
    }

    fn read_text(c: &mut UnixStream) -> std::io::Result<String> {
        mbar_ipc::socket::read_frame(c).map(|f| String::from_utf8_lossy(&f).into_owned())
    }

    // ------------------------------------------------------------ review regressions

    /// CLI-1: a shebang-less (stock) rc runs with bash, not `sh` (dash on Linux).
    #[test]
    fn shebang_less_config_runs_with_bash() {
        let dir = std::env::temp_dir().join(format!("mbar-cfgcmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("sketchybarrc");
        std::fs::write(&plain, "arr=( a b )\n").unwrap();
        let bang = dir.join("mbarrc");
        std::fs::write(&bang, "#!/bin/sh\necho hi\n").unwrap();
        let cmd = config_command(&plain);
        assert!(cmd.contains("exec bash '"), "{cmd}");
        assert_eq!(
            config_command(&bang),
            scripts::shell_quote(&bang.to_string_lossy())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PERF-11: render/animation deadlines do not feed `Input::Timer`; only the routine
    /// clock does.
    #[test]
    fn no_spurious_timer_per_frame() {
        let (mut d, mut res) = driver();
        req(&mut d, &mut res, &["--add", "item", "a", "left"]).unwrap();
        req(
            &mut d,
            &mut res,
            &["--animate", "linear", "30", "--set", "a", "y_offset=10"],
        )
        .unwrap();
        let mut frames = 0;
        for _ in 0..30 {
            res.now += Duration::from_millis(16);
            if d.poll(&mut res).is_some() {
                frames += 1;
            }
        }
        assert!(frames > 10, "animation produced {frames} frames");
        assert_eq!(d.timer_inputs, 0, "no routine tick was due (480 ms)");
        // The routine clock still fires once per second.
        res.now += Duration::from_millis(600);
        let _ = d.poll(&mut res);
        assert_eq!(d.timer_inputs, 1);
    }

    /// F1: `mbar.delay` with a delay `Instant` cannot represent must not panic.
    #[test]
    fn huge_lua_delay_does_not_panic() {
        let (mut d, mut res) = driver();
        {
            let mut host = LuaHost {
                d: &mut d,
                res: &mut res,
            };
            host.schedule(Duration::MAX, 1);
            host.schedule(Duration::from_secs_f64(1e19), 2);
            host.schedule(Duration::from_millis(5), 3);
        }
        assert_eq!(d.lua_timers.len(), 1);
        assert_eq!(d.lua_timers[0].2, 3);
    }

    /// F9: only a `--monitor` the runtime executed starts a stream; anything else gets a
    /// normal reply and the connection closes.
    #[test]
    fn monitor_detected_by_runtime_not_argv() {
        let (mut d, mut res) = driver();
        req(&mut d, &mut res, &["--add", "item", "a", "left"]).unwrap();
        // An empty argument ends the message (`cli.md` §3.1): `--monitor` never runs.
        let mut c = req_socket(
            &mut d,
            &mut res,
            &["--set", "a", "label=x", "", "--monitor"],
        );
        assert_eq!(read_text(&mut c).unwrap(), "");
        assert!(read_text(&mut c).is_err(), "connection must close");
        assert!(d.monitors.is_empty());
        // Output of earlier commands is the first frame instead of being dropped.
        let mut c = req_socket(&mut d, &mut res, &["--query", "a", "--monitor", "events"]);
        let first = read_text(&mut c).unwrap();
        assert!(first.contains("\"name\": \"a\""), "{first}");
        assert_eq!(d.monitors.len(), 1);
        // `--monitor` without a streaming transport is an error, and leaves no stream on.
        let r = req(&mut d, &mut res, &["--monitor", "stats"]).unwrap();
        assert!(r.starts_with("[!] Monitor"), "{r}");
        assert_eq!(d.monitors.len(), 1);
    }

    /// Aerospace design §Binary: `StartAerospace` is handled by the driver (one
    /// subscription, repeated requests ignored), never handed to the platform, and the
    /// subscription ends with the daemon.
    #[test]
    fn start_aerospace_is_handled_by_the_driver() {
        let (mut d, mut res) = driver();
        assert!(d.aerospace.is_none());
        let q = req(&mut d, &mut res, &["--query", "aerospace"]).unwrap();
        assert!(q.contains("\"connected\""), "{q}");
        assert!(d.aerospace.is_some());
        d.apply(vec![Effect::Platform(PlatformRequest::StartAerospace)]);
        assert!(d.aerospace.is_some());
        assert!(
            !d.take_platform_requests()
                .iter()
                .any(|r| matches!(r, PlatformRequest::StartAerospace)),
            "StartAerospace reached the platform"
        );
        req(&mut d, &mut res, &["--exit"]);
        assert!(d.exit_requested());
        assert!(d.aerospace.is_none(), "subscription must stop on exit");
    }

    /// A Lua `mbar.aerospace` result for an earlier config generation is dropped.
    #[test]
    fn stale_aerospace_results_are_dropped() {
        let (mut d, mut res) = driver();
        d.handle_event(
            Event::LuaAerospaceDone {
                generation: d.lua_generation + 7,
                id: 1,
                result: AerospaceResult::default(),
            },
            &mut res,
        );
        assert!(d.deferred.is_empty());
    }

    /// PERF-2: a `--monitor` subscriber that stops reading never blocks the main loop.
    #[test]
    fn stalled_monitor_does_not_block() {
        let (mut d, mut res) = driver();
        req(
            &mut d,
            &mut res,
            &[
                "--add",
                "event",
                "e",
                "--add",
                "item",
                "a",
                "left",
                "--subscribe",
                "a",
                "e",
            ],
        )
        .unwrap();
        // Two subscribers that read the acknowledgement and then nothing.
        let mut stalled = Vec::new();
        for _ in 0..2 {
            let mut c = req_socket(&mut d, &mut res, &["--monitor", "events"]);
            assert_eq!(read_text(&mut c).unwrap(), "");
            stalled.push(c);
        }
        let info = format!("INFO={}", "x".repeat(8000));
        let mut worst = Duration::ZERO;
        for _ in 0..400 {
            let t = Instant::now();
            req(&mut d, &mut res, &["--trigger", "e", &info]).unwrap();
            worst = worst.max(t.elapsed());
        }
        assert!(
            worst < Duration::from_millis(500),
            "worst request {worst:?}"
        );
    }
}
