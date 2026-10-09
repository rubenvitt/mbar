//! Embedded Lua 5.4 configuration and in-process event handlers for mbar.
//!
//! Lua never touches daemon internals: every `mbar.*` call is translated into
//! argv tokens of the regular command language (`docs/spec/cli.md`) and handed to
//! the daemon through the [`Host`] trait, exactly like `mbar --set ...` from a
//! shell. Event handlers registered from Lua are referenced from items as
//! `script=lua:<id>`; when such a script fires, the daemon calls
//! [`LuaEngine::run_handler`] instead of spawning a shell.
//!
//! # Batching
//!
//! Commands are queued and sent as **one** message (one layout pass) when the
//! current engine call returns. The queue is flushed earlier when Lua needs the
//! daemon to be up to date: before `mbar.query`, `mbar.exec`, `mbar.command`,
//! an `mbar.animate` block, and on `mbar.end_config()` / `mbar.flush()`.
//!
//! # Host access (soundness)
//!
//! The `&mut dyn Host` of an engine call is only borrowed for that call. Each
//! entry point runs inside [`mlua::Lua::scope`]: five *scoped* Lua functions
//! (command / spawn / schedule / aerospace / on) capture a `RefCell<&mut dyn Host>` and are stored
//! in the Lua registry for the duration of the call. When the scope ends, mlua
//! invalidates them (any later call raises a Lua error instead of touching a
//! dangling reference) and the registry slots are cleared. No `unsafe` code is
//! involved; re-entrant host use would be a `RefCell` borrow error, not UB.
//!
//! # Re-entrancy contract for the daemon
//!
//! While an engine call runs, the engine is mutably borrowed. Commands sent
//! through [`Host::command`] may trigger events whose subscribers are Lua
//! handlers (`--trigger`, `--update`, a `--set` of a `lua:` script, ...) or ask for
//! `--reload`; the daemon must **queue** those and run them after the current
//! engine call returns (never call into the engine from inside `Host`).

mod aerospace;
mod api;
mod json;
mod props;
mod shell;

pub use aerospace::{AerospaceResult, AEROSPACE_EVENTS};
pub use shell::SYNC_SHELL_ENV;

use std::cell::RefCell;
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, Table, Value};

/// Prefix of item `script`/`click_script` values that refer to Lua handlers.
pub const SCRIPT_PREFIX: &str = "lua:";

/// Returns the handler id if `script` refers to a Lua handler (`lua:<id>`).
pub fn parse_script(script: &str) -> Option<u64> {
    script.strip_prefix(SCRIPT_PREFIX)?.trim().parse().ok()
}

/// The daemon side of the Lua engine.
pub trait Host {
    /// Executes argv like the CLI (one message, may contain many commands) and
    /// returns the response text (empty on success).
    fn command(&mut self, args: Vec<String>) -> String;
    /// Spawns `sh -c cmd` asynchronously. When it exits and `callback` is
    /// `Some(id)`, the daemon calls [`LuaEngine::exec_finished`] with its stdout.
    fn spawn_shell(&mut self, cmd: String, callback: Option<u64>);
    /// Calls [`LuaEngine::timer_fired`] with `callback` after `delay`.
    fn schedule(&mut self, delay: Duration, callback: u64);
    /// Runs the AeroSpace command `args` (`mbar.aerospace.run` / `query`, e.g.
    /// `["workspace", "3"]`) off the main thread, and makes sure the daemon is
    /// connected to AeroSpace (its event stream). When the command finished and
    /// `callback` is `Some(id)`, the daemon calls [`LuaEngine::aerospace_finished`]
    /// with the result (a transport failure as `exit_code = -1` with the error in
    /// `stderr`).
    ///
    /// The default implementation (hosts without AeroSpace support) only logs: the
    /// callback never runs.
    fn aerospace(&mut self, args: Vec<String>, callback: Option<u64>) {
        let _ = callback;
        log::warn!("lua: mbar.aerospace is not supported here (args {args:?})");
    }
    /// Registers the item-less handler `handler` for `events` (`mbar.aerospace.on`):
    /// whenever one of the events is triggered, the daemon calls
    /// [`LuaEngine::run_handler`] with `handler` and the event's variables (`SENDER` is the
    /// event, there is no `NAME`). No item is involved, so item properties (`updates`,
    /// `drawing`, the default item) do not matter. The daemon forgets the handlers on
    /// `--reload` (a fresh engine registers them again).
    ///
    /// The default implementation (hosts without global handlers) only logs.
    fn on_events(&mut self, events: Vec<String>, handler: u64) {
        log::warn!("lua: item-less handlers are not supported here ({events:?}, {handler})");
    }
}

/// Errors from loading a config or running a callback.
#[derive(Debug)]
pub enum Error {
    /// The config file could not be read.
    Io {
        /// Path of the file.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A Lua error (syntax error, runtime error in the config or a handler).
    Lua(mlua::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Lua(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Lua(e) => Some(e),
        }
    }
}

impl From<mlua::Error> for Error {
    fn from(e: mlua::Error) -> Self {
        Error::Lua(e)
    }
}

/// Result type of this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Callback timing statistics (for `--query stats`, key `lua`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LuaStats {
    /// Number of handler / exec / timer callbacks run.
    pub callbacks: u64,
    /// Total time spent in callbacks, in microseconds.
    pub total_us: u64,
    /// Slowest callback, in microseconds.
    pub max_us: u64,
}

impl LuaStats {
    /// Average callback duration in microseconds.
    pub fn avg_us(&self) -> u64 {
        self.total_us.checked_div(self.callbacks).unwrap_or(0)
    }

    fn record(&mut self, d: Duration) {
        let us = u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
        self.callbacks += 1;
        self.total_us = self.total_us.saturating_add(us);
        self.max_us = self.max_us.max(us);
    }
}

/// One Lua state with the `mbar` API installed. Lives on the daemon's main
/// thread (it is neither `Send` nor `Sync`). Create a fresh engine on reload.
pub struct LuaEngine {
    lua: Lua,
    state: api::Shared,
    stats: LuaStats,
}

impl LuaEngine {
    /// Creates a Lua 5.4 state (safe standard libraries) with the `mbar` module
    /// installed as global `mbar` and as `require("mbar")` / `require("sketchybar")`.
    pub fn new() -> Result<Self> {
        let lua = Lua::new();
        let state = Rc::new(RefCell::new(api::State::default()));
        let module = api::install(&lua, &state)?;
        aerospace::install(&lua, &state, &module)?;
        shell::install(&lua, &state)?;
        Ok(LuaEngine {
            lua,
            state,
            stats: LuaStats::default(),
        })
    }

    /// Names the bar (its IPC name) this engine runs in. Shell commands started by
    /// a blocking `io.popen` / `os.execute` of this engine then get
    /// [`SYNC_SHELL_ENV`]`=<bar>` in their environment, and the `mbar` client
    /// refuses to message that bar from them: the daemon could not answer before
    /// the Lua call returns, so the client would only time out (a self-deadlock).
    /// Without a name (the default) the commands run unchanged.
    pub fn set_bar_name(&mut self, bar: Option<&str>) {
        self.state.borrow_mut().shell_marker = bar.map(str::to_owned);
    }

    /// Runs the config file at `path` (`init.lua`). Sets the global `CONFIG_DIR`
    /// (and `mbar.config_dir`) to its directory and prepends
    /// `<dir>/?.lua;<dir>/?/init.lua` to `package.path`. All commands of the
    /// config are sent as one batch at the end (or at the first flush point).
    pub fn load_file(&mut self, path: &Path, host: &mut dyn Host) -> Result<()> {
        let source = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let dir = dir.to_string_lossy().into_owned();
        self.set_config_dir(&dir)?;
        let name = format!("@{}", path.display());
        self.load_string(&source, &name, host)
    }

    /// Runs Lua source code (e.g. a config not stored in a file, or an `eval`).
    /// `chunk_name` appears in error messages (`@path` for files).
    pub fn load_string(
        &mut self,
        source: &str,
        chunk_name: &str,
        host: &mut dyn Host,
    ) -> Result<()> {
        self.call(host, false, |lua, _| {
            lua.load(source).set_name(chunk_name).exec()
        })
    }

    /// Runs the handler of an item whose script is `lua:<id>`. `env` is the
    /// script environment (`NAME`, `SENDER`, `INFO`, `BUTTON`, `MODIFIER`,
    /// `SCROLL_DELTA`, `SELECTED`, `SID`, `DID`, `PERCENTAGE`, custom trigger
    /// vars, ...); it is passed to the Lua function as a table with these string
    /// fields, plus `info` (decoded table) when `INFO` is a JSON object/array.
    /// Dispatch: the function subscribed to `SENDER`, else the catch-all. An
    /// unknown id is ignored.
    pub fn run_handler(
        &mut self,
        id: u64,
        env: &[(String, String)],
        host: &mut dyn Host,
    ) -> Result<()> {
        self.call(host, true, |lua, _| {
            let handlers: Table = lua.named_registry_value(api::KEY_HANDLERS)?;
            let Some(entry) = handlers.raw_get::<Option<Table>>(id)? else {
                log::debug!("lua: no handler {id}");
                return Ok(());
            };
            let sender = env
                .iter()
                .rev()
                .find(|(k, _)| k == "SENDER")
                .map(|(_, v)| v.as_str());
            let mut f: Option<Function> = None;
            if let Some(sender) = sender {
                let by_event: Table = entry.raw_get("events")?;
                f = by_event.raw_get(sender)?;
            }
            if f.is_none() {
                f = entry.raw_get("any")?;
            }
            let Some(f) = f else {
                return Ok(());
            };
            let t = lua.create_table_with_capacity(0, env.len() + 1)?;
            for (k, v) in env {
                t.raw_set(k.as_str(), v.as_str())?;
                if k == "INFO" {
                    if let Some(info) = json::decode_structured(lua, v)? {
                        t.raw_set("info", info)?;
                    }
                }
            }
            f.call::<()>(t)
        })
    }

    /// Completes an `mbar.exec(cmd, fn)`: calls `fn(result, output)` where
    /// `result` is the decoded JSON object/array if `output` is one, else
    /// `output` itself. An unknown id is ignored.
    pub fn exec_finished(&mut self, id: u64, output: String, host: &mut dyn Host) -> Result<()> {
        self.call(host, true, |lua, _| {
            let Some(f) = take_callback(lua, id)? else {
                return Ok(());
            };
            let result = match json::decode_structured(lua, &output)? {
                Some(v) => v,
                None => Value::String(lua.create_string(&output)?),
            };
            f.call::<()>((result, output.as_str()))
        })
    }

    /// Completes an `mbar.aerospace.run(args, fn)` (`fn(result)` with a table
    /// `{ exit_code, stdout, stderr }`) or `mbar.aerospace.query(args, fn)`
    /// (`fn(value, err)`: `value` is stdout decoded as JSON when the command
    /// succeeded and the output parses, else `nil` and `err` says why). An unknown
    /// id is ignored.
    pub fn aerospace_finished(
        &mut self,
        id: u64,
        result: AerospaceResult,
        host: &mut dyn Host,
    ) -> Result<()> {
        self.call(host, true, |lua, st| {
            let query = aerospace::take_query(st, id);
            let Some(f) = take_callback(lua, id)? else {
                return Ok(());
            };
            aerospace::deliver(lua, f, query, &result)
        })
    }

    /// Completes an `mbar.delay(seconds, fn)`. An unknown id is ignored.
    pub fn timer_fired(&mut self, id: u64, host: &mut dyn Host) -> Result<()> {
        self.call(host, true, |lua, _| match take_callback(lua, id)? {
            Some(f) => f.call::<()>(()),
            None => Ok(()),
        })
    }

    /// Whether `id` is a registered item handler (`lua:<id>`).
    pub fn has_handler(&self, id: u64) -> bool {
        self.lua
            .named_registry_value::<Table>(api::KEY_HANDLERS)
            .and_then(|h| h.raw_get::<Option<Table>>(id))
            .map(|t| t.is_some())
            .unwrap_or(false)
    }

    /// Callback statistics (handlers, exec and timer callbacks).
    pub fn stats(&self) -> LuaStats {
        self.stats
    }

    /// Memory currently used by the Lua state, in bytes.
    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }

    fn set_config_dir(&self, dir: &str) -> mlua::Result<()> {
        let globals = self.lua.globals();
        globals.raw_set("CONFIG_DIR", dir)?;
        let mbar: Table = globals.raw_get("mbar")?;
        mbar.raw_set("config_dir", dir)?;
        let package: Table = globals.get("package")?;
        let old: String = package.get("path")?;
        let dir = dir.trim_end_matches('/');
        package.set("path", format!("{dir}/?.lua;{dir}/?/init.lua;{old}"))?;
        Ok(())
    }

    /// Runs `f` with `host` reachable from the `mbar` API, then flushes the queue.
    fn call(
        &mut self,
        host: &mut dyn Host,
        timed: bool,
        f: impl FnOnce(&Lua, &api::Shared) -> mlua::Result<()>,
    ) -> Result<()> {
        let start = Instant::now();
        let lua = &self.lua;
        let st = &self.state;
        let host = RefCell::new(host);
        let host = &host;
        let result = lua.scope(|scope| {
            let command = scope
                .create_function(move |_, argv: Vec<String>| Ok(host.borrow_mut().command(argv)))?;
            let spawn = scope.create_function(move |_, (cmd, id): (String, Option<u64>)| {
                host.borrow_mut().spawn_shell(cmd, id);
                Ok(())
            })?;
            let schedule = scope.create_function(move |_, (secs, id): (f64, u64)| {
                // `mbar.delay` passes a finite, non-negative value. One too large for a
                // `Duration` means "never", not "now".
                let delay = Duration::try_from_secs_f64(secs).unwrap_or(if secs > 0.0 {
                    Duration::MAX
                } else {
                    Duration::ZERO
                });
                host.borrow_mut().schedule(delay, id);
                Ok(())
            })?;
            lua.set_named_registry_value(api::KEY_HOST_COMMAND, command)?;
            lua.set_named_registry_value(api::KEY_HOST_SPAWN, spawn)?;
            lua.set_named_registry_value(api::KEY_HOST_SCHEDULE, schedule)?;
            let aerospace =
                scope.create_function(move |_, (args, id): (Vec<String>, Option<u64>)| {
                    host.borrow_mut().aerospace(args, id);
                    Ok(())
                })?;
            lua.set_named_registry_value(api::KEY_HOST_AEROSPACE, aerospace)?;
            let on = scope.create_function(move |_, (events, id): (Vec<String>, u64)| {
                host.borrow_mut().on_events(events, id);
                Ok(())
            })?;
            lua.set_named_registry_value(api::KEY_HOST_ON, on)?;

            let result = f(lua, st);
            // Commands issued before an error still go out, like the earlier
            // lines of a failing shell script.
            st.borrow_mut().anim.clear();
            let flushed = api::flush(lua, st);

            for key in [
                api::KEY_HOST_COMMAND,
                api::KEY_HOST_SPAWN,
                api::KEY_HOST_SCHEDULE,
                api::KEY_HOST_AEROSPACE,
                api::KEY_HOST_ON,
            ] {
                lua.unset_named_registry_value(key)?;
            }
            result?;
            flushed.map(|_| ())
        });
        if timed {
            self.stats.record(start.elapsed());
        }
        result.map_err(Error::Lua)
    }
}

fn take_callback(lua: &Lua, id: u64) -> mlua::Result<Option<Function>> {
    let callbacks: Table = lua.named_registry_value(api::KEY_CALLBACKS)?;
    let f: Option<Function> = callbacks.raw_get(id)?;
    if f.is_some() {
        callbacks.raw_set(id, Value::Nil)?;
    }
    Ok(f)
}
