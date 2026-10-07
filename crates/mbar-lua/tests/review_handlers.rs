//! Review regressions for the Lua item -> handler bookkeeping.
//!
//! * F6: the name -> handler map went stale after regex / `mbar.command` /
//!   external removes and renames, so a re-added item never got
//!   `script=lua:<id>` (and a renamed item shared the new handlers).
//! * F13: every `click_script = fn` (and `script = fn` on a regex selector)
//!   allocated a new handler entry that was never freed.

use std::time::Duration;

use mbar_lua::{parse_script, Host, LuaEngine};

/// Records every message; answers `--rename` with a failure when asked to.
#[derive(Default)]
struct Mock {
    messages: Vec<Vec<String>>,
    fail_rename: bool,
}

impl Host for Mock {
    fn command(&mut self, args: Vec<String>) -> String {
        let resp = match args.first().map(String::as_str) {
            Some("--rename") if self.fail_rename => format!(
                "[!] Rename: Failed to rename item: {} -> {}\n",
                args[1], args[2]
            ),
            _ => String::new(),
        };
        self.messages.push(args);
        resp
    }

    fn spawn_shell(&mut self, _cmd: String, _callback: Option<u64>) {}

    fn schedule(&mut self, _delay: Duration, _callback: u64) {}
}

fn run(src: &str) -> (LuaEngine, Mock) {
    run_with(src, Mock::default())
}

fn run_with(src: &str, mut host: Mock) -> (LuaEngine, Mock) {
    let mut engine = LuaEngine::new().unwrap();
    engine.load_string(src, "=test", &mut host).unwrap();
    (engine, host)
}

/// Handler ids assigned to `name`'s `key` (`script` / `click_script`), in order.
fn ids(host: &Mock, name: &str, key: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for msg in &host.messages {
        let mut target: Option<&str> = None;
        for (i, t) in msg.iter().enumerate() {
            if t.starts_with("--") {
                target = (t == "--set")
                    .then(|| msg.get(i + 1).map(String::as_str))
                    .flatten();
                continue;
            }
            if target != Some(name) {
                continue;
            }
            if let Some(v) = t.strip_prefix(&format!("{key}=")) {
                out.extend(parse_script(v));
            }
        }
    }
    out
}

/// Runs handler `id` for `SENDER=event` and returns the labels it set.
fn fire(engine: &mut LuaEngine, id: u64, name: &str, event: &str) -> Vec<String> {
    let mut host = Mock::default();
    let env = vec![
        ("NAME".to_string(), name.to_string()),
        ("SENDER".to_string(), event.to_string()),
    ];
    engine.run_handler(id, &env, &mut host).unwrap();
    host.messages
        .concat()
        .into_iter()
        .filter_map(|t| t.strip_prefix("label=").map(String::from))
        .collect()
}

const SUB: &str = r#"
    mbar.add("event", "foo")
    local a = mbar.add("item", "a")
    a:subscribe("foo", function(env) mbar.set(env.NAME, { label = "f1" }) end)
"#;
const RESUB: &str = r#"
    local a2 = mbar.add("item", "a")
    a2:subscribe("foo", function(env) mbar.set(env.NAME, { label = "f2" }) end)
"#;

fn assert_fresh_handler(engine: &mut LuaEngine, host: &Mock) {
    let script = ids(host, "a", "script");
    let (&first, &last) = (script.first().unwrap(), script.last().unwrap());
    assert_ne!(
        first, last,
        "re-added item must get a fresh handler: {script:?}"
    );
    // The re-added item's message carries its script.
    let last_msg = host.messages.last().unwrap();
    let token = format!("script=lua:{last}");
    assert!(last_msg.contains(&token), "{last_msg:?}");
    assert!(!engine.has_handler(first), "stale handler {first} leaked");
    assert_eq!(fire(engine, last, "a", "foo"), vec!["f2"]);
}

#[test]
fn f6_regex_remove_then_readd_gets_script() {
    let src = format!("{SUB} mbar.remove('/a/') mbar.flush() {RESUB}");
    let (mut engine, host) = run(&src);
    assert_eq!(host.messages.len(), 2);
    assert_fresh_handler(&mut engine, &host);
}

#[test]
fn f6_command_remove_then_readd_gets_script() {
    let src = format!("{SUB} mbar.command('--remove', 'a') {RESUB}");
    let (mut engine, host) = run(&src);
    assert_fresh_handler(&mut engine, &host);
}

#[test]
fn f6_command_regex_remove_then_readd_gets_script() {
    let src = format!("{SUB} mbar.command('--remove', '/^a$/') {RESUB}");
    let (mut engine, host) = run(&src);
    assert_fresh_handler(&mut engine, &host);
}

#[test]
fn f6_external_remove_then_readd_gets_script() {
    // A shell plugin removed "a" between two engine calls; the engine never saw it.
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine.load_string(SUB, "=a", &mut host).unwrap();
    engine.load_string(RESUB, "=b", &mut host).unwrap();
    assert_fresh_handler(&mut engine, &host);
}

#[test]
fn f6_external_readd_without_lua_add_still_gets_script() {
    // A shell removed and re-added "a"; Lua only subscribes again.
    let src = format!(
        "{SUB} mbar.flush() mbar.subscribe('a', 'foo', function(env) mbar.set(env.NAME, {{ label = 'f2' }}) end)"
    );
    let (mut engine, host) = run(&src);
    let script = ids(&host, "a", "script");
    assert_eq!(script.len(), 2, "{:?}", host.messages);
    assert_eq!(script[0], script[1]);
    assert_eq!(fire(&mut engine, script[1], "a", "foo"), vec!["f2"]);
}

#[test]
fn f6_rename_keeps_handlers_with_renamed_item() {
    let src = format!("{SUB} mbar.command('--rename', 'a', 'b') {RESUB}");
    let (mut engine, host) = run(&src);
    let script = ids(&host, "a", "script");
    let (old, new) = (script[0], *script.last().unwrap());
    assert_ne!(old, new);
    // "b" (the renamed item, still `script=lua:<old>`) keeps f1; "a" gets f2.
    assert_eq!(fire(&mut engine, old, "b", "foo"), vec!["f1"]);
    assert_eq!(fire(&mut engine, new, "a", "foo"), vec!["f2"]);

    // Further subscribes on "b" extend its existing handler.
    let mut host = Mock::default();
    engine
        .load_string(
            "mbar.subscribe('b', 'bar', function(env) mbar.set(env.NAME, { label = 'g' }) end)",
            "=c",
            &mut host,
        )
        .unwrap();
    assert_eq!(ids(&host, "b", "script"), vec![old]);
    assert_eq!(fire(&mut engine, old, "b", "bar"), vec!["g"]);
    assert_eq!(fire(&mut engine, old, "b", "foo"), vec!["f1"]);
}

#[test]
fn f6_failed_rename_is_not_tracked() {
    let src = format!(
        "{SUB} mbar.command('--rename', 'a', 'b') \
         mbar.subscribe('a', 'bar', function(env) mbar.set(env.NAME, {{ label = 'g' }}) end)"
    );
    let host = Mock {
        fail_rename: true,
        ..Mock::default()
    };
    let (mut engine, host) = run_with(&src, host);
    let script = ids(&host, "a", "script");
    assert_eq!(script.len(), 2);
    assert_eq!(script[0], script[1], "failed rename must keep the mapping");
    assert_eq!(fire(&mut engine, script[0], "a", "foo"), vec!["f1"]);
    assert_eq!(fire(&mut engine, script[0], "a", "bar"), vec!["g"]);
}

#[test]
fn f13_click_script_function_reuses_handler() {
    let (mut engine, host) = run(r#"
        local a = mbar.add("item", "a")
        for i = 1, 2000 do
            a:set({ click_script = function(env) mbar.set(env.NAME, { label = "c" .. i }) end })
        end
    "#);
    let click = ids(&host, "a", "click_script");
    assert_eq!(click.len(), 2000);
    assert!(click.iter().all(|&id| id == click[0]), "{:?}", &click[..3]);
    assert_eq!(
        fire(&mut engine, click[0], "a", "mouse.clicked"),
        vec!["c2000"]
    );
}

#[test]
fn f13_replacing_click_script_does_not_grow_memory() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine
        .load_string(
            r#"mbar.add("item", "a")
               mbar.set("/sp.*/", { script = function() end })"#,
            "=init",
            &mut host,
        )
        .unwrap();
    let churn = r#"
        for i = 1, 5000 do
            mbar.set("a", { click_script = function() return i end })
            mbar.set("/sp.*/", { script = function() return i end,
                                 click_script = function() return i end })
        end
        collectgarbage("collect")
        collectgarbage("collect")
    "#;
    engine
        .load_string(churn, "=warm", &mut Mock::default())
        .unwrap();
    let before = engine.used_memory();
    engine
        .load_string(churn, "=churn", &mut Mock::default())
        .unwrap();
    let after = engine.used_memory();
    assert!(
        after < before + 64 * 1024,
        "Lua memory grew from {before} to {after} bytes"
    );
}

#[test]
fn f13_regex_selector_reuses_handler() {
    let (_, host) = run(r#"
        mbar.set("/sp.*/", { script = function() end, click_script = function() end })
        mbar.set("/sp.*/", { script = function() end, click_script = function() end })
    "#);
    let script = ids(&host, "/sp.*/", "script");
    let click = ids(&host, "/sp.*/", "click_script");
    assert_eq!(script.len(), 2);
    assert_eq!(script[0], script[1]);
    assert_eq!(click.len(), 2);
    assert_eq!(click[0], click[1]);
    assert_ne!(script[0], click[0]);
}

#[test]
fn f13_click_handler_freed_on_shell_script_and_remove() {
    let (engine, host) = run(r#"
        mbar.add("item", "a", { click_script = function() end })
        mbar.set("a", { click_script = "echo hi" })
        mbar.add("item", "b", { click_script = function() end, script = function() end })
        mbar.remove("b")
    "#);
    let a = ids(&host, "a", "click_script");
    let b_click = ids(&host, "b", "click_script");
    let b_script = ids(&host, "b", "script");
    assert_eq!(a.len(), 1);
    assert!(!engine.has_handler(a[0]));
    assert!(!engine.has_handler(b_click[0]));
    assert!(!engine.has_handler(b_script[0]));
}

#[test]
fn default_click_functions_stay_distinct() {
    // Items created from an earlier default keep their own function.
    let (mut engine, host) = run(r#"
        mbar.default({ click_script = function(env) mbar.set(env.NAME, { label = "one" }) end })
        mbar.add("item", "x")
        mbar.default({ click_script = function(env) mbar.set(env.NAME, { label = "two" }) end })
        mbar.add("item", "y")
    "#);
    let flat = host.messages.concat();
    let defaults: Vec<u64> = flat
        .iter()
        .filter_map(|t| t.strip_prefix("click_script=").and_then(parse_script))
        .collect();
    assert_eq!(defaults.len(), 2);
    assert_ne!(defaults[0], defaults[1]);
    assert_eq!(
        fire(&mut engine, defaults[0], "x", "mouse.clicked"),
        vec!["one"]
    );
    assert_eq!(
        fire(&mut engine, defaults[1], "y", "mouse.clicked"),
        vec!["two"]
    );
}
