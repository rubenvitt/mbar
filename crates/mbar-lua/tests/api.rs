use std::collections::HashMap;
use std::time::Duration;

use mbar_lua::{parse_script, AerospaceResult, Host, LuaEngine, AEROSPACE_CARRIER};

/// Records every message; answers `--query` from a table.
#[derive(Default)]
struct Mock {
    messages: Vec<Vec<String>>,
    queries: HashMap<String, String>,
    spawned: Vec<(String, Option<u64>)>,
    scheduled: Vec<(Duration, u64)>,
    aerospace: Vec<(Vec<String>, Option<u64>)>,
}

impl Host for Mock {
    fn command(&mut self, args: Vec<String>) -> String {
        let resp = if args.first().map(String::as_str) == Some("--query") {
            self.queries
                .get(&args[1..].join(" "))
                .cloned()
                .unwrap_or_else(|| format!("[!] Query: Invalid query '{}'\n", args[1..].join(" ")))
        } else {
            String::new()
        };
        self.messages.push(args);
        resp
    }

    fn spawn_shell(&mut self, cmd: String, callback: Option<u64>) {
        self.spawned.push((cmd, callback));
    }

    fn schedule(&mut self, delay: Duration, callback: u64) {
        self.scheduled.push((delay, callback));
    }

    fn aerospace(&mut self, args: Vec<String>, callback: Option<u64>) {
        self.aerospace.push((args, callback));
    }
}

fn argv(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

fn run(src: &str) -> (LuaEngine, Mock) {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine.load_string(src, "=test", &mut host).unwrap();
    (engine, host)
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn flattening_rules() {
    let (_, host) = run(r#"
        mbar.add("item", "clock", {
            position = "right",
            update_freq = 10,
            width = 1.5,
            y_offset = -2,
            space = { 1, 2 },
            icon = { string = "", font = { family = "Hack Nerd Font", style = "Bold", size = 14.0 } },
            label = { color = 0xffffffff, drawing = false, padding_left = 4.0 },
            background = { color = 0x40ffffff, image = { "app.Safari", scale = 0.5 }, drawing = true },
        })
    "#);
    assert_eq!(
        host.messages,
        vec![argv(&[
            "--add",
            "item",
            "clock",
            "right",
            "--set",
            "clock",
            "background.color=0x40ffffff",
            "background.drawing=on",
            "background.image=app.Safari",
            "background.image.scale=0.5",
            "icon.font.family=Hack Nerd Font",
            "icon.font.size=14",
            "icon.font.style=Bold",
            "icon.string=",
            "label.color=0xffffffff",
            "label.drawing=off",
            "label.padding_left=4",
            "space=1,2",
            "update_freq=10",
            "width=1.5",
            "y_offset=-2",
        ])]
    );
}

#[test]
fn add_variants() {
    let (_, host) = run(r#"
        local a = mbar.add("item", "a", "right", { label = "x" })
        assert(a.name == "a")
        mbar.add("item", "b")
        mbar.add("graph", "g", 42, { position = "right", graph = { color = 0xff00ff00 } })
        mbar.add("slider", "s", "left", 100)
        mbar.add("space", "space.1", { space = 1, icon = "1" })
        mbar.add("alias", "Control Center,Battery", { position = "right" })
        mbar.add("item", "p", { position = "popup." .. a.name })
        mbar.add("bracket", "br", { a, "b" }, { background = { color = 0xff000000 } })
        mbar.add("event", "my_event")
        mbar.add("event", "theme", "AppleInterfaceThemeChangedNotification")
        local anon = mbar.add("item", { width = 5 })
        assert(anon.name:find("^__mbar%.item%.%d+$"), anon.name)
    "#);
    assert_eq!(host.messages.len(), 1, "config is sent as one batch");
    let expected = argv(&[
        "--add",
        "item",
        "a",
        "right",
        "--set",
        "a",
        "label=x", //
        "--add",
        "item",
        "b",
        "left", //
        "--add",
        "graph",
        "g",
        "right",
        "42",
        "--set",
        "g",
        "graph.color=0xff00ff00", //
        "--add",
        "slider",
        "s",
        "left",
        "100", //
        "--add",
        "space",
        "space.1",
        "left",
        "--set",
        "space.1",
        "icon=1",
        "space=1", //
        "--add",
        "alias",
        "Control Center,Battery",
        "right", //
        "--add",
        "item",
        "p",
        "popup.a", //
        "--add",
        "bracket",
        "br",
        "a",
        "b",
        "--set",
        "br",
        "background.color=0xff000000", //
        "--add",
        "event",
        "my_event", //
        "--add",
        "event",
        "theme",
        "AppleInterfaceThemeChangedNotification", //
        "--add",
        "item",
        "__mbar.item.1",
        "left",
        "--set",
        "__mbar.item.1",
        "width=5",
    ]);
    assert_eq!(host.messages[0], expected);
}

#[test]
fn set_bar_default_remove_and_empty_tables() {
    let (_, host) = run(r#"
        mbar.bar({ height = 40, color = 0x40000000, position = "top", topmost = "window" })
        mbar.default({ padding_left = 5, icon = { color = 0xffffffff } })
        mbar.set("/space\\..*/", { label = { drawing = false } })
        mbar.set("x", {})      -- no pairs: nothing is sent (would swallow the next command)
        mbar.bar({})
        mbar.remove("x")
        mbar.hotload(true)
        mbar.update()
        mbar.menu(0)
        mbar.trigger("my_event", { INFO = { a = 1 }, FOO = "bar", N = 3 })
        mbar.push("g", 0.5, { 0.25, 1 })
        mbar.provider("cpu", { "cpu", format = "{percent}%", freq = 2 })
        mbar.provider("clock", "clock")
    "#);
    assert_eq!(
        host.messages,
        vec![argv(&[
            "--bar",
            "color=0x40000000",
            "height=40",
            "position=top",
            "topmost=window",
            "--default",
            "icon.color=0xffffffff",
            "padding_left=5",
            "--set",
            "/space\\..*/",
            "label.drawing=off",
            "--remove",
            "x",
            "--hotload",
            "on",
            "--update",
            "--menu",
            "0",
            "--trigger",
            "my_event",
            "FOO=bar",
            "INFO={\"a\":1}",
            "N=3",
            "--push",
            "g",
            "0.5",
            "0.25",
            "1",
            "--set",
            "cpu",
            "provider=cpu",
            "provider.format={percent}%",
            "provider.freq=2",
            "--set",
            "clock",
            "provider=clock",
        ])]
    );
}

#[test]
fn query_flushes_and_decodes() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    host.queries.insert(
        "bar".into(),
        "{\n\t\"position\": \"top\",\n\t\"height\": 40,\n\t\"items\": [\"a\", \"b\"]\n}\n".into(),
    );
    host.queries.insert(
        "item a".into(),
        "{ \"name\": \"a\", \"label\": { \"value\": \"hi\" } }".into(),
    );
    engine
        .load_string(
            r#"
            local a = mbar.add("item", "a", { label = "hi" })
            local bar = mbar.query("bar")
            assert(bar.position == "top" and bar.height == 40)
            assert(#bar.items == 2 and bar.items[2] == "b")
            mbar.set("a", { width = 1 })
            local q = a:query()
            assert(q.label.value == "hi")
            local none, msg = mbar.query("nope")
            assert(none == nil and msg:find("Invalid query"))
            local d = mbar.json.decode('{"x":[1,2.5,true]}')
            assert(d.x[1] == 1 and d.x[2] == 2.5 and d.x[3] == true)
            assert(mbar.json.encode({1, 2}) == "[1,2]")
        "#,
            "=test",
            &mut host,
        )
        .unwrap();
    assert_eq!(
        host.messages,
        vec![
            argv(&["--add", "item", "a", "left", "--set", "a", "label=hi"]),
            argv(&["--query", "bar"]),
            argv(&["--set", "a", "width=1"]),
            argv(&["--query", "item", "a"]),
            argv(&["--query", "nope"]),
        ]
    );
}

#[test]
fn begin_end_config_flushes() {
    let (_, host) = run(r#"
        mbar.begin_config()
        mbar.add("item", "a")
        mbar.add("item", "b")
        mbar.end_config()
        mbar.set("a", { label = "x" })
    "#);
    assert_eq!(
        host.messages,
        vec![
            argv(&["--add", "item", "a", "left", "--add", "item", "b", "left"]),
            argv(&["--set", "a", "label=x"]),
        ]
    );
}

#[test]
fn animate_groups_commands() {
    let (_, host) = run(r#"
        mbar.set("a", { width = 1 })
        mbar.animate("tanh", 30, function()
            mbar.set("a", { y_offset = 10 })
            mbar.bar({ height = 40 })
            mbar.animate("sin", 15, function()
                mbar.set("b", { y_offset = 0 })
            end)
            mbar.set("c", { y_offset = 3 })
        end)
        mbar.set("b", { label = "after" })
        mbar.animate("linear", 10, function() end) -- empty: nothing sent
    "#);
    assert_eq!(
        host.messages,
        vec![
            argv(&["--set", "a", "width=1"]),
            argv(&[
                "--animate",
                "tanh",
                "30",
                "--set",
                "a",
                "y_offset=10",
                "--bar",
                "height=40",
                "--animate",
                "sin",
                "15",
                "--set",
                "b",
                "y_offset=0",
                "--animate",
                "tanh",
                "30",
                "--set",
                "c",
                "y_offset=3",
            ]),
            argv(&["--set", "b", "label=after"]),
        ]
    );
}

#[test]
fn subscribe_dispatch_by_event() {
    let (mut engine, mut host) = run(r#"
        local x = mbar.add("item", "x", "right")
        x:subscribe("mouse.clicked", function(env)
            mbar.set(env.NAME, { label = "clicked " .. env.BUTTON .. " " .. env.MODIFIER })
        end)
        x:subscribe({ "front_app_switched", "routine", "forced" }, function(env)
            mbar.set(env.NAME, { label = env.SENDER .. ":" .. (env.INFO or "") })
        end)
        mbar.subscribe("x", function(env)
            mbar.set(env.NAME, { label = "any " .. env.SENDER })
        end)
        mbar.subscribe("x", "mouse.entered mouse.exited", function(env)
            mbar.set(env.NAME, { background = { drawing = env.SENDER == "mouse.entered" } })
        end)
    "#);
    assert_eq!(
        host.messages,
        vec![argv(&[
            "--add",
            "item",
            "x",
            "right",
            "--set",
            "x",
            "script=lua:1",
            "--subscribe",
            "x",
            "mouse.clicked",
            // every subscribe re-asserts the (idempotent) script
            "--set",
            "x",
            "script=lua:1",
            "--subscribe",
            "x",
            "front_app_switched",
            "--set",
            "x",
            "script=lua:1",
            "--set",
            "x",
            "script=lua:1",
            "--subscribe",
            "x",
            "mouse.entered",
            "mouse.exited",
        ])]
    );
    assert_eq!(parse_script("lua:1"), Some(1));
    assert!(engine.has_handler(1));
    host.messages.clear();

    let cases: [(&[(&str, &str)], &str); 5] = [
        (
            &[
                ("NAME", "x"),
                ("SENDER", "mouse.clicked"),
                ("BUTTON", "left"),
                ("MODIFIER", "shift,cmd"),
            ],
            "label=clicked left shift,cmd",
        ),
        (
            &[
                ("NAME", "x"),
                ("SENDER", "front_app_switched"),
                ("INFO", "Finder"),
            ],
            "label=front_app_switched:Finder",
        ),
        (&[("NAME", "x"), ("SENDER", "routine")], "label=routine:"),
        (
            &[("NAME", "x"), ("SENDER", "system_woke")],
            "label=any system_woke",
        ),
        (
            &[("NAME", "x"), ("SENDER", "mouse.entered")],
            "background.drawing=on",
        ),
    ];
    for (e, expected) in cases {
        engine.run_handler(1, &env(e), &mut host).unwrap();
        assert_eq!(
            host.messages.pop().unwrap(),
            argv(&["--set", "x", expected])
        );
    }
    // Unknown handler id: ignored.
    engine
        .run_handler(99, &env(&[("SENDER", "x")]), &mut host)
        .unwrap();
    assert!(host.messages.is_empty());
    assert_eq!(engine.stats().callbacks, 6);
}

#[test]
fn handler_env_and_info_json() {
    let (mut engine, mut host) = run(r#"
        mbar.add("item", "s", { space = 2 })
        mbar.subscribe("s", { "mouse.clicked", "space_windows_change", "mouse.scrolled" }, function(env)
            if env.SENDER == "mouse.clicked" then
                assert(env.info.button == "left" and env.info.modfier_code == 0)
                mbar.set(env.NAME, { label = env.BUTTON .. "/" .. env.MODIFIER .. "/" .. env.SID .. "/" .. env.SELECTED })
            elseif env.SENDER == "space_windows_change" then
                local n = 0
                for _, c in pairs(env.info.apps) do n = n + c end
                mbar.set(env.NAME, { label = env.info.space .. ":" .. n })
            else
                assert(env.info == nil)
                mbar.set(env.NAME, { label = "scroll " .. env.SCROLL_DELTA })
            end
        end)
    "#);
    host.messages.clear();
    let click_info = "{\n\t\"button\": \"left\",\n\t\"button_code\": 0,\n\t\"modifier\": \"none\",\n\t\"modfier_code\": 0\n}\n";
    engine
        .run_handler(
            1,
            &env(&[
                ("NAME", "s"),
                ("SENDER", "mouse.clicked"),
                ("INFO", click_info),
                ("BUTTON", "left"),
                ("MODIFIER", "none"),
                ("SID", "2"),
                ("DID", "1"),
                ("SELECTED", "true"),
            ]),
            &mut host,
        )
        .unwrap();
    engine
        .run_handler(
            1,
            &env(&[
                ("NAME", "s"),
                ("SENDER", "space_windows_change"),
                (
                    "INFO",
                    "{\"space\": 2, \"apps\": {\"Finder\": 2, \"Safari\": 1}}",
                ),
            ]),
            &mut host,
        )
        .unwrap();
    engine
        .run_handler(
            1,
            &env(&[
                ("NAME", "s"),
                ("SENDER", "mouse.scrolled"),
                ("SCROLL_DELTA", "-3"),
                ("INFO", "42"),
            ]),
            &mut host,
        )
        .unwrap();
    assert_eq!(
        host.messages,
        vec![
            argv(&["--set", "s", "label=left/none/2/true"]),
            argv(&["--set", "s", "label=2:3"]),
            argv(&["--set", "s", "label=scroll -3"]),
        ]
    );
}

#[test]
fn script_functions_in_props() {
    let (mut engine, mut host) = run(r#"
        mbar.add("item", "c", {
            script = function(env) mbar.set(env.NAME, { label = "script " .. env.SENDER }) end,
            click_script = function(env) mbar.set(env.NAME, { label = "click " .. env.BUTTON }) end,
            update_freq = 5,
        })
        -- reuses the handler created by script = fn (keys are processed in
        -- sorted order, so click_script gets id 1 and script id 2)
        mbar.subscribe("c", "volume_change", function(env) mbar.set("c", { label = "vol " .. env.INFO }) end)
    "#);
    assert_eq!(
        host.messages,
        vec![argv(&[
            "--add",
            "item",
            "c",
            "left",
            "--set",
            "c",
            "click_script=lua:1",
            "script=lua:2",
            "update_freq=5",
            "--set",
            "c",
            "script=lua:2",
            "--subscribe",
            "c",
            "volume_change",
        ])]
    );
    host.messages.clear();
    engine
        .run_handler(2, &env(&[("NAME", "c"), ("SENDER", "routine")]), &mut host)
        .unwrap();
    engine
        .run_handler(
            2,
            &env(&[("NAME", "c"), ("SENDER", "volume_change"), ("INFO", "40")]),
            &mut host,
        )
        .unwrap();
    engine
        .run_handler(1, &env(&[("NAME", "c"), ("BUTTON", "right")]), &mut host)
        .unwrap();
    assert_eq!(
        host.messages,
        vec![
            argv(&["--set", "c", "label=script routine"]),
            argv(&["--set", "c", "label=vol 40"]),
            argv(&["--set", "c", "label=click right"]),
        ]
    );
}

#[test]
fn exec_and_delay_callbacks() {
    let (mut engine, mut host) = run(r#"
        mbar.add("item", "w")
        mbar.exec("echo hi", function(result, raw)
            mbar.set("w", { label = type(result) == "table" and result.ssid or result })
        end)
        mbar.exec("true")
        mbar.delay(1.5, function() mbar.set("w", { label = "later" }) end)
    "#);
    // The queue is flushed before the shell command runs.
    assert_eq!(host.messages, vec![argv(&["--add", "item", "w", "left"])]);
    assert_eq!(
        host.spawned,
        vec![("echo hi".to_string(), Some(1)), ("true".to_string(), None)]
    );
    assert_eq!(host.scheduled, vec![(Duration::from_millis(1500), 2)]);
    host.messages.clear();

    engine
        .exec_finished(1, "{\"ssid\": \"home\"}".into(), &mut host)
        .unwrap();
    engine.timer_fired(2, &mut host).unwrap();
    // Callbacks are one-shot.
    engine.exec_finished(1, "plain".into(), &mut host).unwrap();
    engine.timer_fired(2, &mut host).unwrap();
    assert_eq!(
        host.messages,
        vec![
            argv(&["--set", "w", "label=home"]),
            argv(&["--set", "w", "label=later"]),
        ]
    );
}

#[test]
fn exec_plain_output() {
    let (mut engine, mut host) = run(r#"
        mbar.exec("date", function(result, raw)
            assert(result == raw)
            mbar.set("d", { label = result })
        end)
    "#);
    engine.exec_finished(1, "Mon\n".into(), &mut host).unwrap();
    assert_eq!(host.messages, vec![argv(&["--set", "d", "label=Mon\n"])]);
}

#[test]
fn errors_still_flush_and_are_reported() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    let err = engine
        .load_string(
            r#"
            mbar.set("a", { label = "before" })
            error("boom")
        "#,
            "=cfg",
            &mut host,
        )
        .unwrap_err();
    assert!(err.to_string().contains("boom"), "{err}");
    assert_eq!(host.messages, vec![argv(&["--set", "a", "label=before"])]);

    host.messages.clear();
    for bad in [
        r#"mbar.add("item", "")"#,
        r#"mbar.set("a", { label = { 1, {} } })"#,
        r#"mbar.set("a", { width = function() end })"#,
        r#"mbar.set("a", { 1, 2 })"#,
        r#"mbar.subscribe("a", "x")"#,
        r#"mbar.delay(1/0, function() end)"#,
        r#"mbar.trigger("")"#,
    ] {
        assert!(engine.load_string(bad, "=bad", &mut host).is_err(), "{bad}");
    }
    assert!(host.messages.is_empty(), "{:?}", host.messages);
}

#[test]
fn borders_emits_jankyborders_syntax() {
    let (_, host) = run(r#"
        mbar.borders({
            active_color = 0xffe1e3e4,
            inactive_color = "0xff494d64",
            background_color = 0x302c2e34,
            width = 5.0,
            style = "round",
            hidpi = true,
            ax_focus = false,
            order = "above",
            blacklist = { "Safari", "kitty" },
        })
        mbar.borders({
            active_color = { glow = 0xd2e1e3e4 },
            inactive_color = { gradient = { top_left = 0xffff0000, bottom_right = 0x0000ff00 } },
            background_color = { gradient = { top_right = 0x11223344, bottom_left = "0x55667788" } },
            width = 4.5,
            whitelist = {},
        })
        mbar.borders({ drawing = false })
        mbar.borders({ apply_to = 4242, active_color = 0xffff0000 })
        mbar.borders({ ["apply-to"] = 7, width = 2 })
        mbar.borders({})    -- no pairs: nothing is sent
    "#);
    assert_eq!(
        host.messages,
        vec![argv(&[
            "--borders",
            "active_color=0xffe1e3e4",
            "ax_focus=off",
            "background_color=0x302c2e34",
            "blacklist=Safari,kitty",
            "hidpi=on",
            "inactive_color=0xff494d64",
            "order=above",
            "style=round",
            "width=5",
            "--borders",
            "active_color=glow(0xd2e1e3e4)",
            "background_color=gradient(top_right=0x11223344,bottom_left=0x55667788)",
            "inactive_color=gradient(top_left=0xffff0000,bottom_right=0x0000ff00)",
            "whitelist=",
            "width=4.5",
            "--borders",
            "drawing=off",
            "--borders",
            "active_color=0xffff0000",
            "apply-to=4242",
            "--borders",
            "apply-to=7",
            "width=2",
        ])]
    );
    // Every emitted pair is accepted by the core's JankyBorders parser.
    let pairs: Vec<String> = host.messages[0]
        .iter()
        .filter(|a| *a != "--borders")
        .cloned()
        .collect();
    let (valid, errors) = mbar_core::borders::validate_args(&pairs);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(valid, pairs);
    let mut settings = mbar_core::borders::BorderSettings::default();
    for p in &pairs[..9] {
        mbar_core::borders::parse_arg(&mut settings, p).unwrap();
    }
    assert_eq!(settings.blacklist, vec!["Safari", "kitty"]);
    assert_eq!(settings.width, 5.0);
    assert!(settings.hidpi);
    assert_eq!(settings.ax_focus, Some(false));
}

#[test]
fn borders_rejects_malformed_tables() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    for bad in [
        r#"mbar.borders({ 1, 2 })"#,
        r#"mbar.borders({ active_color = {} })"#,
        r#"mbar.borders({ active_color = { glow = 1, gradient = {} } })"#,
        r#"mbar.borders({ active_color = { glow = true } })"#,
        r#"mbar.borders({ active_color = { shimmer = 0xff000000 } })"#,
        r#"mbar.borders({ active_color = { gradient = { top_left = 1, bottom_left = 2 } } })"#,
        r#"mbar.borders({ active_color = { gradient = { top_left = 1, bottom_right = 2, x = 3 } } })"#,
        r#"mbar.borders({ blacklist = { app = "Safari" } })"#,
        r#"mbar.borders({ style = { "round" } })"#,
        r#"mbar.borders({ width = function() end })"#,
    ] {
        assert!(engine.load_string(bad, "=bad", &mut host).is_err(), "{bad}");
    }
    assert!(host.messages.is_empty(), "{:?}", host.messages);
}

#[test]
fn require_and_config_dir() {
    let dir = std::env::temp_dir().join(format!("mbar-lua-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("items")).unwrap();
    std::fs::write(
        dir.join("init.lua"),
        r#"
        local sbar = require("sketchybar")
        assert(sbar == mbar and require("mbar") == mbar)
        local colors = require("colors")
        require("items")
        mbar.bar({ color = colors.bar, height = 32 })
        mbar.set("cfg", { label = CONFIG_DIR == mbar.config_dir and "ok" or "bad" })
        mbar.event_loop()
        "#,
    )
    .unwrap();
    std::fs::write(dir.join("colors.lua"), "return { bar = 0xff1e1e2e }").unwrap();
    std::fs::write(
        dir.join("items/init.lua"),
        r#"mbar.add("item", "cfg", "right")"#,
    )
    .unwrap();

    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine.load_file(&dir.join("init.lua"), &mut host).unwrap();
    assert_eq!(
        host.messages,
        vec![argv(&[
            "--add",
            "item",
            "cfg",
            "right",
            "--bar",
            "color=0xff1e1e2e",
            "height=32",
            "--set",
            "cfg",
            "label=ok",
        ])]
    );
    std::fs::remove_dir_all(&dir).unwrap();

    let missing = engine.load_file(&dir.join("init.lua"), &mut host);
    assert!(matches!(missing, Err(mbar_lua::Error::Io { .. })));
}

#[test]
fn command_escape_hatch_and_remove_forgets_handler() {
    let (mut engine, mut host) = run(r#"
        local x = mbar.add("item", "x")
        x:subscribe("mouse.clicked", function() end)
        local r = mbar.command("--move", "x", "before", "y")
        assert(r == "")
        x:remove()
        mbar.add("item", "x")
        x:subscribe("mouse.clicked", function() end) -- new handler id after remove
    "#);
    assert_eq!(
        host.messages,
        vec![
            argv(&[
                "--add",
                "item",
                "x",
                "left",
                "--set",
                "x",
                "script=lua:1",
                "--subscribe",
                "x",
                "mouse.clicked",
            ]),
            argv(&["--move", "x", "before", "y"]),
            argv(&[
                "--remove",
                "x",
                "--add",
                "item",
                "x",
                "left",
                "--set",
                "x",
                "script=lua:2",
                "--subscribe",
                "x",
                "mouse.clicked",
            ]),
        ]
    );
    assert!(!engine.has_handler(1));
    assert!(engine.has_handler(2));
    host.messages.clear();
    engine
        .run_handler(1, &env(&[("SENDER", "mouse.clicked")]), &mut host)
        .unwrap();
    assert!(host.messages.is_empty());
}

#[test]
fn parse_script_values() {
    assert_eq!(parse_script("lua:42"), Some(42));
    assert_eq!(parse_script("lua:x"), None);
    assert_eq!(parse_script("~/plugins/clock.sh"), None);
}

fn aero(exit_code: i32, stdout: &str, stderr: &str) -> AerospaceResult {
    AerospaceResult {
        exit_code,
        stdout: stdout.into(),
        stderr: stderr.into(),
    }
}

/// `mbar.aerospace.run` / `query` hand the command to the host (numbers converted, no
/// daemon message), and their callbacks get the documented values.
#[test]
fn aerospace_run_and_query() {
    let (mut engine, mut host) = run(r#"
        mbar.aerospace.run({ "workspace", 3 })
        mbar.aerospace.run({ "list-workspaces", "--all" }, function(r)
            mbar.set("r", { label = r.exit_code .. "|" .. r.stdout .. "|" .. r.stderr })
        end)
        mbar.aerospace.query({ "list-windows", "--json" }, function(v, err)
            if err then
                mbar.set("q", { label = "err:" .. err })
            else
                mbar.set("q", { label = v[1]["app-name"] .. #v })
            end
        end)
    "#);
    assert!(host.messages.is_empty(), "{:?}", host.messages);
    assert_eq!(
        host.aerospace,
        vec![
            (argv(&["workspace", "3"]), None),
            (argv(&["list-workspaces", "--all"]), Some(1)),
            (argv(&["list-windows", "--json"]), Some(2)),
        ]
    );

    engine
        .aerospace_finished(1, aero(0, "1\n2\n", ""), &mut host)
        .unwrap();
    engine
        .aerospace_finished(2, aero(0, r#"[{"app-name":"Finder"}]"#, ""), &mut host)
        .unwrap();
    // One-shot: a second completion is ignored.
    engine
        .aerospace_finished(2, aero(0, "[]", ""), &mut host)
        .unwrap();
    engine
        .aerospace_finished(99, aero(0, "", ""), &mut host)
        .unwrap();
    assert_eq!(
        host.messages,
        vec![
            argv(&["--set", "r", "label=0|1\n2\n|"]),
            argv(&["--set", "q", "label=Finder1"]),
        ]
    );
}

#[test]
fn aerospace_query_errors() {
    let (mut engine, mut host) = run(r#"
        for i = 1, 4 do
            mbar.aerospace.query({ "list-windows", "--json" }, function(v, err)
                assert(v == nil)
                mbar.set("q" .. i, { label = err })
            end)
        end
        mbar.aerospace.run({ "x" }, function(r)
            mbar.set("r", { label = r.exit_code .. ":" .. r.stderr })
        end)
    "#);
    engine
        .aerospace_finished(1, aero(2, "", "No window is focused\n"), &mut host)
        .unwrap();
    engine
        .aerospace_finished(2, aero(0, "not json", ""), &mut host)
        .unwrap();
    engine
        .aerospace_finished(3, aero(1, "", ""), &mut host)
        .unwrap();
    engine
        .aerospace_finished(
            4,
            aero(-1, "", "AeroSpace is not running: no socket"),
            &mut host,
        )
        .unwrap();
    engine
        .aerospace_finished(5, aero(-1, "", "AeroSpace is not running"), &mut host)
        .unwrap();
    let labels: Vec<&str> = host
        .messages
        .iter()
        .map(|m| m[2].strip_prefix("label=").unwrap())
        .collect();
    assert_eq!(labels[0], "No window is focused");
    assert!(
        labels[1].starts_with("aerospace output is not JSON"),
        "{labels:?}"
    );
    assert_eq!(labels[2], "aerospace exited with code 1");
    assert_eq!(labels[3], "AeroSpace is not running: no socket");
    assert_eq!(labels[4], "-1:AeroSpace is not running");
}

#[test]
fn aerospace_argument_validation() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    for (bad, msg) in [
        (r#"mbar.aerospace.run("workspace 3")"#, "non-empty list"),
        (r#"mbar.aerospace.run({})"#, "non-empty list"),
        (
            r#"mbar.aerospace.run({ "a", true })"#,
            "element 2 is a boolean",
        ),
        (r#"mbar.aerospace.run({ "a", x = "b" })"#, "non-empty list"),
        (r#"mbar.aerospace.run({ "a", nil, "c" })"#, "non-empty list"),
        (
            r#"mbar.aerospace.run({ "a" }, "nope")"#,
            "fn must be a function",
        ),
        (r#"mbar.aerospace.query({ "a" })"#, "fn must be a function"),
        (
            r#"mbar.aerospace.query(nil, function() end)"#,
            "non-empty list",
        ),
        (
            r#"mbar.aerospace.on("space_change", function() end)"#,
            "unknown event 'space_change'",
        ),
        (
            r#"mbar.aerospace.on("aerospace_nope", function() end)"#,
            "unknown event",
        ),
        (
            r#"mbar.aerospace.on(1, function() end)"#,
            "event must be a string",
        ),
        (
            r#"mbar.aerospace.on("mode_change")"#,
            "fn must be a function",
        ),
    ] {
        let e = engine.load_string(bad, "=bad", &mut host).unwrap_err();
        assert!(e.to_string().contains(msg), "{bad}: {e}");
    }
    assert!(host.messages.is_empty(), "{:?}", host.messages);
    assert!(host.aerospace.is_empty(), "{:?}", host.aerospace);
}

/// `mbar.aerospace.on`: one hidden carrier item, subscribed once per event; every
/// function registered for an event runs, in order, with the handler env.
#[test]
fn aerospace_on_uses_one_carrier_item() {
    let (mut engine, mut host) = run(r#"
        mbar.aerospace.on("workspace_change", function(env)
            mbar.set("a", { label = env.FOCUSED_WORKSPACE .. "/" .. env.info.prev_workspace })
        end)
        mbar.aerospace.on("aerospace_workspace_change", function(env)
            mbar.set("b", { label = env.NAME })
        end)
        mbar.aerospace.on("mode_change", function(env)
            mbar.set("m", { label = env.MODE })
        end)
    "#);
    let c = AEROSPACE_CARRIER;
    assert_eq!(
        host.messages,
        vec![argv(&[
            "--add",
            "item",
            c,
            "left",
            "--set",
            c,
            "drawing=off",
            "--set",
            c,
            "script=lua:1",
            "--subscribe",
            c,
            "aerospace_workspace_change",
            "--set",
            c,
            "script=lua:1",
            "--subscribe",
            c,
            "aerospace_mode_change",
        ])]
    );
    assert!(
        host.aerospace.is_empty(),
        "the subscription starts the connection"
    );
    host.messages.clear();

    engine
        .run_handler(
            1,
            &env(&[
                ("NAME", c),
                ("SENDER", "aerospace_workspace_change"),
                ("INFO", r#"{"focused_workspace":"2","prev_workspace":"1"}"#),
                ("FOCUSED_WORKSPACE", "2"),
                ("PREV_WORKSPACE", "1"),
            ]),
            &mut host,
        )
        .unwrap();
    engine
        .run_handler(
            1,
            &env(&[
                ("NAME", c),
                ("SENDER", "aerospace_mode_change"),
                ("MODE", "service"),
            ]),
            &mut host,
        )
        .unwrap();
    // Pseudo senders reach no handler.
    engine
        .run_handler(1, &env(&[("NAME", c), ("SENDER", "forced")]), &mut host)
        .unwrap();
    assert_eq!(
        host.messages,
        vec![
            argv(&[
                "--set",
                "a",
                "label=2/1",
                "--set",
                "b",
                &format!("label={c}")
            ]),
            argv(&["--set", "m", "label=service"]),
        ]
    );
}

/// A failing `on` handler does not stop the others; its error is reported.
#[test]
fn aerospace_on_handler_errors() {
    let (mut engine, mut host) = run(r#"
        mbar.aerospace.on("focus_change", function() error("first fails") end)
        mbar.aerospace.on("focus_change", function(env) mbar.set("x", { label = env.WINDOW_ID }) end)
    "#);
    host.messages.clear();
    let e = engine
        .run_handler(
            1,
            &env(&[("SENDER", "aerospace_focus_change"), ("WINDOW_ID", "7")]),
            &mut host,
        )
        .unwrap_err();
    assert!(e.to_string().contains("first fails"), "{e}");
    assert_eq!(host.messages, vec![argv(&["--set", "x", "label=7"])]);
}

#[test]
fn aerospace_events_match_the_core() {
    assert_eq!(
        mbar_lua::AEROSPACE_EVENTS,
        mbar_core::aerospace::EVENT_NAMES
    );
}
