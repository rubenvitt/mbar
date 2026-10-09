//! The `mbar` Lua module: every function turns into argv tokens of the
//! command language (`docs/spec/cli.md`) that are queued and sent to the daemon
//! through the [`crate::Host`] of the current engine call.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, Result, Table, Value, Variadic};

use crate::json;
use crate::props::{self, format_float, format_scalar};
use crate::SCRIPT_PREFIX;

pub(crate) const KEY_HOST_COMMAND: &str = "mbar.host.command";
pub(crate) const KEY_HOST_SPAWN: &str = "mbar.host.spawn";
pub(crate) const KEY_HOST_SCHEDULE: &str = "mbar.host.schedule";
pub(crate) const KEY_HANDLERS: &str = "mbar.handlers";
pub(crate) const KEY_CALLBACKS: &str = "mbar.callbacks";
const KEY_ITEM_MT: &str = "mbar.item_mt";

/// Pseudo events: they are `SENDER` values (routine tick, `--update`) but not
/// subscribable events, so they are never sent to `--subscribe`.
const PSEUDO_EVENTS: [&str; 3] = ["*", "routine", "forced"];

/// Rust-side state shared by all `mbar.*` functions. Lua values (handler
/// functions) live in registry tables, never here, so there are no reference
/// cycles between Rust and Lua.
#[derive(Default)]
pub(crate) struct State {
    /// Commands queued for the next flush, as one flat argv.
    pub(crate) queue: Vec<String>,
    /// Open `mbar.animate` blocks (innermost last).
    pub(crate) anim: Vec<AnimFrame>,
    next_id: u64,
    /// Item name (or `/regex/` selector) -> handler id of its `script=lua:<id>`.
    item_handlers: HashMap<String, u64>,
    /// Item name (or `/regex/` selector) -> handler id of its `click_script=lua:<id>`.
    click_handlers: HashMap<String, u64>,
    anon: u64,
    batch_depth: u32,
    /// IPC bar name exported to blocking `io.popen` / `os.execute` commands
    /// (see `crate::shell`).
    pub(crate) shell_marker: Option<String>,
}

pub(crate) struct AnimFrame {
    curve: String,
    duration: String,
    cmds: Vec<String>,
}

pub(crate) type Shared = Rc<RefCell<State>>;

fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(mlua::Error::runtime(msg.into()))
}

// ---------------------------------------------------------------------------
// Queue / host access
// ---------------------------------------------------------------------------

/// Queues one command (or appends it to the innermost animate block).
pub(crate) fn emit(st: &Shared, argv: Vec<String>) -> Result<()> {
    if let Some(i) = argv.iter().position(String::is_empty) {
        // An empty argv element terminates the whole message (cli.md §3.1), which
        // would silently drop every later command of the batch.
        return err(format!(
            "mbar: empty argument #{i} in command {argv:?} (would end the message)"
        ));
    }
    let mut s = st.borrow_mut();
    match s.anim.last_mut() {
        Some(frame) => frame.cmds.extend(argv),
        None => s.queue.extend(argv),
    }
    Ok(())
}

pub(crate) fn host_command(lua: &Lua, argv: Vec<String>) -> Result<String> {
    match lua.named_registry_value::<Option<Function>>(KEY_HOST_COMMAND)? {
        Some(f) => f.call(argv),
        None => err("mbar: no daemon connection outside of an engine call"),
    }
}

fn host_fn(lua: &Lua, key: &str) -> Result<Function> {
    match lua.named_registry_value::<Option<Function>>(key)? {
        Some(f) => Ok(f),
        None => err("mbar: no daemon connection outside of an engine call"),
    }
}

pub(crate) fn log_response(resp: &str) {
    for line in resp.lines().filter(|l| !l.trim().is_empty()) {
        log::warn!("lua: {line}");
    }
}

/// Sends the queued commands as one message. Returns the daemon's response.
pub(crate) fn flush(lua: &Lua, st: &Shared) -> Result<String> {
    let queue = std::mem::take(&mut st.borrow_mut().queue);
    if queue.is_empty() {
        return Ok(String::new());
    }
    let resp = host_command(lua, queue)?;
    log_response(&resp);
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn new_id(st: &Shared) -> u64 {
    let mut s = st.borrow_mut();
    s.next_id += 1;
    s.next_id
}

/// Registry entry `{ events = { [event] = fn }, any = fn? }` for handler `id`.
fn handler_entry(lua: &Lua, id: u64) -> Result<Table> {
    let handlers: Table = lua.named_registry_value(KEY_HANDLERS)?;
    if let Some(t) = handlers.raw_get::<Option<Table>>(id)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.raw_set("events", lua.create_table()?)?;
    handlers.raw_set(id, &t)?;
    Ok(t)
}

fn is_regex(name: &str) -> bool {
    name.len() > 1 && name.starts_with('/') && name.ends_with('/')
}

/// The per-item handler slots: `script=lua:<id>` and `click_script=lua:<id>`.
#[derive(Clone, Copy)]
enum Slot {
    Script,
    Click,
}

impl State {
    fn slot(&mut self, slot: Slot) -> &mut HashMap<String, u64> {
        match slot {
            Slot::Script => &mut self.item_handlers,
            Slot::Click => &mut self.click_handlers,
        }
    }
}

/// Handler id of the `slot` of item (or `/regex/` selector) `name`, created on
/// first use and reused afterwards, so replacing a handler function never
/// allocates a new registry entry.
fn item_handler(st: &Shared, slot: Slot, name: &str) -> u64 {
    let existing = st.borrow_mut().slot(slot).get(name).copied();
    if let Some(id) = existing {
        return id;
    }
    let id = new_id(st);
    st.borrow_mut().slot(slot).insert(name.to_string(), id);
    id
}

/// Drops the `slot` handler of `name` (mapping and registry entry).
fn forget_slot(lua: &Lua, st: &Shared, slot: Slot, name: &str) -> Result<()> {
    let id = st.borrow_mut().slot(slot).remove(name);
    if let Some(id) = id {
        let handlers: Table = lua.named_registry_value(KEY_HANDLERS)?;
        handlers.raw_set(id, Value::Nil)?;
    }
    Ok(())
}

/// Drops every Lua handler of `name`: the item is gone (or about to be
/// re-created), so a later `subscribe` must start from a fresh handler.
fn forget_item(lua: &Lua, st: &Shared, name: &str) -> Result<()> {
    forget_slot(lua, st, Slot::Script, name)?;
    forget_slot(lua, st, Slot::Click, name)
}

/// Moves the handlers of `old` to `new` after a successful `--rename`.
fn rename_item(lua: &Lua, st: &Shared, old: &str, new: &str) -> Result<()> {
    // `new` did not exist in the daemon, so anything still mapped to it is stale.
    forget_item(lua, st, new)?;
    let mut s = st.borrow_mut();
    for slot in [Slot::Script, Slot::Click] {
        if let Some(id) = s.slot(slot).remove(old) {
            s.slot(slot).insert(new.to_string(), id);
        }
    }
    Ok(())
}

/// Keeps the handler maps in sync with `--remove` / `--rename` sent verbatim
/// through `mbar.command`. Regex removes cannot be resolved here; `mbar.add`
/// and `subscribe` cover the names they removed.
fn track_command(lua: &Lua, st: &Shared, argv: &[String], resp: &str) -> Result<()> {
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--remove" if i + 1 < argv.len() => {
                if !is_regex(&argv[i + 1]) {
                    forget_item(lua, st, &argv[i + 1])?;
                }
                i += 2;
            }
            "--rename" if i + 2 < argv.len() => {
                let (old, new) = (&argv[i + 1], &argv[i + 2]);
                let failed = format!("Failed to rename item: {old} -> {new}\n");
                if !new.is_empty() && !resp.contains(&failed) {
                    rename_item(lua, st, old, new)?;
                }
                i += 3;
            }
            _ => i += 1,
        }
    }
    Ok(())
}

pub(crate) fn register_callback(lua: &Lua, st: &Shared, f: Function) -> Result<u64> {
    let id = new_id(st);
    let callbacks: Table = lua.named_registry_value(KEY_CALLBACKS)?;
    callbacks.raw_set(id, f)?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn check_name(name: String) -> Result<String> {
    if name.is_empty() {
        return err("mbar: item name must not be empty");
    }
    if name.starts_with('-') {
        return err(format!("mbar: item name '{name}' must not start with '-'"));
    }
    Ok(name)
}

/// An item name, or an item object (any table with a `name` field).
fn name_of(v: &Value) -> Result<String> {
    let name = match v {
        Value::String(s) => s.to_string_lossy(),
        Value::Table(t) => match t.raw_get::<Option<String>>("name")? {
            Some(n) => n,
            None => return err("mbar: expected an item name or item object"),
        },
        other => {
            return err(format!(
                "mbar: expected an item name, got {}",
                other.type_name()
            ))
        }
    };
    check_name(name)
}

/// A free-form argument token (string, number, boolean or item object).
fn token_of(v: &Value) -> Result<String> {
    match v {
        Value::Table(_) => name_of(v),
        other => format_scalar("", other),
    }
}

/// Event list: a string (whitespace separated) or a list of strings. `nil` and
/// `"*"` mean "every sender" (catch-all).
fn events_of(v: &Value) -> Result<Vec<String>> {
    let mut out = Vec::new();
    match v {
        Value::Nil => {}
        Value::String(s) => out.extend(s.to_string_lossy().split_whitespace().map(String::from)),
        Value::Table(t) => {
            for e in t.sequence_values::<String>() {
                out.extend(e?.split_whitespace().map(String::from));
            }
        }
        other => {
            return err(format!(
                "mbar.subscribe: events must be a string or a list, got {}",
                other.type_name()
            ))
        }
    }
    Ok(out)
}

/// Resolver for functions inside property tables: `script = fn` becomes the
/// item's Lua handler (catch-all), `click_script = fn` the item's click handler.
/// Both reuse the item's (or regex selector's) handler id, so replacing them
/// from a handler does not grow the registry. `mbar.bar` / `mbar.default`
/// (`item == None`) need a new id per call: items created from an earlier
/// default keep referring to the earlier function.
fn resolver<'a>(
    lua: &'a Lua,
    st: &'a Shared,
    item: Option<&'a str>,
) -> impl FnMut(&str, Function) -> Result<String> + 'a {
    move |key, f| {
        let id = match (key, item) {
            ("script", Some(name)) => item_handler(st, Slot::Script, name),
            ("click_script", Some(name)) => item_handler(st, Slot::Click, name),
            ("script" | "click_script", None) => new_id(st),
            _ => {
                return err(format!(
                "mbar: functions are only allowed for 'script' and 'click_script', not for '{key}'"
            ))
            }
        };
        handler_entry(lua, id)?.raw_set("any", f)?;
        Ok(format!("{SCRIPT_PREFIX}{id}"))
    }
}

/// Flattens item properties, registering function values as handlers.
fn item_tokens(lua: &Lua, st: &Shared, name: &str, props: &Table) -> Result<Vec<String>> {
    let tokens = props::flatten(props, &mut resolver(lua, st, Some(name)))?;
    for (key, slot) in [("script=", Slot::Script), ("click_script=", Slot::Click)] {
        let plain = tokens.iter().any(|t| {
            t.strip_prefix(key)
                .is_some_and(|v| !v.starts_with(SCRIPT_PREFIX))
        });
        if plain {
            // A shell script replaces the item's Lua handler.
            forget_slot(lua, st, slot, name)?;
        }
    }
    Ok(tokens)
}

fn emit_pairs(st: &Shared, head: Vec<String>, tokens: Vec<String>) -> Result<()> {
    // A Mode A domain without pairs would swallow the next command (cli.md §3.2).
    if tokens.is_empty() {
        return Ok(());
    }
    let mut argv = head;
    argv.extend(tokens);
    emit(st, argv)
}

fn make_item(lua: &Lua, name: &str) -> Result<Table> {
    let t = lua.create_table()?;
    t.raw_set("name", name)?;
    let mt: Table = lua.named_registry_value(KEY_ITEM_MT)?;
    t.set_metatable(Some(mt))?;
    Ok(t)
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

fn set(lua: &Lua, st: &Shared, name: &Value, props: &Table) -> Result<()> {
    let name = name_of(name)?;
    let tokens = item_tokens(lua, st, &name, props)?;
    emit_pairs(st, vec!["--set".into(), name], tokens)
}

fn add(lua: &Lua, st: &Shared, args: Variadic<Value>) -> Result<Value> {
    let mut args = args.into_iter();
    let ty = match args.next() {
        Some(Value::String(s)) => s.to_string_lossy(),
        _ => return err("mbar.add: the first argument must be the item type"),
    };
    if ty.is_empty() || ty.starts_with('-') {
        return err(format!("mbar.add: invalid type '{ty}'"));
    }
    let mut rest: Vec<Value> = args.collect();

    if ty == "event" {
        let name = match rest.first() {
            Some(v @ Value::String(_)) => name_of(v)?,
            _ => return err("mbar.add('event', name, notification?): name required"),
        };
        let mut argv = vec!["--add".into(), "event".into(), name];
        if let Some(Value::String(n)) = rest.get(1) {
            argv.push(n.to_string_lossy());
        }
        emit(st, argv)?;
        return Ok(Value::Nil);
    }

    let name = match rest.first() {
        Some(v @ Value::String(_)) => {
            let n = name_of(v)?;
            rest.remove(0);
            n
        }
        _ => {
            let n = {
                let mut s = st.borrow_mut();
                s.anon += 1;
                s.anon
            };
            format!("__mbar.{ty}.{n}")
        }
    };
    // A (re-)created item starts without Lua handlers. Removes by regex, through
    // `mbar.command` or from a shell leave stale mappings behind, and reusing
    // them would skip `script=lua:<id>` or share handlers with a renamed item.
    forget_item(lua, st, &name)?;

    if ty == "bracket" {
        let mut members = Vec::new();
        if let Some(Value::Table(t)) = rest.first() {
            for m in t.sequence_values::<Value>() {
                members.push(name_of(&m?)?);
            }
        }
        if members.is_empty() {
            return err(
                "mbar.add('bracket', name, members, props?): members must be a non-empty list",
            );
        }
        let mut argv = vec!["--add".into(), "bracket".into(), name.clone()];
        argv.extend(members);
        emit(st, argv)?;
        if let Some(Value::Table(props)) = rest.get(1) {
            let tokens = item_tokens(lua, st, &name, props)?;
            emit_pairs(st, vec!["--set".into(), name.clone()], tokens)?;
        }
        return Ok(Value::Table(make_item(lua, &name)?));
    }

    let mut position: Option<String> = None;
    let mut width: Option<String> = None;
    let mut props: Option<Table> = None;
    for v in rest {
        match v {
            Value::String(s) if position.is_none() => position = Some(s.to_string_lossy()),
            Value::Integer(i) => width = Some(i.to_string()),
            Value::Number(f) => width = Some(format_float(f.round())),
            Value::Table(t) => props = Some(t),
            Value::Nil => {}
            other => {
                return err(format!(
                    "mbar.add: unexpected argument of type {}",
                    other.type_name()
                ))
            }
        }
    }
    let mut tokens = Vec::new();
    if let Some(p) = &props {
        tokens = item_tokens(lua, st, &name, p)?;
        // `position` is the positional argument of `--add`, not a `--set` pair.
        let mut from_props = None;
        tokens.retain(|t| match t.strip_prefix("position=") {
            Some(pos) => {
                from_props = Some(pos.to_string());
                false
            }
            None => true,
        });
        position = position.or(from_props);
    }
    let position = position.unwrap_or_else(|| "left".into());
    let mut argv = vec!["--add".into(), ty, name.clone(), position];
    argv.extend(width);
    emit(st, argv)?;
    emit_pairs(st, vec!["--set".into(), name.clone()], tokens)?;
    Ok(Value::Table(make_item(lua, &name)?))
}

fn subscribe(
    lua: &Lua,
    st: &Shared,
    name: &Value,
    events: Value,
    f: Option<Function>,
) -> Result<()> {
    let name = name_of(name)?;
    let (events, f) = match (events, f) {
        (Value::Function(f), None) => (Vec::new(), f),
        (ev, Some(f)) => (events_of(&ev)?, f),
        (_, None) => return err("mbar.subscribe(name, events, fn): handler function required"),
    };
    let id = item_handler(st, Slot::Script, &name);
    let entry = handler_entry(lua, id)?;
    if events.is_empty() || events.iter().any(|e| e == "*") {
        entry.raw_set("any", f.clone())?;
    }
    let by_event: Table = entry.raw_get("events")?;
    for e in events.iter().filter(|e| *e != "*") {
        by_event.raw_set(e.as_str(), f.clone())?;
    }
    // Always (re)assert the script: setting the same value is a no-op in the
    // daemon (item.md §3), and the item may have been removed and re-added, or
    // had its script replaced, behind the engine's back.
    emit(
        st,
        vec![
            "--set".into(),
            name.clone(),
            format!("script={SCRIPT_PREFIX}{id}"),
        ],
    )?;
    let real: Vec<String> = events
        .into_iter()
        .filter(|e| !PSEUDO_EVENTS.contains(&e.as_str()))
        .collect();
    if !real.is_empty() {
        let mut argv = vec!["--subscribe".into(), name];
        argv.extend(real);
        emit(st, argv)?;
    }
    Ok(())
}

fn animate(lua: &Lua, st: &Shared, curve: String, duration: Value, f: Function) -> Result<()> {
    if curve.is_empty() || curve.starts_with('-') {
        return err(format!("mbar.animate: invalid curve '{curve}'"));
    }
    let duration = match duration {
        Value::Integer(i) => i.max(0).to_string(),
        Value::Number(n) => format_float(n.max(0.0).round()),
        Value::String(s) => s.to_string_lossy(),
        other => {
            return err(format!(
                "mbar.animate: duration must be a number, got {}",
                other.type_name()
            ))
        }
    };
    st.borrow_mut().anim.push(AnimFrame {
        curve,
        duration,
        cmds: Vec::new(),
    });
    let result = f.call::<()>(());
    let frame = st.borrow_mut().anim.pop();
    result?;
    let Some(frame) = frame else {
        return err("mbar.animate: animation stack corrupted");
    };
    if frame.cmds.is_empty() {
        return Ok(());
    }
    let mut msg = vec!["--animate".to_string(), frame.curve, frame.duration];
    msg.extend(frame.cmds);
    {
        let mut s = st.borrow_mut();
        if let Some(parent) = s.anim.last_mut() {
            // Nested block: inline it and restore the outer animation afterwards.
            let restore = [
                "--animate".to_string(),
                parent.curve.clone(),
                parent.duration.clone(),
            ];
            parent.cmds.extend(msg);
            parent.cmds.extend(restore);
            return Ok(());
        }
    }
    // `--animate` applies to the rest of a message, so the block goes out as its
    // own message after everything queued before it.
    flush(lua, st)?;
    let resp = host_command(lua, msg)?;
    log_response(&resp);
    Ok(())
}

fn trigger(st: &Shared, event: String, env: Option<Table>) -> Result<()> {
    let mut argv = vec!["--trigger".to_string(), event];
    if let Some(env) = env {
        let (_, named) = props::split_table(&env)?;
        for (k, v) in named {
            let value = match &v {
                Value::Table(_) => json::from_lua(&v)?.to_string(),
                other => format_scalar(&k, other)?,
            };
            argv.push(format!("{k}={value}"));
        }
    }
    emit(st, argv)
}

fn query(lua: &Lua, st: &Shared, args: &[Value]) -> Result<(Value, Option<String>)> {
    let mut argv = vec!["--query".to_string()];
    for a in args {
        argv.push(token_of(a)?);
    }
    if argv.len() == 1 {
        return err("mbar.query(what, ...): argument required");
    }
    flush(lua, st)?;
    let resp = host_command(lua, argv)?;
    match json::decode(lua, &resp)? {
        Some(v) => Ok((v, None)),
        None => Ok((Value::Nil, Some(resp))),
    }
}

fn exec(lua: &Lua, st: &Shared, cmd: String, f: Option<Function>) -> Result<()> {
    let id = match f {
        Some(f) => Some(register_callback(lua, st, f)?),
        None => None,
    };
    // The command may talk to the daemon itself, so it must see our changes.
    flush(lua, st)?;
    host_fn(lua, KEY_HOST_SPAWN)?.call::<()>((cmd, id))
}

fn delay(lua: &Lua, st: &Shared, seconds: f64, f: Function) -> Result<()> {
    if !seconds.is_finite() {
        return err("mbar.delay: seconds must be a finite number");
    }
    let id = register_callback(lua, st, f)?;
    host_fn(lua, KEY_HOST_SCHEDULE)?.call::<()>((seconds.max(0.0), id))
}

fn push(st: &Shared, name: &Value, values: Variadic<Value>) -> Result<()> {
    let name = name_of(name)?;
    let mut argv = vec!["--push".to_string(), name];
    for v in values {
        match v {
            Value::Table(t) => {
                for x in t.sequence_values::<Value>() {
                    argv.push(format_scalar("", &x?)?);
                }
            }
            other => argv.push(format_scalar("", &other)?),
        }
    }
    if argv.len() > 2 {
        emit(st, argv)?;
    }
    Ok(())
}

fn provider(st: &Shared, name: &Value, spec: Value) -> Result<()> {
    let name = name_of(name)?;
    let mut tokens = Vec::new();
    match spec {
        Value::Nil | Value::Boolean(false) => tokens.push("provider=none".to_string()),
        Value::String(s) => tokens.push(format!("provider={}", s.to_string_lossy())),
        Value::Table(t) => {
            let (positional, named) = props::split_table(&t)?;
            let pname = match t.raw_get::<Option<String>>("name")? {
                Some(n) => Some(n),
                None => positional
                    .first()
                    .map(|v| format_scalar("provider", v))
                    .transpose()?,
            };
            if let Some(p) = pname {
                tokens.push(format!("provider={p}"));
            }
            for (k, v) in named.into_iter().filter(|(k, _)| k != "name") {
                let key = format!("provider.{k}");
                let value = format_scalar(&key, &v)?;
                tokens.push(format!("{key}={value}"));
            }
        }
        other => {
            return err(format!(
                "mbar.provider: expected a provider name or table, got {}",
                other.type_name()
            ))
        }
    }
    emit_pairs(st, vec!["--set".into(), name], tokens)
}

fn remove(lua: &Lua, st: &Shared, name: &Value) -> Result<()> {
    let name = name_of(name)?;
    forget_item(lua, st, &name)?;
    emit(st, vec!["--remove".into(), name])
}

fn bool_token(v: &Value) -> Result<String> {
    match v {
        Value::Nil => Ok("on".into()),
        other => format_scalar("", other),
    }
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

fn reg<A, R, F>(lua: &Lua, t: &Table, name: &str, st: &Shared, f: F) -> Result<()>
where
    A: FromLuaMulti,
    R: IntoLuaMulti,
    F: Fn(&Lua, &Shared, A) -> Result<R> + 'static,
{
    let st = st.clone();
    t.raw_set(name, lua.create_function(move |lua, a: A| f(lua, &st, a))?)
}

/// Creates the `mbar` module, sets the global `mbar` and makes
/// `require("mbar")` / `require("sketchybar")` return it.
pub(crate) fn install(lua: &Lua, st: &Shared) -> Result<Table> {
    lua.set_named_registry_value(KEY_HANDLERS, lua.create_table()?)?;
    lua.set_named_registry_value(KEY_CALLBACKS, lua.create_table()?)?;

    let m = lua.create_table()?;
    reg(lua, &m, "add", st, |lua, st, args: Variadic<Value>| {
        add(lua, st, args)
    })?;
    reg(
        lua,
        &m,
        "set",
        st,
        |lua, st, (name, props): (Value, Table)| set(lua, st, &name, &props),
    )?;
    reg(lua, &m, "bar", st, |lua, st, props: Table| {
        let tokens = props::flatten(&props, &mut resolver(lua, st, None))?;
        emit_pairs(st, vec!["--bar".into()], tokens)
    })?;
    reg(lua, &m, "borders", st, |_, st, props: Table| {
        emit_pairs(
            st,
            vec!["--borders".into()],
            props::flatten_borders(&props)?,
        )
    })?;
    reg(lua, &m, "default", st, |lua, st, props: Table| {
        let tokens = props::flatten(&props, &mut resolver(lua, st, None))?;
        emit_pairs(st, vec!["--default".into()], tokens)
    })?;
    reg(lua, &m, "remove", st, |lua, st, name: Value| {
        remove(lua, st, &name)
    })?;
    reg(
        lua,
        &m,
        "subscribe",
        st,
        |lua, st, (name, events, f): (Value, Value, Option<Function>)| {
            subscribe(lua, st, &name, events, f)
        },
    )?;
    reg(
        lua,
        &m,
        "animate",
        st,
        |lua, st, (curve, duration, f): (String, Value, Function)| {
            animate(lua, st, curve, duration, f)
        },
    )?;
    reg(
        lua,
        &m,
        "trigger",
        st,
        |_, st, (event, env): (String, Option<Table>)| trigger(st, event, env),
    )?;
    reg(lua, &m, "query", st, |lua, st, args: Variadic<Value>| {
        query(lua, st, &args)
    })?;
    reg(
        lua,
        &m,
        "exec",
        st,
        |lua, st, (cmd, f): (String, Option<Function>)| exec(lua, st, cmd, f),
    )?;
    reg(
        lua,
        &m,
        "delay",
        st,
        |lua, st, (secs, f): (f64, Function)| delay(lua, st, secs, f),
    )?;
    reg(
        lua,
        &m,
        "push",
        st,
        |_, st, (name, values): (Value, Variadic<Value>)| push(st, &name, values),
    )?;
    reg(
        lua,
        &m,
        "provider",
        st,
        |_, st, (name, spec): (Value, Value)| provider(st, &name, spec),
    )?;
    reg(lua, &m, "begin_config", st, |_, st, ()| {
        st.borrow_mut().batch_depth += 1;
        Ok(())
    })?;
    reg(lua, &m, "end_config", st, |lua, st, ()| {
        {
            let mut s = st.borrow_mut();
            s.batch_depth = s.batch_depth.saturating_sub(1);
        }
        flush(lua, st).map(|_| ())
    })?;
    reg(lua, &m, "flush", st, |lua, st, ()| flush(lua, st))?;
    reg(lua, &m, "hotload", st, |_, st, v: Value| {
        emit(st, vec!["--hotload".into(), bool_token(&v)?])
    })?;
    reg(lua, &m, "update", st, |_, st, ()| {
        emit(st, vec!["--update".into()])
    })?;
    reg(lua, &m, "menu", st, |_, st, v: Value| {
        emit(st, vec!["--menu".into(), token_of(&v)?])
    })?;
    reg(lua, &m, "reload", st, |_, st, path: Option<String>| {
        let mut argv = vec!["--reload".to_string()];
        argv.extend(path);
        emit(st, argv)
    })?;
    reg(lua, &m, "command", st, |lua, st, args: Variadic<Value>| {
        let argv = args.iter().map(token_of).collect::<Result<Vec<_>>>()?;
        if argv.is_empty() {
            return Ok(String::new());
        }
        if argv.iter().any(String::is_empty) {
            return err("mbar.command: empty argument");
        }
        flush(lua, st)?;
        let resp = host_command(lua, argv.clone())?;
        track_command(lua, st, &argv, &resp)?;
        Ok(resp)
    })?;
    m.raw_set("event_loop", lua.create_function(|_, ()| Ok(()))?)?;

    let json_t = lua.create_table()?;
    json_t.raw_set(
        "decode",
        lua.create_function(|lua, s: String| -> Result<(Value, Option<String>)> {
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(j) => Ok((json::to_lua(lua, &j)?, None)),
                Err(e) => Ok((Value::Nil, Some(e.to_string()))),
            }
        })?,
    )?;
    json_t.raw_set(
        "encode",
        lua.create_function(|_, v: Value| Ok(json::from_lua(&v)?.to_string()))?,
    )?;
    m.raw_set("json", json_t)?;

    // Item objects returned by mbar.add.
    let methods = lua.create_table()?;
    reg(
        lua,
        &methods,
        "set",
        st,
        |lua, st, (this, props): (Table, Table)| {
            set(lua, st, &Value::Table(this.clone()), &props)?;
            Ok(this)
        },
    )?;
    reg(
        lua,
        &methods,
        "subscribe",
        st,
        |lua, st, (this, events, f): (Table, Value, Option<Function>)| {
            subscribe(lua, st, &Value::Table(this.clone()), events, f)?;
            Ok(this)
        },
    )?;
    reg(lua, &methods, "query", st, |lua, st, this: Table| {
        let name = name_of(&Value::Table(this))?;
        query(
            lua,
            st,
            &[
                Value::String(lua.create_string("item")?),
                Value::String(lua.create_string(&name)?),
            ],
        )
    })?;
    reg(lua, &methods, "remove", st, |lua, st, this: Table| {
        remove(lua, st, &Value::Table(this))
    })?;
    reg(
        lua,
        &methods,
        "push",
        st,
        |_, st, (this, values): (Table, Variadic<Value>)| push(st, &Value::Table(this), values),
    )?;
    reg(
        lua,
        &methods,
        "provider",
        st,
        |_, st, (this, spec): (Table, Value)| {
            provider(st, &Value::Table(this.clone()), spec)?;
            Ok(this)
        },
    )?;
    let mt = lua.create_table()?;
    mt.raw_set("__index", methods)?;
    mt.raw_set(
        "__tostring",
        lua.create_function(|_, this: Table| {
            Ok(format!("mbar.item({})", this.raw_get::<String>("name")?))
        })?,
    )?;
    lua.set_named_registry_value(KEY_ITEM_MT, mt)?;

    let globals = lua.globals();
    globals.raw_set("mbar", &m)?;
    let package: Table = globals.get("package")?;
    let loaded: Table = package.get("loaded")?;
    loaded.raw_set("mbar", &m)?;
    loaded.raw_set("sketchybar", &m)?;
    Ok(m)
}
