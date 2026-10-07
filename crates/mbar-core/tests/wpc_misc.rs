//! Window lifecycle, platform commands and extensions.

mod wpc_common;
use mbar_core::geometry::Rect;
use mbar_core::platform::{Effect, Input, PlatformRequest, WindowKey};
use std::time::Duration;
use wpc_common::*;

#[test]
fn bar_windows_follow_displays() {
    let mut h = H::new();
    // begin() produced the bar window
    let mut h2 = H::new();
    let _ = &mut h2;
    let mut d2 = h.res.displays[0].clone();
    d2.id = 2;
    d2.adid = 2;
    d2.frame = Rect::new(1920.0, 0.0, 1920.0, 1080.0);
    h.res.displays.push(d2);
    h.input(Input::DisplaysChanged);
    let out = h.frame();
    let keys: Vec<_> = out.windows.iter().map(|w| w.key).collect();
    assert_eq!(keys, vec![WindowKey::Bar(1), WindowKey::Bar(2)]);
    assert_eq!(out.windows[1].frame.x, 1920.0);
    // --bar display=1 recreates the bars: bar 2 is closed.
    h.msg(&["--bar", "display=1"]);
    assert_eq!(h.last_frame.closed, vec![WindowKey::Bar(2)]);
    // display removed
    h.msg(&["--bar", "display=all"]);
    h.res.displays.pop();
    h.input(Input::DisplaysChanged);
    let out = h.frame();
    assert_eq!(out.closed, vec![WindowKey::Bar(2)]);
    // hidden bar: window parked, not closed
    h.msg(&["--bar", "hidden=on"]);
    assert!(h.last_frame.closed.is_empty());
    assert!(h.last_frame.windows[0].frame.y < 0.0);
    assert_eq!(h.msg(&["--bar", "hidden=current"]), "");
    assert!(!h.rt.model.bars[0].hidden);
    h.res.active_adid = 7;
    h.input(Input::Timer);
    let (_, fx) = h.msg_fx(&["--bar", "hidden=current"]);
    assert!(fx.contains(&Effect::Log("No bar on display 7 \n".into())));
}

#[test]
fn window_properties() {
    let mut h = H::new();
    h.msg(&[
        "--bar",
        "topmost=on",
        "shadow=on",
        "sticky=off",
        "font_smoothing=on",
    ]);
    let w = &h.last_frame.windows[0];
    assert_eq!(w.level, mbar_core::platform::level::STATUS);
    assert!(w.shadow && !w.sticky && w.font_smoothing);
    h.msg(&["--bar", "margin=10"]);
    assert_eq!(h.last_frame.windows[0].frame.x, 10.0);
}

#[test]
fn platform_commands() {
    let mut h = H::new();
    let (rsp, fx) = h.msg_fx(&[
        "--hotload",
        "on",
        "--load-font",
        "/f.ttf",
        "--menubar",
        "hide",
    ]);
    assert_eq!(rsp.as_deref(), Some(""));
    let p = platform(&fx);
    assert_eq!(
        p,
        vec![
            PlatformRequest::SetHotload(true),
            PlatformRequest::LoadFont("/f.ttf".into()),
            PlatformRequest::SetMenuBarHidden(true)
        ]
    );
    let (_, fx) = h.msg_fx(&["--bar", "hide_menubar=on"]);
    assert!(platform(&fx).contains(&PlatformRequest::SetMenuBarHidden(true)));
    let (_, fx) = h.msg_fx(&["--menu", "0"]);
    assert!(platform(&fx).contains(&PlatformRequest::OpenMenu { index: 0 }));
    assert_eq!(
        h.msg(&["--menu", "Nope"]),
        "[!] Menu: Menu 'Nope' not found\n"
    );
    // --monitor keeps the connection open (no reply) and streams events.
    let (rsp, _) = h.msg_fx(&["--monitor", "all"]);
    assert!(rsp.is_none());
    h.msg(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "script=a.sh",
        "--add",
        "event",
        "e",
        "--subscribe",
        "a",
        "e",
    ]);
    let fx = h.msg_fx(&["--trigger", "e", "INFO=[1,2]"]).1;
    let line = fx
        .iter()
        .find_map(|e| match e {
            Effect::Monitor(l) => Some(l.clone()),
            _ => None,
        })
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["type"], "event");
    assert_eq!(v["name"], "e");
    assert_eq!(v["info"], serde_json::json!([1, 2]));
    assert_eq!(v["items"], serde_json::json!(["a"]));
    let fx = h.advance(Duration::from_millis(1100));
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::Monitor(l) if l.starts_with("{\"type\":\"stats\""))));
}

#[test]
fn scroll_texts_marquee() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "m",
        "left",
        "--set",
        "m",
        "label=abcdefghijklmnop",
        "label.max_chars=4",
        "scroll_texts=on",
    ]);
    // counter % 15 == 0 on the first routine tick: the marquee starts.
    h.advance(Duration::from_millis(1050));
    assert_eq!(h.rt.animator.len(), 3);
    assert!(h
        .rt
        .animator
        .animations()
        .iter()
        .all(|a| a.path == "label.scroll"));
    h.advance_by(Duration::from_secs(1), Duration::from_millis(16));
    let item = h.rt.model.item(h.rt.model.find("m").unwrap()).unwrap();
    assert!(item.label.scroll > 0.0);
}

#[test]
fn script_stats() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "script=a.sh",
        "--update",
    ]);
    h.res.now += Duration::from_millis(20);
    h.input(Input::ScriptFinished {
        pid: 1,
        item: Some("a".into()),
        output: None,
    });
    let st = h.query(&["stats"]);
    assert_eq!(st["scripts"]["spawned"], 1);
    assert_eq!(st["scripts"]["running"], 0);
    assert_eq!(st["scripts"]["by_item"]["a"]["runs"], 1);
    assert!(st["scripts"]["by_item"]["a"]["max_ms"].as_f64().unwrap() >= 19.0);
    assert!(st["redraws_by_window"]["bar:1"].as_u64().unwrap() >= 1);
}
