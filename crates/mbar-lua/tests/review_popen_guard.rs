//! Review regression F8: a blocking `io.popen` / `os.execute` of the `mbar` client
//! from the daemon's own Lua self-deadlocked until the client timeout. The engine
//! now exports `MBAR_LUA_SYNC=<bar>` into those shells (the client refuses at once,
//! see `crates/mbar/tests/review_lua_popen.rs`), without changing what the commands
//! do or return.

use std::time::Duration;

use mbar_lua::{Host, LuaEngine, SYNC_SHELL_ENV};

#[derive(Default)]
struct Mock {
    messages: Vec<Vec<String>>,
}

impl Host for Mock {
    fn command(&mut self, args: Vec<String>) -> String {
        self.messages.push(args);
        String::new()
    }

    fn spawn_shell(&mut self, _cmd: String, _callback: Option<u64>) {}

    fn schedule(&mut self, _delay: Duration, _callback: u64) {}
}

/// Runs `src` and returns the label values it set (`mbar.set("x", {label=...})`).
fn labels(bar: Option<&str>, src: &str) -> Vec<String> {
    let mut host = Mock::default();
    let mut engine = LuaEngine::new().unwrap();
    engine.set_bar_name(bar);
    engine.load_string(src, "=test", &mut host).unwrap();
    host.messages
        .iter()
        .flat_map(|m| m.iter())
        .filter_map(|t| t.strip_prefix("label=").map(str::to_owned))
        .collect()
}

#[test]
fn env_name_is_stable() {
    assert_eq!(SYNC_SHELL_ENV, "MBAR_LUA_SYNC");
}

#[test]
fn popen_and_execute_export_the_bar_name() {
    let got = labels(
        Some("my 'bar'"),
        r#"
local p = io.popen('printf "%s" "$MBAR_LUA_SYNC"')
local out = p:read("a"); p:close()
local ok, how, code = os.execute([[test "$MBAR_LUA_SYNC" = "my 'bar'"]])
mbar.set("x", { label = out .. "|" .. tostring(ok) .. "|" .. how .. "|" .. code })
"#,
    );
    assert_eq!(got, vec!["my 'bar'|true|exit|0".to_string()]);
}

#[test]
fn without_a_bar_name_commands_run_unchanged() {
    let got = labels(
        None,
        r#"
local p = io.popen('printf "[%s]" "${MBAR_LUA_SYNC-unset}"')
mbar.set("x", { label = p:read("a") }); p:close()
"#,
    );
    assert_eq!(got, vec!["[unset]".to_string()]);
}

#[test]
fn results_and_modes_pass_through() {
    let got = labels(
        Some("mbar"),
        r#"
local ok, how, code = os.execute("exit 3")
local shell = os.execute()
local w = io.popen("cat >/dev/null", "w")
local wrote = w:write("data") ~= nil
local closed = w:close()
local multi = io.popen("echo a; echo b"):read("a")
mbar.set("x", { label = table.concat({ tostring(ok), how, code, tostring(shell),
  tostring(wrote), tostring(closed), (multi:gsub("\n", ",")) }, "|") })
"#,
    );
    assert_eq!(got, vec!["nil|exit|3|true|true|true|a,b,".to_string()]);
}

#[test]
fn bad_arguments_still_raise() {
    let mut host = Mock::default();
    let mut engine = LuaEngine::new().unwrap();
    engine.set_bar_name(Some("mbar"));
    assert!(engine
        .load_string("io.popen({})", "=test", &mut host)
        .is_err());
}
