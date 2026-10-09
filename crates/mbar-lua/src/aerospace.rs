//! `mbar.aerospace` (`docs/superpowers/specs/2026-10-09-aerospace-design.md`, "Lua"):
//!
//! * `run(args, fn?)` / `query(args, fn)` hand the command to [`crate::Host::aerospace`]
//!   (the daemon runs it on a worker thread and connects to AeroSpace's event stream);
//!   the result comes back through [`crate::LuaEngine::aerospace_finished`].
//! * `on(event, fn)` registers in-process handlers for the built-in `aerospace_*` events.
//!   The daemon only delivers events to items, so the first `on` adds one carrier item,
//!   [`AEROSPACE_CARRIER`] (`drawing=off`), and subscribes it with a dispatcher that calls
//!   every function registered for the event's `SENDER`, in registration order. The
//!   subscription also makes the daemon connect to AeroSpace.

use std::collections::HashSet;

use mlua::{Function, Lua, Result, Table, Value};

use crate::api::{self, Shared};
use crate::json;
use crate::props::format_scalar;

/// The built-in AeroSpace events (`mbar_core::aerospace::EVENT_NAMES`; the crate does not
/// depend on `mbar-core`, a test keeps both lists equal).
pub const AEROSPACE_EVENTS: [&str; 6] = [
    "aerospace_workspace_change",
    "aerospace_focus_change",
    "aerospace_monitor_change",
    "aerospace_mode_change",
    "aerospace_window_detected",
    "aerospace_binding_triggered",
];

/// The hidden (`drawing=off`) item that carries the `mbar.aerospace.on` handlers. It is
/// listed by `--query bar` like any other item.
pub const AEROSPACE_CARRIER: &str = "__mbar_aerospace";

const PREFIX: &str = "aerospace_";
/// Registry table `{ [event] = { fn, ... } }` of the `on` handlers.
const KEY_ON_HANDLERS: &str = "mbar.aerospace.handlers";
/// Registry slot of the dispatcher function (the carrier's handler for every event).
const KEY_DISPATCH: &str = "mbar.aerospace.dispatch";

/// The outcome of one AeroSpace command, as `mbar.aerospace.run` callbacks see it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AerospaceResult {
    /// The command's exit code; `-1` when it could not run (AeroSpace not running,
    /// protocol or I/O error).
    pub exit_code: i32,
    pub stdout: String,
    /// The command's error output, or the transport error.
    pub stderr: String,
}

/// Rust-side `mbar.aerospace` state of one engine.
#[derive(Default)]
pub(crate) struct LuaState {
    /// Callback ids of pending `query` calls (the others are `run` callbacks).
    queries: HashSet<u64>,
    /// The carrier item was added by this engine (i.e. since the last config load).
    carrier: bool,
}

/// Removes `id` from the pending queries; whether it was one.
pub(crate) fn take_query(st: &Shared, id: u64) -> bool {
    st.borrow_mut().aerospace.queries.remove(&id)
}

/// Calls a finished command's callback: `fn(result)` for `run`, `fn(value, err)` for
/// `query`.
pub(crate) fn deliver(lua: &Lua, f: Function, query: bool, r: &AerospaceResult) -> Result<()> {
    if !query {
        let t = lua.create_table_with_capacity(0, 3)?;
        t.raw_set("exit_code", r.exit_code)?;
        t.raw_set("stdout", r.stdout.as_str())?;
        t.raw_set("stderr", r.stderr.as_str())?;
        return f.call::<()>(t);
    }
    if r.exit_code != 0 {
        let msg = match r.stderr.trim() {
            "" => format!("aerospace exited with code {}", r.exit_code),
            e => e.to_string(),
        };
        return f.call::<()>((Value::Nil, msg));
    }
    match serde_json::from_str::<serde_json::Value>(&r.stdout) {
        Ok(v) => f.call::<()>((json::to_lua(lua, &v)?, Value::Nil)),
        Err(e) => f.call::<()>((Value::Nil, format!("aerospace output is not JSON: {e}"))),
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(mlua::Error::runtime(msg.into()))
}

/// The argument list of `run` / `query`: a non-empty sequence of strings (numbers are
/// converted).
fn args_of(func: &str, v: &Value) -> Result<Vec<String>> {
    let usage = format!("mbar.aerospace.{func}: args must be a non-empty list of strings");
    let Value::Table(t) = v else {
        return err(format!("{usage}, got {}", v.type_name()));
    };
    let len = t.raw_len();
    let mut count = 0usize;
    for pair in t.pairs::<Value, Value>() {
        pair?;
        count += 1;
    }
    if len == 0 || count != len {
        return err(usage);
    }
    let mut args = Vec::with_capacity(len);
    for i in 1..=len {
        match t.raw_get::<Value>(i)? {
            v @ (Value::String(_) | Value::Integer(_) | Value::Number(_)) => {
                args.push(format_scalar("", &v)?)
            }
            other => return err(format!("{usage}; element {i} is a {}", other.type_name())),
        }
    }
    Ok(args)
}

fn send(lua: &Lua, args: Vec<String>, id: Option<u64>) -> Result<()> {
    api::host_fn(lua, api::KEY_HOST_AEROSPACE)?.call::<()>((args, id))
}

fn run(lua: &Lua, st: &Shared, args: Value, f: Value) -> Result<()> {
    let args = args_of("run", &args)?;
    let id = match f {
        Value::Nil => None,
        Value::Function(f) => Some(api::register_callback(lua, st, f)?),
        other => {
            return err(format!(
                "mbar.aerospace.run(args, fn?): fn must be a function, got {}",
                other.type_name()
            ))
        }
    };
    send(lua, args, id)
}

fn query(lua: &Lua, st: &Shared, args: Value, f: Value) -> Result<()> {
    let args = args_of("query", &args)?;
    let Value::Function(f) = f else {
        return err(format!(
            "mbar.aerospace.query(args, fn): fn must be a function, got {}",
            f.type_name()
        ));
    };
    let id = api::register_callback(lua, st, f)?;
    st.borrow_mut().aerospace.queries.insert(id);
    send(lua, args, Some(id))
}

/// `aerospace_<event>` for `event` with or without the prefix, if it is a built-in one.
pub(crate) fn event_name(event: &str) -> Option<String> {
    let name = if event.starts_with(PREFIX) {
        event.to_string()
    } else {
        format!("{PREFIX}{event}")
    };
    AEROSPACE_EVENTS.contains(&name.as_str()).then_some(name)
}

fn on(lua: &Lua, st: &Shared, event: Value, f: Value) -> Result<()> {
    let names = AEROSPACE_EVENTS
        .iter()
        .map(|e| &e[PREFIX.len()..])
        .collect::<Vec<_>>()
        .join(", ");
    let Value::String(event) = event else {
        return err(format!(
            "mbar.aerospace.on(event, fn): event must be a string ({names}), got {}",
            event.type_name()
        ));
    };
    let event = event.to_string_lossy();
    let Some(name) = event_name(&event) else {
        return err(format!(
            "mbar.aerospace.on: unknown event '{event}' (one of: {names}, with or without the \
             '{PREFIX}' prefix)"
        ));
    };
    let Value::Function(f) = f else {
        return err(format!(
            "mbar.aerospace.on(event, fn): fn must be a function, got {}",
            f.type_name()
        ));
    };
    let handlers: Table = lua.named_registry_value(KEY_ON_HANDLERS)?;
    let list = match handlers.raw_get::<Option<Table>>(name.as_str())? {
        Some(l) => l,
        None => {
            let l = lua.create_table()?;
            handlers.raw_set(name.as_str(), &l)?;
            l
        }
    };
    let first = list.raw_len() == 0;
    list.raw_set(list.raw_len() + 1, f)?;
    if !first {
        return Ok(());
    }
    let add_carrier = !std::mem::replace(&mut st.borrow_mut().aerospace.carrier, true);
    if add_carrier {
        api::emit(
            st,
            vec![
                "--add".into(),
                "item".into(),
                AEROSPACE_CARRIER.into(),
                "left".into(),
                "--set".into(),
                AEROSPACE_CARRIER.into(),
                "drawing=off".into(),
            ],
        )?;
    }
    let dispatch: Function = lua.named_registry_value(KEY_DISPATCH)?;
    let carrier = Value::String(lua.create_string(AEROSPACE_CARRIER)?);
    let events = Value::String(lua.create_string(&name)?);
    api::subscribe(lua, st, &carrier, events, Some(dispatch))
}

/// The carrier's handler: calls every `on` function of the event in `env.SENDER` with
/// `env`. All of them run even when one fails; the first error is reported.
fn dispatch(lua: &Lua, env: Table) -> Result<()> {
    let Some(sender) = env.raw_get::<Option<String>>("SENDER")? else {
        return Ok(());
    };
    let handlers: Table = lua.named_registry_value(KEY_ON_HANDLERS)?;
    let Some(list) = handlers.raw_get::<Option<Table>>(sender)? else {
        return Ok(());
    };
    // A copy: a handler may register more handlers for the same event.
    let fns: Vec<Function> = list.sequence_values::<Function>().collect::<Result<_>>()?;
    let mut first_err = None;
    for f in fns {
        if let Err(e) = f.call::<()>(&env) {
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Installs `mbar.aerospace` into the module table `m`.
pub(crate) fn install(lua: &Lua, st: &Shared, m: &Table) -> Result<()> {
    lua.set_named_registry_value(KEY_ON_HANDLERS, lua.create_table()?)?;
    lua.set_named_registry_value(
        KEY_DISPATCH,
        lua.create_function(|lua, env: Table| dispatch(lua, env))?,
    )?;
    let t = lua.create_table()?;
    let s = st.clone();
    t.raw_set(
        "run",
        lua.create_function(move |lua, (args, f): (Value, Value)| run(lua, &s, args, f))?,
    )?;
    let s = st.clone();
    t.raw_set(
        "query",
        lua.create_function(move |lua, (args, f): (Value, Value)| query(lua, &s, args, f))?,
    )?;
    let s = st.clone();
    t.raw_set(
        "on",
        lua.create_function(move |lua, (event, f): (Value, Value)| on(lua, &s, event, f))?,
    )?;
    m.raw_set("aerospace", t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_with_and_without_prefix() {
        assert_eq!(
            event_name("workspace_change").as_deref(),
            Some("aerospace_workspace_change")
        );
        assert_eq!(
            event_name("aerospace_mode_change").as_deref(),
            Some("aerospace_mode_change")
        );
        assert_eq!(event_name("space_change"), None);
        assert_eq!(event_name("aerospace_"), None);
        assert_eq!(event_name(""), None);
    }
}
