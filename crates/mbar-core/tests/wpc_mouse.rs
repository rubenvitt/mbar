//! Mouse handling: clicks, synthesized item enter/exit, global enter/exit, scroll throttle,
//! slider clicks (`docs/spec/events.md` §6, `item.md` §9).

mod wpc_common;
use mbar_core::platform::{MouseButton, MouseKind, WindowKey};
use std::time::Duration;
use wpc_common::*;

fn setup() -> H {
    let mut h = H::new();
    h.msg(&[
        "--add", "item", "btn", "left", "--set", "btn", "label=Button", "script=btn.sh",
        "click_script=click.sh", "--subscribe", "btn", "mouse.clicked", "mouse.entered",
        "mouse.exited", "mouse.scrolled", "--add", "item", "other", "left", "--set", "other",
        "label=Other", "script=other.sh", "--subscribe", "other", "mouse.exited",
        "--add", "item", "g", "right", "--set", "g", "script=g.sh", "--subscribe", "g",
        "mouse.entered.global", "mouse.exited.global", "mouse.scrolled.global",
    ]);
    h
}

#[test]
fn click_runs_click_script_then_script() {
    let mut h = setup();
    let (x, y) = h.center("btn");
    let fx = h.mouse(
        MouseKind::Up { button: MouseButton::Right, button_code: 1 },
        x,
        y,
        Some(WindowKey::Bar(1)),
    );
    let r = runs(&fx);
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(r[0].script, "click.sh");
    assert_eq!(r[0].get("BUTTON"), Some("right"));
    assert_eq!(r[0].get("MODIFIER"), Some("none"));
    assert_eq!(
        r[0].get("INFO"),
        Some("{\n\t\"button\": \"right\",\n\t\"button_code\": 1,\n\t\"modifier\": \"none\",\n\t\"modfier_code\": 256\n}\n")
    );
    assert_eq!(r[0].get("NAME"), Some("btn"));
    assert_eq!(r[0].sender(), None);
    assert_eq!(r[1].script, "btn.sh");
    assert_eq!(r[1].sender(), Some("mouse.clicked"));
    assert_eq!(r[1].get("BUTTON"), Some("right"));
    // Click on empty bar area does nothing.
    let fx = h.click(1000.0, y);
    assert!(runs(&fx).is_empty());
}

#[test]
fn enter_exit() {
    let mut h = setup();
    let (bx, by) = h.center("btn");
    let (ox, _) = h.center("other");
    let fx = h.mouse(MouseKind::Entered, bx, by, Some(WindowKey::Bar(1)));
    let r = runs(&fx);
    assert_eq!(r[0].item.as_deref(), Some("g"));
    assert_eq!(r[0].sender(), Some("mouse.entered.global"));
    assert_eq!(r[1].item.as_deref(), Some("btn"));
    assert_eq!(r[1].sender(), Some("mouse.entered"));
    // moving inside the item: nothing
    let fx = h.mouse(MouseKind::Moved, bx + 1.0, by, Some(WindowKey::Bar(1)));
    assert!(runs(&fx).is_empty());
    // onto `other`: btn exits (other has no mouse.entered, but is tracked)
    let fx = h.mouse(MouseKind::Moved, ox, by, Some(WindowKey::Bar(1)));
    let r = runs(&fx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].item.as_deref(), Some("btn"));
    assert_eq!(r[0].sender(), Some("mouse.exited"));
    // leaving the bar: exited.global + mouse.exited for every subscribed item (Q6)
    let fx = h.mouse(MouseKind::Exited, ox, 500.0, Some(WindowKey::Bar(1)));
    let r = runs(&fx);
    let got: Vec<(String, String)> = r
        .iter()
        .map(|r| (r.item.clone().unwrap(), r.sender().unwrap().to_string()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("g".to_string(), "mouse.exited.global".to_string()),
            ("btn".to_string(), "mouse.exited".to_string()),
            ("other".to_string(), "mouse.exited".to_string()),
        ]
    );
    // entering again fires global enter again
    let fx = h.mouse(MouseKind::Entered, 1000.0, by, Some(WindowKey::Bar(1)));
    assert_eq!(runs(&fx).len(), 1);
}

#[test]
fn scroll_throttle_and_global() {
    let mut h = setup();
    let (bx, by) = h.center("btn");
    h.mouse(MouseKind::Entered, 1000.0, by, Some(WindowKey::Bar(1)));
    let fx = h.mouse(MouseKind::Scrolled { delta: 2 }, bx, by, Some(WindowKey::Bar(1)));
    let r = runs(&fx);
    assert_eq!(r[0].sender(), Some("mouse.scrolled"));
    assert_eq!(r[0].get("SCROLL_DELTA"), Some("2"));
    h.res.now += Duration::from_millis(50);
    let fx = h.mouse(MouseKind::Scrolled { delta: 3 }, bx, by, Some(WindowKey::Bar(1)));
    assert!(runs(&fx).is_empty(), "throttled");
    h.res.now += Duration::from_millis(150);
    let fx = h.mouse(MouseKind::Scrolled { delta: 1 }, bx, by, Some(WindowKey::Bar(1)));
    assert_eq!(runs(&fx)[0].get("SCROLL_DELTA"), Some("4"));
    // empty bar area: global scroll with DID
    h.res.now += Duration::from_millis(400);
    let fx = h.mouse(MouseKind::Scrolled { delta: -1 }, 1000.0, by, Some(WindowKey::Bar(1)));
    let r = runs(&fx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].item.as_deref(), Some("g"));
    assert_eq!(r[0].sender(), Some("mouse.scrolled.global"));
    assert_eq!(r[0].get("DID"), Some("1"));
    assert_eq!(r[0].get("SCROLL_DELTA"), Some("-1"));
}

#[test]
fn slider_click_sets_percentage() {
    let mut h = H::new();
    h.msg(&[
        "--add", "slider", "s", "left", "100", "--set", "s", "script=s.sh", "slider.background.height=10",
        "click_script=c.sh", "--subscribe", "s", "mouse.clicked",
    ]);
    let q = h.query(&["s"]);
    let r = &q["bounding_rects"]["display-1"];
    let x0 = r["origin"][0].as_f64().unwrap() as f32;
    let (_, y) = h.center("s");
    // Somewhere in the track.
    let fx = h.click(x0 + 50.0, y);
    let r = runs(&fx);
    assert_eq!(r.len(), 2, "{r:?}");
    let pct: u32 = r[1].get("PERCENTAGE").unwrap().parse().unwrap();
    assert!(pct > 0 && pct <= 100);
    assert_eq!(h.query(&["s"])["slider"]["percentage"].to_string().trim_matches('"'), pct.to_string());
    // Dragging updates without scripts.
    let fx = h.mouse(MouseKind::Dragged, x0 + 10.0, y, Some(WindowKey::Bar(1)));
    assert!(runs(&fx).is_empty());
}
