//! Review probes (temporary).
mod wpc_common;
use mbar_core::platform::{Input, MouseKind, OsEvent, WindowKey};
use std::time::Duration;
use wpc_common::*;

fn senders(fx: &[mbar_core::platform::Effect], item: &str) -> Vec<String> {
    runs_of(fx, item)
        .iter()
        .map(|r| r.sender().unwrap_or("").to_string())
        .collect()
}

#[test]
fn p1_when_shown_after_wake() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "w",
        "left",
        "--set",
        "w",
        "label=W",
        "script=w.sh",
        "updates=when_shown",
        "--subscribe",
        "w",
        "system_woke",
        "display_change",
        "space_change",
    ]);
    h.frame();
    assert!(h.rt.model.items[0].is_shown());
    h.input(Input::Event(OsEvent::SystemWillSleep));
    let fx = h.input(Input::Event(OsEvent::SystemWoke));
    eprintln!("wake senders: {:?}", senders(&fx, "w"));
    let fx = h.input(Input::DisplaysChanged);
    eprintln!("displays changed senders: {:?}", senders(&fx, "w"));
    assert!(
        !senders(&fx, "w").is_empty(),
        "when_shown item missed display rebuild events"
    );
}

#[test]
fn p2_non_sticky_space_change() {
    let mut h = H::new();
    h.res.spaces.push(mbar_core::platform::SpaceInfo { id: 2, display: 1, fullscreen: false });
    h.msg(&["--bar", "sticky=off", "--add", "item", "a", "left", "--set", "a", "label=A"]);
    h.frame();
    h.effects.clear();
    h.res.displays[0].current_space = 2;
    let fx = h.input(Input::Event(OsEvent::SpaceChanged));
    let out = h.frame();
    eprintln!("platform reqs: {:?}", platform(&fx));
    eprintln!("windows: {:?}", out.windows.iter().map(|w| w.key).collect::<Vec<_>>());
    assert_eq!(h.rt.model.bars[0].sid, 2);
}

#[test]
fn p3_deadline_during_animation() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left", "--animate", "linear", "600", "--set", "a", "y_offset=100"]);
    h.frame();
    let d = h.rt.next_deadline().unwrap();
    eprintln!("deadline - now = {:?}", d.saturating_duration_since(h.res.now));
    h.res.now += Duration::from_millis(5);
    let d2 = h.rt.next_deadline().unwrap();
    eprintln!("deadline after 5ms already past: {}", d2 < h.res.now);
}
