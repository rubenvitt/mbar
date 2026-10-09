//! Keeps `docs/LUA.md` and `lua/mbar.d.lua` honest: the documented stock config
//! must run and behave like the stock SketchyBar config, and the LuaLS
//! definitions must be valid Lua.

use std::time::Duration;

use mbar_lua::{parse_script, Host, LuaEngine};

const LUA_MD: &str = include_str!("../../../docs/LUA.md");
const DEFS: &str = include_str!("../../../lua/mbar.d.lua");

#[derive(Default)]
struct Mock {
    messages: Vec<Vec<String>>,
    spawned: Vec<(String, Option<u64>)>,
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
}

fn stock_example() -> &'static str {
    example("stock")
}

/// The code block after `<!-- example: <name> -->` in `docs/LUA.md`.
fn example(name: &str) -> &'static str {
    let start = LUA_MD
        .find(&format!("<!-- example: {name} -->"))
        .expect("example marker");
    let rest = &LUA_MD[start..];
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
    let mut i = 0;
    while i + 1 < msg.len() {
        if msg[i] == "--set" && msg[i + 1] == item {
            let mut j = i + 2;
            while j < msg.len() && !msg[j].starts_with("--") {
                if let Some(v) = msg[j].strip_prefix("script=") {
                    return parse_script(v).expect("lua script");
                }
                j += 1;
            }
        }
        i += 1;
    }
    panic!("no lua script for {item}");
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
