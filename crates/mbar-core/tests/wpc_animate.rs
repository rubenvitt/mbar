//! `--animate` over simulated frames (`docs/spec/events.md` §10, D12, D18), redraw
//! batching and per-window dirty tracking.

mod wpc_common;
use mbar_core::platform::WindowKey;
use std::time::Duration;
use wpc_common::*;

fn y_offset(h: &mut H, name: &str) -> i64 {
    h.query(&[name])["geometry"]["y_offset"].as_i64().unwrap()
}

fn step(h: &mut H, ms: u64) {
    h.res.now += Duration::from_millis(ms);
    h.frame();
}

#[test]
fn linear_animation_and_lock() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left", "--set", "a", "label=A"]);
    assert!(!h.rt.needs_frame());
    h.msg(&["--animate", "linear", "30", "--set", "a", "y_offset=10"]);
    assert!(h.rt.needs_frame());
    assert_eq!(h.rt.next_deadline(), Some(h.res.now));
    // The first stepped frame (in msg) has t = 0.
    assert_eq!(y_offset(&mut h, "a"), 0);
    step(&mut h, 250);
    assert_eq!(y_offset(&mut h, "a"), 5);
    assert!(h.last_frame.windows.iter().any(|w| w.key == WindowKey::Bar(1)));
    // A later animated set of the same property cancels the locked animation and starts
    // from the current intermediate value.
    h.msg(&["--animate", "linear", "60", "--set", "a", "y_offset=0"]);
    step(&mut h, 500);
    assert_eq!(y_offset(&mut h, "a"), 3);
    step(&mut h, 600);
    assert_eq!(y_offset(&mut h, "a"), 0);
    assert!(h.rt.animator.is_empty());
    // Idle: frames produce nothing.
    let out = h.frame();
    assert!(out.windows.is_empty());
    assert!(!h.rt.needs_frame());
}

#[test]
fn chained_bounce_and_snap() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left"]);
    h.msg(&["--animate", "sin", "30", "--set", "a", "y_offset=10", "y_offset=0"]);
    step(&mut h, 500);
    assert_eq!(y_offset(&mut h, "a"), 10);
    step(&mut h, 250);
    let mid = y_offset(&mut h, "a");
    assert!(mid > 0 && mid < 10, "{mid}");
    step(&mut h, 300);
    assert_eq!(y_offset(&mut h, "a"), 0);
    // Immediate set while animating snaps then sets.
    h.msg(&["--animate", "linear", "60", "--set", "a", "y_offset=20"]);
    step(&mut h, 100);
    h.msg(&["--set", "a", "y_offset=7"]);
    assert_eq!(y_offset(&mut h, "a"), 7);
    assert!(h.rt.animator.is_empty());
}

#[test]
fn bar_animation_resizes_window() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left"]);
    let f0 = h.rt.model.bars[0].frame;
    h.msg(&["--animate", "linear", "60", "--bar", "height=45"]);
    step(&mut h, 500);
    let mid = h.rt.model.bars[0].frame;
    assert!(mid.height > f0.height && mid.height < f0.height + 20.0, "{mid:?}");
    let w = h.last_frame.windows.iter().find(|w| w.key == WindowKey::Bar(1)).unwrap();
    assert_eq!(w.frame, mid);
    step(&mut h, 600);
    assert_eq!(h.query(&["bar"])["height"], 45);
}

#[test]
fn remove_cancels_animations() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left", "--add", "item", "b", "left"]);
    h.msg(&["--animate", "linear", "60", "--set", "a", "y_offset=10", "--set", "b", "y_offset=10"]);
    assert_eq!(h.rt.animator.len(), 2);
    h.msg(&["--remove", "a"]);
    assert_eq!(h.rt.animator.len(), 1);
}

#[test]
fn width_animation_on_string_change() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left", "--set", "a", "label=ab"]);
    let w0 = h.query(&["a"])["bounding_rects"]["display-1"]["size"][0].as_f64().unwrap();
    h.msg(&["--animate", "linear", "30", "--set", "a", "label=abcdefghij"]);
    step(&mut h, 250);
    let w1 = h.query(&["a"])["bounding_rects"]["display-1"]["size"][0].as_f64().unwrap();
    step(&mut h, 400);
    let w2 = h.query(&["a"])["bounding_rects"]["display-1"]["size"][0].as_f64().unwrap();
    assert!(w0 < w1 && w1 < w2, "{w0} {w1} {w2}");
    assert_eq!(h.query(&["a"])["label"]["width"].to_string().trim_matches('"') == "0", false);
}

#[test]
fn batching_one_layout_and_dirty_windows() {
    let mut h = H::new();
    let mut d2 = h.res.displays[0].clone();
    d2.id = 2;
    d2.adid = 2;
    d2.frame = mbar_core::geometry::Rect::new(1920.0, 0.0, 1920.0, 1080.0);
    h.res.displays.push(d2);
    h.input(mbar_core::platform::Input::DisplaysChanged);
    h.frame();
    h.msg(&["--add", "item", "a", "left", "--set", "a", "display=1", "--add", "item", "b",
        "left", "--set", "b", "display=2"]);
    // Changing an item on display 2 only redraws bar 2.
    h.msg(&["--set", "b", "label=x"]);
    let keys: Vec<_> = h.last_frame.windows.iter().map(|w| w.key).collect();
    assert_eq!(keys, vec![WindowKey::Bar(2)]);
    // Several messages, one frame.
    let s0 = h.query(&["stats"])["frames"].as_u64().unwrap();
    for i in 0..5 {
        let label = format!("label={i}");
        h.input(mbar_core::platform::Input::Message {
            args: vec!["--set".into(), "a".into(), label],
            reply: mbar_core::platform::ReplyToken(1000 + i),
        });
    }
    let out = h.frame();
    assert_eq!(out.windows.len(), 1);
    let s1 = h.query(&["stats"])["frames"].as_u64().unwrap();
    assert_eq!(s1 - s0, 1 + 1, "one frame for the batch (+1 from the stats query msg)");
    // blur_radius changes window props without a redraw request from the setter.
    h.msg(&["--bar", "blur_radius=20"]);
    assert!(h.last_frame.windows.iter().all(|w| w.blur_radius == 20));
    assert_eq!(h.last_frame.windows.len(), 2);
}
