//! Subscriptions, `--trigger`, update gating, timers, OS events (`docs/spec/events.md`).

mod wpc_common;
use mbar_core::platform::{
    Effect, Input, LuaRequest, OsEvent, PlatformRequest, SpaceInfo, SystemValue,
};
use mbar_core::runtime::CLOCK_PERIOD;
use std::time::Duration;
use wpc_common::*;

fn keys(r: &Run) -> Vec<&str> {
    r.env.iter().map(|(k, _)| k.as_str()).collect()
}

#[test]
fn custom_trigger_env() {
    let mut h = H::new();
    assert_eq!(
        h.msg(&[
            "--add",
            "item",
            "a",
            "left",
            "--set",
            "a",
            "script=echo a",
            "--add",
            "item",
            "b",
            "left",
            "--set",
            "b",
            "script=echo b",
            "--add",
            "event",
            "my_ev",
            "--subscribe",
            "a",
            "my_ev",
            "nope",
            "--subscribe",
            "missing",
            "my_ev",
        ]),
        "[?] Event: 'nope' not found\n[!] Subscribe: Item not found 'missing'\n"
    );
    assert_eq!(h.query(&["a"])["scripting"]["update_mask"], 1u64 << 18);
    let (rsp, fx) = h.msg_fx(&[
        "--trigger",
        "my_ev",
        "INFO=hi",
        "FOO=bar",
        "EMPTY=",
        "junk",
        "FOO=baz",
    ]);
    assert_eq!(rsp.as_deref(), Some(""));
    let r = runs(&fx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].script, "echo a");
    assert_eq!(r[0].item.as_deref(), Some("a"));
    assert_eq!(keys(&r[0]), vec!["INFO", "FOO", "NAME", "SENDER"]);
    assert_eq!(r[0].get("FOO"), Some("baz"));
    assert_eq!(r[0].sender(), Some("my_ev"));
    // Unknown events trigger nothing, silently.
    let (rsp, fx) = h.msg_fx(&["--trigger", "does_not_exist", "INFO=1"]);
    assert_eq!(rsp.as_deref(), Some(""));
    assert!(runs(&fx).is_empty());
    // Duplicate event names are ignored; built-in triggers go through custom dispatch.
    h.msg(&[
        "--add",
        "event",
        "my_ev",
        "com.x",
        "--subscribe",
        "b",
        "brightness_change",
    ]);
    let fx = h.msg_fx(&["--trigger", "brightness_change", "INFO=5"]).1;
    let r = runs(&fx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].get("INFO"), Some("5"));
    assert!(h.rt.listeners().brightness);
}

#[test]
fn forced_triggers_query_the_system() {
    let mut h = H::new();
    h.res
        .system
        .insert("Volume".into(), SystemValue::Level(0.5));
    h.res
        .system
        .insert("Wifi".into(), SystemValue::Text("home".into()));
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "v",
        "right",
        "--set",
        "v",
        "script=v.sh",
        "--subscribe",
        "v",
        "volume_change",
        "wifi_change",
        "media_change",
        "space_change",
        "display_change",
    ]);
    let p = platform(&fx);
    assert!(p.contains(&PlatformRequest::StartVolumeEvents));
    assert!(p.contains(&PlatformRequest::StartMediaEvents));
    let fx = h.msg_fx(&["--trigger", "volume_change", "INFO=99"]).1;
    assert_eq!(runs(&fx)[0].get("INFO"), Some("50"));
    let fx = h.msg_fx(&["--trigger", "wifi_change", "INFO=x"]).1;
    assert_eq!(runs(&fx)[0].get("INFO"), Some("home"));
    let fx = h.msg_fx(&["--trigger", "media_change"]).1;
    assert!(platform(&fx).contains(&PlatformRequest::RefreshMedia));
    assert!(runs(&fx).is_empty());
    let fx = h.msg_fx(&["--trigger", "space_change", "INFO=x"]).1;
    assert_eq!(runs(&fx)[0].get("INFO"), Some("{\n\t\"display-1\": 1\n}"));
    let fx = h.msg_fx(&["--trigger", "display_change"]).1;
    assert_eq!(runs(&fx)[0].get("INFO"), Some("1"));
    // OS events.
    let fx = h.input(Input::Event(OsEvent::VolumeChanged(0.734)));
    assert_eq!(runs(&fx)[0].get("INFO"), Some("73"));
    let fx = h.input(Input::Event(OsEvent::MediaChanged("{}".into())));
    assert_eq!(runs(&fx)[0].sender(), Some("media_change"));
}

#[test]
fn update_gating_and_routine_counter() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "r",
        "left",
        "--set",
        "r",
        "script=r.sh",
        "update_freq=3",
        "--add",
        "item",
        "off",
        "left",
        "--set",
        "off",
        "script=off.sh",
        "updates=off",
        "update_freq=1",
        "--subscribe",
        "off",
        "system_woke",
        "--add",
        "event",
        "e",
        "--subscribe",
        "r",
        "e",
    ]);
    // First tick 1 s after begin; r runs every 3 ticks.
    let fx = h.advance(Duration::from_millis(3050));
    assert_eq!(runs_of(&fx, "r").len(), 1);
    assert!(runs_of(&fx, "off").is_empty());
    // An event run resets the countdown (Q2).
    h.advance(Duration::from_secs(1));
    let fx = h.msg_fx(&["--trigger", "e"]).1;
    assert_eq!(runs_of(&fx, "r")[0].sender(), Some("e"));
    let fx = h.advance(Duration::from_secs(2));
    assert!(runs_of(&fx, "r").is_empty());
    let fx = h.advance(Duration::from_secs(1));
    assert_eq!(runs_of(&fx, "r").len(), 1);
    // updates=off blocks events, not --update.
    let fx = h.msg_fx(&["--trigger", "system_woke"]).1;
    assert!(runs_of(&fx, "off").is_empty());
    let fx = h.msg_fx(&["--update"]).1;
    assert_eq!(runs_of(&fx, "off")[0].sender(), Some("forced"));
    // Q3: the env-less run stored SENDER in the persistent env.
    let fx = h
        .msg_fx(&["--set", "off", "updates=on", "--trigger", "system_woke"])
        .1;
    let r = runs_of(&fx, "off");
    assert_eq!(r[0].sender(), Some("system_woke"));
    assert_eq!(keys(&r[0]), vec!["NAME", "SENDER"]);
}

#[test]
fn when_shown() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "w",
        "left",
        "--set",
        "w",
        "script=w.sh",
        "update_freq=1",
        "updates=when_shown",
        "drawing=off",
    ]);
    let fx = h.advance(Duration::from_millis(2500));
    assert!(runs_of(&fx, "w").is_empty());
    h.msg(&["--set", "w", "drawing=on"]);
    let fx = h.advance(Duration::from_secs(1));
    assert_eq!(runs_of(&fx, "w").len(), 1);
}

#[test]
fn deadlines() {
    let mut h = H::new();
    let t0 = h.res.now;
    assert_eq!(h.rt.next_deadline(), Some(t0 + CLOCK_PERIOD));
    assert!(!h.rt.needs_frame());
    // Missed fires are skipped.
    h.res.now = t0 + Duration::from_millis(3500);
    h.input(Input::Timer);
    assert_eq!(h.rt.next_deadline(), Some(t0 + Duration::from_secs(4)));
    // An idle frame produces nothing.
    let out = h.frame();
    assert!(out.windows.is_empty() && out.closed.is_empty());
}

#[test]
fn space_change_flips() {
    let mut h = H::new();
    h.res.spaces = vec![
        SpaceInfo {
            id: 10,
            display: 1,
            fullscreen: false,
        },
        SpaceInfo {
            id: 11,
            display: 1,
            fullscreen: false,
        },
        SpaceInfo {
            id: 12,
            display: 1,
            fullscreen: true,
        },
    ];
    h.res.displays[0].current_space = 10;
    h.msg(&[
        "--add",
        "space",
        "s1",
        "left",
        "--set",
        "s1",
        "space=1",
        "--add",
        "space",
        "s2",
        "left",
        "--set",
        "s2",
        "space=2",
        "--add",
        "item",
        "x",
        "left",
        "--set",
        "x",
        "script=x.sh",
        "--subscribe",
        "x",
        "space_change",
    ]);
    let s1 = h.query(&["s1"]);
    assert_eq!(
        s1["scripting"]["script"],
        "sketchybar -m --set $NAME icon.highlight=$SELECTED"
    );
    let fx = h.input(Input::Event(OsEvent::SpaceChanged));
    let r = runs(&fx);
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(r[0].item.as_deref(), Some("s1"));
    assert_eq!(r[0].get("SELECTED"), Some("true"));
    assert_eq!(r[1].item.as_deref(), Some("x"));
    // switch to space 2: both flip
    h.res.displays[0].current_space = 11;
    let fx = h.input(Input::Event(OsEvent::SpaceChanged));
    let r = runs(&fx);
    assert_eq!(r.len(), 3);
    assert_eq!(r[0].get("SELECTED"), Some("false"));
    assert_eq!(r[1].item.as_deref(), Some("s2"));
    assert_eq!(r[1].get("INFO"), Some("{\n\t\"display-1\": 2\n}"));
    // same space again: only the plain subscriber
    let fx = h.input(Input::Event(OsEvent::SpaceChanged));
    assert_eq!(runs(&fx).len(), 1);
    // fullscreen space hides the bar
    h.res.displays[0].current_space = 12;
    h.input(Input::Event(OsEvent::SpaceChanged));
    assert!(!h.rt.model.bars[0].shown);
    h.frame();
    assert!(h.rt.model.items.iter().all(|i| !i.is_shown()));
    h.msg(&["--bar", "show_in_fullscreen=on"]);
    h.input(Input::Event(OsEvent::SpaceChanged));
    assert!(h.rt.model.bars[0].shown);
}

#[test]
fn sleep_and_wake() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "script=a.sh",
        "update_freq=1",
        "--subscribe",
        "a",
        "system_woke",
        "system_will_sleep",
        "display_change",
    ]);
    let fx = h.input(Input::Event(OsEvent::SystemWillSleep));
    assert_eq!(runs(&fx)[0].sender(), Some("system_will_sleep"));
    let fx = h.advance(Duration::from_secs(3));
    assert!(runs(&fx).is_empty(), "routine suppressed while asleep");
    let fx = h.msg_fx(&["--update"]).1;
    assert!(runs(&fx).is_empty(), "--update suppressed while asleep");
    let fx = h.input(Input::Event(OsEvent::SystemWoke));
    let senders: Vec<String> = runs(&fx)
        .iter()
        .map(|r| r.sender().unwrap().to_string())
        .collect();
    assert_eq!(senders, vec!["display_change", "system_woke"]);
    // second system_woke ~500 ms later
    let fx = h.advance_by(Duration::from_millis(600), Duration::from_millis(50));
    let woke = runs(&fx)
        .into_iter()
        .filter(|r| r.sender() == Some("system_woke"))
        .count();
    assert_eq!(woke, 1);
}

#[test]
fn active_display_poll() {
    let mut h = H::new();
    let mut d2 = h.res.displays[0].clone();
    d2.id = 2;
    d2.adid = 2;
    d2.frame = mbar_core::geometry::Rect::new(1920.0, 0.0, 1920.0, 1080.0);
    h.res.displays.push(d2);
    let fx = h.input(Input::DisplaysChanged);
    assert_eq!(h.rt.model.bars.len(), 2);
    let _ = fx;
    h.msg(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "script=a.sh",
        "--subscribe",
        "a",
        "display_change",
    ]);
    h.res.active_adid = 2;
    // Any input polls the active display first.
    let (_, fx) = h.msg_fx(&["--query", "bar"]);
    let r = runs(&fx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].get("INFO"), Some("2"));
    let fx = h.msg_fx(&["--query", "bar"]).1;
    assert!(runs(&fx).is_empty());
    // Both bars are drawn.
    let out = h.msg_fx(&["--bar", "color=0xff000000"]);
    let _ = out;
    let keys: Vec<_> = h.last_frame.windows.iter().map(|w| w.key).collect();
    assert_eq!(keys.len(), 2);
}

#[test]
fn notifications_mach_and_lua() {
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&[
        "--add",
        "event",
        "theme",
        "AppleInterfaceThemeChangedNotification",
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "mach_helper=git.felix.helper",
        "--subscribe",
        "a",
        "theme",
    ]);
    assert!(
        platform(&fx).contains(&PlatformRequest::ObserveNotification(
            "AppleInterfaceThemeChangedNotification".into()
        ))
    );
    let fx = h.input(Input::Event(OsEvent::DistributedNotification {
        name: "AppleInterfaceThemeChangedNotification".into(),
        info: Some("{}".into()),
    }));
    let mach: Vec<_> = platform(&fx)
        .into_iter()
        .filter_map(|p| match p {
            PlatformRequest::MachSend { service, payload } => Some((service, payload)),
            _ => None,
        })
        .collect();
    assert_eq!(mach.len(), 1);
    assert_eq!(mach[0].0, "git.felix.helper");
    assert_eq!(mach[0].1, b"INFO\0{}\0NAME\0a\0SENDER\0theme\0\0".to_vec());
    assert!(
        runs(&fx).is_empty(),
        "mach helper without script spawns nothing"
    );

    // Lua handlers: script=lua:<id> and LuaRequest::Subscribe.
    h.msg(&[
        "--add",
        "item",
        "l",
        "left",
        "--set",
        "l",
        "script=lua:7",
        "--subscribe",
        "l",
        "theme",
    ]);
    let fx = h.msg_fx(&["--trigger", "theme"]).1;
    let lua: Vec<_> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::LuaCallback { handler, env } => Some((*handler, env.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(lua.len(), 1);
    assert_eq!(lua[0].0, 7);
    assert!(lua[0].1.contains(&("SENDER".into(), "theme".into())));
    h.msg(&["--add", "item", "m", "left"]);
    h.input(Input::Lua(LuaRequest::Subscribe {
        item: "m".into(),
        events: vec!["theme".into(), "routine".into()],
        handler: 9,
    }));
    assert_eq!(h.query(&["m"])["scripting"]["script"], "lua:9");
    let fx = h.input(Input::Lua(LuaRequest::Command {
        args: vec![
            "--trigger".into(),
            "theme".into(),
            "--query".into(),
            "m".into(),
        ],
        callback: Some(3),
    }));
    let handlers: Vec<u64> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::LuaCallback { handler, .. } => Some(*handler),
            _ => None,
        })
        .collect();
    assert_eq!(handlers, vec![7, 9, 3]);
    match fx.last().unwrap() {
        Effect::LuaCallback { env, .. } => {
            assert_eq!(env[0].0, "RESPONSE");
            assert!(env[0].1.starts_with("{\n\t\"name\": \"m\""));
        }
        e => panic!("{e:?}"),
    }
    // --exit: mach helpers get "k", no reply.
    let (rsp, fx) = h.msg_fx(&["--exit", "--set", "a", "label=x"]);
    assert!(rsp.is_none());
    assert!(platform(&fx).contains(&PlatformRequest::MachSend {
        service: "git.felix.helper".into(),
        payload: b"k\0".to_vec()
    }));
    assert!(fx.contains(&Effect::Exit));
}

#[test]
fn reload_resets_state() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "event",
        "e",
        "--add",
        "item",
        "a",
        "left",
        "--subscribe",
        "a",
        "volume_change",
        "--hotload",
        "on",
    ]);
    assert!(h.rt.hotload());
    // Mode B quirk: the first argument is taken even if it starts with '-'.
    assert_eq!(
        h.msg(&["--reload", "--query", "bar"]),
        "[?] Reload: Invalid config path '--query'\n"
    );
    let (_, fx) = h.msg_fx(&["--reload"]);
    let rsp = h.msg(&["--query", "bar"]);
    assert!(rsp.contains("\"items\": [\n\n\t]"), "{rsp}");
    assert!(fx.contains(&Effect::RunConfig { path: None }));
    let ev = h.query(&["events"]);
    assert!(ev.get("e").is_none());
    assert!(h.rt.listeners().volume, "listeners survive");
    assert!(h.rt.hotload());
    assert_eq!(
        h.msg(&["--reload", "/definitely/not/here"]),
        "[?] Reload: Invalid config path '/definitely/not/here'\n"
    );
    // hotload via the watcher
    h.msg(&["--add", "item", "z", "left"]);
    let fx = h.input(Input::Event(OsEvent::ConfigChanged));
    assert!(fx.contains(&Effect::RunConfig { path: None }));
    assert!(h.rt.model.items.is_empty());
}
