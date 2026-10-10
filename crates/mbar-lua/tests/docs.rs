//! Keeps `docs/LUA.md`, `docs/MIGRATING.md` and `lua/mbar.d.lua` honest: the
//! documented stock config must run and behave like the stock SketchyBar config,
//! the marked examples must run against the real API, and the LuaLS definitions
//! must be valid Lua.

use std::time::Duration;

use mbar_lua::{parse_script, AerospaceResult, Host, LuaEngine};

const LUA_MD: &str = include_str!("../../../docs/LUA.md");
const MIGRATING_MD: &str = include_str!("../../../docs/MIGRATING.md");
const DEFS: &str = include_str!("../../../lua/mbar.d.lua");

#[derive(Default)]
struct Mock {
    messages: Vec<Vec<String>>,
    spawned: Vec<(String, Option<u64>)>,
    aerospace: Vec<(Vec<String>, Option<u64>)>,
    on: Vec<(Vec<String>, u64)>,
}

impl Host for Mock {
    fn command(&mut self, args: Vec<String>) -> String {
        self.messages.push(args);
        String::new()
    }
    fn spawn_shell(&mut self, cmd: String, callback: Option<u64>) {
        self.spawned.push((cmd, callback));
    }
    fn schedule(&mut self, _delay: Duration, _callback: u64) {}
    fn aerospace(&mut self, args: Vec<String>, callback: Option<u64>) {
        self.aerospace.push((args, callback));
    }
    fn on_events(&mut self, events: Vec<String>, handler: u64) {
        self.on.push((events, handler));
    }
}

fn stock_example() -> &'static str {
    example("stock")
}

/// The code block after `<!-- example: <name> -->` in `docs/LUA.md`.
fn example(name: &str) -> &'static str {
    example_in(LUA_MD, name)
}

/// The code block after `<!-- example: <name> -->` in `doc`.
fn example_in(doc: &'static str, name: &str) -> &'static str {
    let start = doc
        .find(&format!("<!-- example: {name} -->"))
        .expect("example marker");
    let rest = &doc[start..];
    let code = rest.find("```lua\n").expect("code block") + "```lua\n".len();
    let end = rest[code..].find("\n```").expect("end of code block");
    &rest[code..code + end]
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Handler id from `script=lua:<id>` in the `--set <item>` of a message.
fn script_id(msg: &[String], item: &str) -> u64 {
    prop_id(msg, item, "script=")
}

/// Handler id from `<key>lua:<id>` (`key` is e.g. `"click_script="`) in the
/// `--set <item>` of a message.
fn prop_id(msg: &[String], item: &str, key: &str) -> u64 {
    let mut i = 0;
    while i + 1 < msg.len() {
        if msg[i] == "--set" && msg[i + 1] == item {
            let mut j = i + 2;
            while j < msg.len() && !msg[j].starts_with("--") {
                if let Some(v) = msg[j].strip_prefix(key) {
                    return parse_script(v).expect("lua script");
                }
                j += 1;
            }
        }
        i += 1;
    }
    panic!("no lua {key} for {item}");
}

fn has_seq(msg: &[String], seq: &[&str]) -> bool {
    msg.windows(seq.len())
        .any(|w| w.iter().zip(seq).all(|(a, b)| a == b))
}

#[test]
fn definitions_compile() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    // A long bracket keeps the file verbatim; load() only compiles it.
    let src = format!("assert(load([==========[{DEFS}]==========], '=mbar.d.lua'))");
    engine.load_string(&src, "=defs", &mut host).unwrap();
}

#[test]
fn borders_example() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine
        .load_string(example("borders"), "=init.lua", &mut host)
        .unwrap();
    let msg = host.messages.concat();
    assert_eq!(
        msg.iter().filter(|a| *a == "--borders").count(),
        4,
        "{msg:?}"
    );
    assert!(has_seq(
        &msg,
        &[
            "--borders",
            "active_color=0xffe1e3e4",
            "blacklist=Safari,kitty",
            "hidpi=off",
            "inactive_color=0xff494d64",
            "style=round",
            "width=5",
        ]
    ));
    assert!(has_seq(&msg, &["--borders", "drawing=off"]));
    // The core's JankyBorders parser accepts every pair.
    let pairs: Vec<String> = msg.into_iter().filter(|a| a != "--borders").collect();
    let (valid, errors) = mbar_core::borders::validate_args(&pairs);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(valid, pairs);
}

#[test]
fn stock_config_example() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine
        .load_string(stock_example(), "=init.lua", &mut host)
        .unwrap();

    // One batch for the whole config, then `--update`.
    assert_eq!(host.messages.len(), 2, "{:?}", host.messages);
    assert_eq!(host.messages[1], vec!["--update".to_string()]);
    let cfg = host.messages[0].clone();
    assert!(has_seq(
        &cfg,
        &[
            "--bar",
            "blur_radius=30",
            "color=0x40000000",
            "height=40",
            "position=top"
        ]
    ));
    assert!(has_seq(
        &cfg,
        &[
            "--default",
            "icon.color=0xffffffff",
            "icon.font=Hack Nerd Font:Bold:17.0",
            "icon.padding_left=4",
            "icon.padding_right=4",
            "label.color=0xffffffff",
            "label.font.family=Hack Nerd Font",
            "label.font.size=14",
            "label.font.style=Bold",
            "label.padding_left=4",
            "label.padding_right=4",
            "padding_left=5",
            "padding_right=5",
        ]
    ));
    assert!(has_seq(&cfg, &["--add", "space", "space.10", "left"]));
    assert!(has_seq(
        &cfg,
        &[
            "background.height=25",
            "click_script=yabai -m space --focus 3"
        ]
    ));
    assert!(has_seq(
        &cfg,
        &["--set", "chevron", "icon=\u{f054}", "label.drawing=off"]
    ));
    assert!(has_seq(
        &cfg,
        &["--subscribe", "front_app", "front_app_switched"]
    ));
    assert!(has_seq(&cfg, &["--add", "item", "clock", "right"]));
    assert!(has_seq(&cfg, &["--subscribe", "volume", "volume_change"]));
    assert!(has_seq(
        &cfg,
        &[
            "--subscribe",
            "battery",
            "system_woke",
            "power_source_change"
        ]
    ));
    // Items appear in the stock order.
    let adds: Vec<&str> = cfg
        .windows(3)
        .filter(|w| w[0] == "--add")
        .map(|w| w[2].as_str())
        .collect();
    assert_eq!(adds.len(), 15);
    assert_eq!(
        &adds[9..],
        [
            "space.10",
            "chevron",
            "front_app",
            "clock",
            "volume",
            "battery"
        ]
    );

    let space = script_id(&cfg, "space.2");
    let front = script_id(&cfg, "front_app");
    let clock = script_id(&cfg, "clock");
    let volume = script_id(&cfg, "volume");
    let battery = script_id(&cfg, "battery");
    host.messages.clear();

    engine
        .run_handler(
            space,
            &env(&[
                ("NAME", "space.2"),
                ("SENDER", "space_change"),
                ("SELECTED", "true"),
            ]),
            &mut host,
        )
        .unwrap();
    engine
        .run_handler(
            front,
            &env(&[
                ("NAME", "front_app"),
                ("SENDER", "front_app_switched"),
                ("INFO", "Ghostty"),
            ]),
            &mut host,
        )
        .unwrap();
    engine
        .run_handler(
            front,
            &env(&[("NAME", "front_app"), ("SENDER", "forced")]),
            &mut host,
        )
        .unwrap();
    engine
        .run_handler(
            volume,
            &env(&[
                ("NAME", "volume"),
                ("SENDER", "volume_change"),
                ("INFO", "73"),
            ]),
            &mut host,
        )
        .unwrap();
    engine
        .run_handler(
            clock,
            &env(&[("NAME", "clock"), ("SENDER", "routine")]),
            &mut host,
        )
        .unwrap();
    assert_eq!(host.messages.len(), 4, "{:?}", host.messages);
    assert_eq!(
        host.messages[0],
        ["--set", "space.2", "background.drawing=on"]
    );
    assert_eq!(host.messages[1], ["--set", "front_app", "label=Ghostty"]);
    assert_eq!(
        host.messages[2],
        ["--set", "volume", "icon=\u{f057e}", "label=73%"]
    );
    assert_eq!(host.messages[3][..2], ["--set", "clock"]);
    assert!(host.messages[3][2].starts_with("label="));
    host.messages.clear();

    engine
        .run_handler(
            battery,
            &env(&[("NAME", "battery"), ("SENDER", "system_woke")]),
            &mut host,
        )
        .unwrap();
    let (cmd, id) = host.spawned.pop().unwrap();
    assert_eq!(cmd, "pmset -g batt");
    let out = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t75%; discharging; 4:12 remaining present: true\n";
    engine
        .exec_finished(id.unwrap(), out.into(), &mut host)
        .unwrap();
    assert_eq!(
        host.messages,
        vec![vec!["--set", "battery", "icon=\u{f241}", "label=75%"]]
    );
}

fn aero(exit_code: i32, stdout: &str) -> AerospaceResult {
    AerospaceResult {
        exit_code,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

/// The workspace example of `docs/LUA.md` and its copy in `docs/MIGRATING.md`: items
/// from `list-workspaces`, highlight on `aerospace_workspace_change`, click runs
/// `aerospace workspace <sid>`.
#[test]
fn aerospace_workspace_examples() {
    for (doc, src) in [
        ("LUA.md", example("aerospace-spaces")),
        (
            "MIGRATING.md",
            example_in(MIGRATING_MD, "aerospace-migrate"),
        ),
    ] {
        let mut engine = LuaEngine::new().unwrap();
        let mut host = Mock::default();
        engine
            .load_string(src, "=init.lua", &mut host)
            .unwrap_or_else(|e| panic!("{doc}: {e}"));
        assert_eq!(host.aerospace.len(), 1, "{doc}");
        let (args, id) = host.aerospace[0].clone();
        assert_eq!(args, ["list-workspaces", "--all"], "{doc}");
        // An item-less handler: no item, no subscription message.
        assert!(host.messages.is_empty(), "{doc}: {:?}", host.messages);
        assert_eq!(host.on.len(), 1, "{doc}");
        let (events, handler) = host.on[0].clone();
        assert_eq!(events, ["aerospace_workspace_change"], "{doc}");

        engine
            .aerospace_finished(id.expect("callback"), aero(0, "1\n2\n"), &mut host)
            .unwrap();
        let added = host.messages.concat();
        for sid in ["1", "2"] {
            let name = format!("space.{sid}");
            assert!(
                has_seq(&added, &["--add", "item", &name, "left"]),
                "{doc}: {added:?}"
            );
        }
        assert!(
            host.messages.iter().any(|m| m == &["--query", "aerospace"]),
            "{doc}: the state is queried after adding the items"
        );
        let click = prop_id(&added, "space.2", "click_script=");
        host.messages.clear();

        engine
            .run_handler(
                handler,
                &env(&[
                    ("SENDER", "aerospace_workspace_change"),
                    ("FOCUSED_WORKSPACE", "2"),
                    ("PREV_WORKSPACE", "1"),
                ]),
                &mut host,
            )
            .unwrap();
        let set = host.messages.concat();
        assert!(
            has_seq(&set, &["--set", "space.1", "background.drawing=off"]),
            "{doc}: {set:?}"
        );
        assert!(
            has_seq(&set, &["--set", "space.2", "background.drawing=on"]),
            "{doc}: {set:?}"
        );

        engine
            .run_handler(click, &env(&[("NAME", "space.2")]), &mut host)
            .unwrap();
        assert_eq!(
            host.aerospace.last(),
            Some(&(vec!["workspace".to_string(), "2".to_string()], None)),
            "{doc}"
        );
    }
}

/// The `mbar.aerospace.query` example of `docs/LUA.md`.
#[test]
fn aerospace_windows_example() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine
        .load_string(example("aerospace-windows"), "=init.lua", &mut host)
        .unwrap();
    let (args, id) = host.aerospace[0].clone();
    assert_eq!(args, ["list-windows", "--workspace", "focused", "--json"]);
    engine
        .aerospace_finished(
            id.unwrap(),
            aero(
                0,
                r#"[{"app-name":"Finder","window-id":1},{"app-name":"Mail","window-id":2}]"#,
            ),
            &mut host,
        )
        .unwrap();
    assert_eq!(
        host.messages,
        vec![vec![
            "--set".to_string(),
            "windows".to_string(),
            "label=Finder Mail".to_string()
        ]]
    );
}

/// The privacy indicator example of `docs/LUA.md`: inset on, the clock subscribes
/// and turns orange while an app uses the microphone.
#[test]
fn privacy_indicator_example() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine
        .load_string(example("privacy-indicator"), "=init.lua", &mut host)
        .unwrap();
    let msg = host.messages.concat();
    assert!(
        has_seq(&msg, &["--bar", "privacy_indicator_inset=on"]),
        "{msg:?}"
    );
    assert!(
        has_seq(&msg, &["--subscribe", "clock", "privacy_indicator_change"]),
        "{msg:?}"
    );
    let handler = script_id(&msg, "clock");
    host.messages.clear();
    engine
        .run_handler(
            handler,
            &env(&[
                ("NAME", "clock"),
                ("SENDER", "privacy_indicator_change"),
                ("MIC", "com.goodsnooze.MacWhisper"),
                (
                    "INFO",
                    r#"{"visible":"on","mic":["com.goodsnooze.MacWhisper"],"camera":[],"screen":[],"audio":[],"location":[],"attribution":"on"}"#,
                ),
            ]),
            &mut host,
        )
        .unwrap();
    let set = host.messages.concat();
    assert!(set.iter().any(|a| a.starts_with("label.color=")), "{set:?}");
}
