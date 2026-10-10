//! Privacy indicator in the core runtime: built-in `privacy_indicator_change`, lazy
//! `StartPrivacyIndicator`, `privacy_indicator_inset`, `--query privacy_indicator`
//! and `--reload` (`docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`).

mod wpc_common;
use mbar_core::geometry::Rect;
use mbar_core::platform::{Effect, Input, LuaRequest, PlatformRequest};
use mbar_core::privacy::{Attributions, PrivacySample};
use wpc_common::*;

const DOT: Rect = Rect::new(1892.0, 0.0, 28.0, 100.0);

fn sample(visible: bool, mic: &[&str]) -> Input {
    Input::PrivacyIndicator(PrivacySample {
        visible,
        frames: if visible { vec![DOT] } else { vec![] },
        attributions: Some(Attributions {
            mic: mic.iter().map(|s| s.to_string()).collect(),
            ..Attributions::default()
        }),
    })
}

fn starts(fx: &[Effect]) -> usize {
    platform(fx)
        .iter()
        .filter(|p| matches!(p, PlatformRequest::StartPrivacyIndicator))
        .count()
}

fn feed(h: &mut H, input: Input) -> Vec<Effect> {
    let fx = h.input(input);
    h.frame();
    fx
}

fn add_watcher(h: &mut H, name: &str) -> Vec<Effect> {
    h.msg_fx(&[
        "--add",
        "item",
        name,
        "right",
        "--set",
        name,
        "script=dot.sh",
        "--subscribe",
        name,
        "privacy_indicator_change",
    ])
    .1
}

#[test]
fn subscription_registers_the_event_and_starts_once() {
    let mut h = H::new();
    let (rsp, fx) = h.msg_fx(&[
        "--add",
        "item",
        "a",
        "right",
        "--set",
        "a",
        "script=dot.sh",
        "--subscribe",
        "a",
        "privacy_indicator_change",
    ]);
    assert_eq!(rsp.unwrap_or_default(), "", "no '[?] Event: ... not found'");
    assert_eq!(starts(&fx), 1);
    assert!(h
        .query(&["events"])
        .get("privacy_indicator_change")
        .is_some());
    assert_eq!(starts(&add_watcher(&mut h, "b")), 0, "started once");
}

#[test]
fn inset_property_starts_detection_once() {
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&["--bar", "privacy_indicator_inset=on"]);
    assert_eq!(starts(&fx), 1);
    h.msg(&["--bar", "privacy_indicator_inset=off"]);
    let (_, fx) = h.msg_fx(&["--bar", "privacy_indicator_inset=on"]);
    assert_eq!(starts(&fx), 0);
    // `--query bar` stays SketchyBar's output.
    assert!(h.query(&["bar"]).get("privacy_indicator_inset").is_none());
}

#[test]
fn query_reports_without_starting() {
    let mut h = H::new();
    let (text, fx) = h.msg_fx(&["--query", "privacy_indicator"]);
    assert_eq!(starts(&fx), 0);
    let q: serde_json::Value = serde_json::from_str(&text.unwrap()).unwrap();
    assert_eq!(q["active"], "off");
    assert_eq!(q["visible"], "off");
    assert_eq!(q["inset"], "off");
    assert_eq!(q["attribution"], "off");
    // An item with that name wins.
    h.msg(&["--add", "item", "privacy_indicator", "left"]);
    assert_eq!(h.query(&["privacy_indicator"])["name"], "privacy_indicator");
}

#[test]
fn sample_triggers_the_event_once_per_change() {
    let mut h = H::new();
    add_watcher(&mut h, "a");
    let fx = feed(&mut h, sample(true, &["com.b", "com.a"]));
    let r = runs_of(&fx, "a");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].sender(), Some("privacy_indicator_change"));
    assert_eq!(r[0].get("VISIBLE"), Some("on"));
    assert_eq!(r[0].get("MIC"), Some("com.b,com.a"));
    assert_eq!(r[0].get("SCREEN"), Some(""));
    let info: serde_json::Value = serde_json::from_str(r[0].get("INFO").unwrap()).unwrap();
    assert_eq!(info["frame"]["x"], 1892);
    assert_eq!(info["attribution"], "on");

    assert!(
        runs(&feed(&mut h, sample(true, &["com.b", "com.a"]))).is_empty(),
        "unchanged"
    );
    let fx = feed(&mut h, sample(false, &[]));
    assert_eq!(runs_of(&fx, "a")[0].get("VISIBLE"), Some("off"));
    assert_eq!(h.query(&["privacy_indicator"])["visible"], "off");
}

#[test]
fn late_subscriber_gets_the_state_once() {
    let mut h = H::new();
    h.msg(&["--bar", "privacy_indicator_inset=on"]);
    feed(&mut h, sample(true, &["com.a"]));
    let fx = add_watcher(&mut h, "late");
    let r = runs_of(&fx, "late");
    assert_eq!(r.len(), 1, "{fx:?}");
    assert_eq!(r[0].get("MIC"), Some("com.a"));
    let (_, fx) = h.msg_fx(&["--subscribe", "late", "privacy_indicator_change"]);
    assert!(
        runs_of(&fx, "late").is_empty(),
        "already subscribed: no second delivery"
    );
    // Before any sample there is nothing to deliver.
    let mut h = H::new();
    assert!(runs_of(&add_watcher(&mut h, "early"), "early").is_empty());
}

#[test]
fn reload_keeps_state_and_detection() {
    let mut h = H::new();
    add_watcher(&mut h, "a");
    feed(&mut h, sample(true, &["com.a"]));
    let before = h.msg(&["--query", "privacy_indicator"]);
    h.msg(&["--reload"]);
    assert_eq!(h.msg(&["--query", "privacy_indicator"]), before);
    let fx = add_watcher(&mut h, "a");
    assert_eq!(starts(&fx), 0, "detection survives --reload");
    assert_eq!(runs_of(&fx, "a").len(), 1, "re-run config gets the state");
}

#[test]
fn manual_trigger_reaches_subscribers_but_keeps_state() {
    let mut h = H::new();
    add_watcher(&mut h, "a");
    let (_, fx) = h.msg_fx(&["--trigger", "privacy_indicator_change", "VISIBLE=on"]);
    assert_eq!(runs_of(&fx, "a")[0].get("VISIBLE"), Some("on"));
    assert_eq!(h.query(&["privacy_indicator"])["visible"], "off");
}

#[test]
fn add_event_with_a_notification_observes_nothing() {
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&[
        "--add",
        "event",
        "privacy_indicator_change",
        "com.example.note",
    ]);
    assert!(!platform(&fx)
        .iter()
        .any(|p| matches!(p, PlatformRequest::ObserveNotification(_))));
}

fn on(h: &mut H, handler: u64) -> Vec<Effect> {
    h.input(Input::Lua(LuaRequest::On {
        events: vec!["privacy_indicator_change".into()],
        handler,
    }))
}

fn callbacks(fx: &[Effect]) -> Vec<(u64, Vec<(String, String)>)> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::LuaCallback { handler, env } => Some((*handler, env.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn global_handler_starts_detection_once() {
    let mut h = H::new();
    assert_eq!(starts(&on(&mut h, 1)), 1);
    assert_eq!(starts(&on(&mut h, 2)), 0);
    assert_eq!(starts(&add_watcher(&mut h, "a")), 0, "started once");
}

#[test]
fn late_global_handler_gets_the_state_once() {
    let mut h = H::new();
    assert!(callbacks(&on(&mut h, 1)).is_empty(), "no state yet");
    feed(&mut h, sample(true, &["com.a"]));
    let fx = on(&mut h, 2);
    let cb = callbacks(&fx);
    assert_eq!(cb.len(), 1, "{fx:?}");
    assert_eq!(cb[0].0, 2);
    assert!(cb[0].1.contains(&("MIC".to_string(), "com.a".to_string())));
    assert!(callbacks(&on(&mut h, 2)).is_empty(), "already registered");
}

fn x_of(h: &mut H, item: &str) -> f64 {
    h.query(&["item", item])["bounding_rects"]["display-1"]["origin"][0]
        .as_f64()
        .unwrap()
}

fn clock_bar(h: &mut H) {
    h.msg(&[
        "--add",
        "item",
        "clock",
        "right",
        "--set",
        "clock",
        "label=12:00",
    ]);
    h.msg(&["--add", "item", "left", "left", "--set", "left", "label=L"]);
}

#[test]
fn inset_moves_right_items_left_of_the_dot() {
    let mut h = H::new();
    clock_bar(&mut h);
    let (x0, l0) = (x_of(&mut h, "clock"), x_of(&mut h, "left"));
    h.msg(&["--bar", "privacy_indicator_inset=on"]);
    feed(&mut h, sample(true, &[]));
    // Display 1920 wide, dot at 1892: the right edge moves 28 pt left.
    assert_eq!(x_of(&mut h, "clock"), x0 - 28.0);
    assert_eq!(x_of(&mut h, "left"), l0, "left items stay");
    // padding_right stays the gap to the dot.
    h.msg(&["--bar", "padding_right=10"]);
    let with_dot = x_of(&mut h, "clock");
    feed(&mut h, sample(false, &[]));
    assert_eq!(
        x_of(&mut h, "clock"),
        with_dot + 28.0,
        "dot gone: back to the edge"
    );
}

#[test]
fn no_inset_when_off_or_not_overlapping() {
    let mut h = H::new();
    clock_bar(&mut h);
    let x0 = x_of(&mut h, "clock");
    // Detection started by a subscription; the property stays off. (`w` is added after
    // `clock`, so it sits left of it and does not move it.)
    add_watcher(&mut h, "w");
    feed(&mut h, sample(true, &[]));
    assert_eq!(x_of(&mut h, "clock"), x0, "property off");
    // Property on, but the dot does not intersect the bar (another display).
    h.msg(&["--bar", "privacy_indicator_inset=on"]);
    feed(
        &mut h,
        Input::PrivacyIndicator(PrivacySample {
            visible: true,
            frames: vec![Rect::new(3000.0, 500.0, 28.0, 28.0)],
            attributions: None,
        }),
    );
    assert_eq!(x_of(&mut h, "clock"), x0, "no overlap");
    // A bottom bar does not meet a dot at the top.
    feed(&mut h, sample(true, &[]));
    h.msg(&["--bar", "position=bottom"]);
    let mut plain = H::new();
    clock_bar(&mut plain);
    plain.msg(&["--add", "item", "w", "right"]);
    plain.msg(&["--bar", "position=bottom"]);
    assert_eq!(
        x_of(&mut h, "clock"),
        x_of(&mut plain, "clock"),
        "bottom bar"
    );
}

#[test]
fn inset_smaller_than_padding_does_not_panic() {
    let mut h = H::new();
    clock_bar(&mut h);
    h.msg(&["--bar", "privacy_indicator_inset=on"]);
    feed(
        &mut h,
        Input::PrivacyIndicator(PrivacySample {
            visible: true,
            frames: vec![Rect::new(5.0, 0.0, 28.0, 100.0)],
            attributions: None,
        }),
    );
    // Right items overflow; SketchyBar's rule puts them at the edge. No panic, finite x.
    assert!(x_of(&mut h, "clock").is_finite());
}
