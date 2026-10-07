//! Guard against the self-deadlock of a blocking `io.popen` / `os.execute` of the
//! `mbar` client from inside the daemon's own Lua.
//!
//! Lua runs on the daemon's main thread, which is also the only thread that answers
//! IPC requests. A synchronous shell command that sends a message to the same bar
//! therefore waits for a reply that cannot come until Lua returns: the client gives
//! up after its timeout and prints nothing, and the whole bar is frozen meanwhile.
//!
//! The wrapped `io.popen` / `os.execute` export [`SYNC_SHELL_ENV`] (the IPC bar name
//! of the daemon) into the shell they start. The `mbar` client refuses at once, with
//! an error, to message that bar while the variable names it. The variable is only
//! set in those children's environment, never in the daemon's own process
//! environment, so `mbar.exec`, item scripts and other children are unaffected.

use mlua::{Function, Lua, MultiValue, Result, Table, Value};

use crate::api::Shared;

/// Environment variable exported into commands started by a blocking `io.popen` /
/// `os.execute` of the daemon's Lua. Its value is the IPC bar name of the daemon.
pub const SYNC_SHELL_ENV: &str = "MBAR_LUA_SYNC";

/// Quotes `s` for a POSIX shell (single quotes).
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The shell prefix that exports the marker for `bar`.
pub(crate) fn marker_prefix(bar: &str) -> String {
    format!("export {SYNC_SHELL_ENV}={}; ", sh_quote(bar))
}

/// Replaces `table[name]` by a wrapper that prefixes its first argument (the shell
/// command) with the marker export when a bar name is set. Other arguments and all
/// results pass through unchanged; a call without a string command (e.g.
/// `os.execute()`) is forwarded as is.
fn wrap(lua: &Lua, st: &Shared, table: &Table, name: &str) -> Result<()> {
    let Some(orig) = table.raw_get::<Option<Function>>(name)? else {
        return Ok(());
    };
    let st = st.clone();
    let f = lua.create_function(move |lua, mut args: MultiValue| {
        let marker = st.borrow().shell_marker.clone();
        if let (Some(bar), Some(Value::String(cmd))) = (marker, args.front()) {
            let mut full = marker_prefix(&bar).into_bytes();
            full.extend_from_slice(&cmd.as_bytes());
            args[0] = Value::String(lua.create_string(&full)?);
        }
        orig.call::<MultiValue>(args)
    })?;
    table.raw_set(name, f)
}

/// Wraps `io.popen` and `os.execute` (when the libraries are loaded).
pub(crate) fn install(lua: &Lua, st: &Shared) -> Result<()> {
    let globals = lua.globals();
    if let Some(io) = globals.raw_get::<Option<Table>>("io")? {
        wrap(lua, st, &io, "popen")?;
    }
    if let Some(os) = globals.raw_get::<Option<Table>>("os")? {
        wrap(lua, st, &os, "execute")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(marker_prefix("mbar"), "export MBAR_LUA_SYNC='mbar'; ");
        assert_eq!(marker_prefix("it's"), r"export MBAR_LUA_SYNC='it'\''s'; ");
    }
}
