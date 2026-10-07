//! Review regressions for the runtime's refresh ordering, space moves and window
//! invalidation:
//!
//! * RT-1: display rebuilds (wake, `DisplaysChanged`) run the forced refresh before
//!   `display_change` / `space_change` / `system_woke` are dispatched (`events.md` §9.2–9.3),
//!   so `updates=when_shown` items still count as shown.
//! * RT-2: non-sticky bars are moved to the new space on `space_change`
//!   (`bar_change_space`, `bar.md` §6.4).
//! * RT-4: `--update` in the same message as `--add` sees the new item as shown, because
//!   `handle_space_change` ends with `unfreeze(); bar_manager_refresh` (Q7, D14).
//! * PERF-4: a bar change does not re-render open popups and a popup-member change does not
//!   re-render the bar (unless the member clips the bar).
//! * PERF-6: regex selectors (compiled once, index-based `--set`) keep their semantics.
mod wpc_common;
use mbar_core::item::ItemId;
use mbar_core::platform::{
    HeadlessResources, Input, OsEvent, SpaceInfo, SpaceMove, SystemValue, WindowKey,
};
use wpc_common::{runs_of, H};

fn senders(fx: &[mbar_core::platform::Effect], item: &str) -> Vec<String> {
    runs_of(fx, item)
        .iter()
        .map(|r| r.sender().unwrap_or("").to_string())
        .collect()
}

fn when_shown_item(h: &mut H) {
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
}

#[test]
fn rt1_wake_runs_when_shown_items() {
    let mut h = H::new();
    when_shown_item(&mut h);
    h.input(Input::Event(OsEvent::SystemWillSleep));
    let fx = h.input(Input::Event(OsEvent::SystemWoke));
    assert_eq!(
        senders(&fx, "w"),
        ["display_change", "space_change", "system_woke"]
    );
}

#[test]
fn rt1_displays_changed_runs_when_shown_items() {
    let mut h = H::new();
    when_shown_item(&mut h);
    let fx = h.input(Input::DisplaysChanged);
    assert_eq!(senders(&fx, "w"), ["display_change", "space_change"]);
    // The rebuilt bar is still drawn.
    let out = h.frame();
    assert!(out.windows.iter().any(|w| w.key == WindowKey::Bar(1)));
}

#[test]
fn rt1_hidden_when_shown_item_still_skipped() {
    let mut h = H::new();
    when_shown_item(&mut h);
    h.msg(&["--set", "w", "drawing=off"]);
    let fx = h.input(Input::DisplaysChanged);
    assert!(senders(&fx, "w").is_empty());
}

fn two_spaces() -> HeadlessResources {
    let mut res = HeadlessResources::default();
    res.displays[0].current_space = 10;
    res.spaces = vec![
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
    ];
    res
}

#[test]
fn rt2_non_sticky_bar_moves_to_new_space() {
    let mut h = H::with_res(two_spaces());
    h.msg(&[
        "--bar",
        "sticky=off",
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "label=A",
        "popup.drawing=on",
        "--add",
        "item",
        "p",
        "popup.a",
        "--set",
        "p",
        "label=P",
    ]);
    h.frame();
    let host = h.rt.model.find("a").unwrap();
    h.res.displays[0].current_space = 11;
    h.input(Input::Event(OsEvent::SpaceChanged));
    assert!(h.rt.needs_frame());
    let out = h.frame();
    assert_eq!(
        out.space_moves,
        vec![
            SpaceMove {
                key: WindowKey::Bar(1),
                dsid: 11
            },
            SpaceMove {
                key: WindowKey::Popup(host),
                dsid: 11
            },
        ]
    );
    assert_eq!(h.rt.model.bars[0].sid, 2);
    assert_eq!(h.rt.model.bars[0].dsid, 11);
    // Once only; a repeated notification for the same space moves nothing.
    h.input(Input::Event(OsEvent::SpaceChanged));
    assert!(h.frame().space_moves.is_empty());
}

#[test]
fn rt2_sticky_bar_is_not_moved() {
    let mut h = H::with_res(two_spaces());
    h.msg(&["--add", "item", "a", "left", "--set", "a", "label=A"]);
    assert!(h.rt.model.bar.sticky);
    h.res.displays[0].current_space = 11;
    h.input(Input::Event(OsEvent::SpaceChanged));
    assert!(h.frame().space_moves.is_empty());
    assert_eq!(h.rt.model.bars[0].dsid, 11);
}

#[test]
fn rt4_update_in_add_batch_reaches_when_shown_item() {
    let mut res = HeadlessResources::default();
    res.system
        .insert("FrontApp".into(), SystemValue::Text("Finder".into()));
    let mut h = H::with_res(res);
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "label=A",
        "script=a.sh",
        "updates=when_shown",
        "--subscribe",
        "a",
        "front_app_switched",
        "--update",
    ]);
    assert_eq!(senders(&fx, "a"), ["front_app_switched", "forced"]);
    let runs = runs_of(&fx, "a");
    assert_eq!(runs[0].get("INFO"), Some("Finder"));
}

#[test]
fn rt4_events_after_trigger_space_change_in_add_batch_reach_when_shown_item() {
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "label=A",
        "script=a.sh",
        "updates=when_shown",
        "--subscribe",
        "a",
        "space_change",
        "front_app_switched",
        "--trigger",
        "space_change",
        "--trigger",
        "front_app_switched",
    ]);
    // `space_change` itself is dispatched before the refresh that ends the batch freeze
    // (the new item is not shown yet, as in SketchyBar); later events see it shown.
    assert_eq!(senders(&fx, "a"), ["front_app_switched"]);
}

fn popup_setup() -> (H, ItemId) {
    let mut h = H::new();
    for i in 0..6 {
        let n = format!("item{i}");
        h.msg(&["--add", "item", &n, "left", "--set", &n, "label=X"]);
    }
    for i in 0..3 {
        let n = format!("pm{i}");
        h.msg(&["--add", "item", &n, "popup.item0", "--set", &n, "label=P"]);
    }
    h.msg(&["--set", "item0", "popup.drawing=on"]);
    let host = h.rt.model.find("item0").unwrap();
    assert!(h
        .last_frame
        .windows
        .iter()
        .any(|w| w.key == WindowKey::Popup(host)));
    // The first anchoring marks every member for one more redraw (`popup_set_anchor`
    // with a new adid); let that settle.
    h.msg(&["--set", "item5", "label=Y"]);
    (h, host)
}

fn keys(h: &H) -> Vec<WindowKey> {
    h.last_frame.windows.iter().map(|w| w.key).collect()
}

#[test]
fn perf4_bar_item_change_does_not_redraw_popup() {
    let (mut h, _) = popup_setup();
    h.msg(&["--set", "item3", "icon.color=0xffff0000"]);
    assert_eq!(keys(&h), [WindowKey::Bar(1)]);
}

#[test]
fn perf4_popup_member_change_does_not_redraw_bar() {
    let (mut h, host) = popup_setup();
    h.msg(&["--set", "pm1", "label=xI"]);
    assert_eq!(keys(&h), [WindowKey::Popup(host)]);
    h.msg(&["--set", "pm1", "label.color=0xff00ff00"]);
    assert_eq!(keys(&h), [WindowKey::Popup(host)]);
    // Hiding a member still updates the popup (and its association bits).
    h.msg(&["--set", "pm2", "drawing=off"]);
    assert_eq!(keys(&h), [WindowKey::Popup(host)]);
    let pm2 = h.rt.model.find("pm2").unwrap();
    assert_eq!(h.rt.model.item(pm2).unwrap().associated_bar, 0);
}

#[test]
fn perf4_reshown_popup_member_is_associated_without_bar_redraw() {
    let (mut h, host) = popup_setup();
    let pm2 = h.rt.model.find("pm2").unwrap();
    h.msg(&["--set", "pm2", "drawing=off"]);
    assert_eq!(h.rt.model.item(pm2).unwrap().associated_bar, 0);
    h.msg(&["--set", "pm2", "drawing=on"]);
    assert_eq!(keys(&h), [WindowKey::Popup(host)]);
    assert_ne!(h.rt.model.item(pm2).unwrap().associated_bar, 0);
}

#[test]
fn perf4_clipping_popup_member_still_redraws_bar() {
    let (mut h, host) = popup_setup();
    h.msg(&[
        "--set",
        "pm0",
        "background.drawing=on",
        "background.color=0xff000000",
        "background.clip=1.0",
    ]);
    let k = keys(&h);
    assert!(k.contains(&WindowKey::Bar(1)), "{k:?}");
    assert!(k.contains(&WindowKey::Popup(host)), "{k:?}");
    // Changing the clipping member again re-renders the bar (its hole may have moved).
    h.msg(&["--set", "pm0", "label=longer label"]);
    assert!(keys(&h).contains(&WindowKey::Bar(1)));
    // Turning the clip off removes the hole: one more bar render, then none.
    h.msg(&["--set", "pm0", "background.clip=0.0"]);
    assert!(keys(&h).contains(&WindowKey::Bar(1)));
    h.msg(&["--set", "pm0", "label=short"]);
    assert_eq!(keys(&h), [WindowKey::Popup(host)]);
}

#[test]
fn perf6_regex_set_applies_to_every_match_in_order() {
    let mut h = H::new();
    for i in 0..30 {
        let n = format!("item{i}");
        h.msg(&["--add", "item", &n, "left"]);
    }
    for _ in 0..3 {
        assert_eq!(
            h.msg(&["--set", "/item.*/", "label=L", "icon.padding_left=2"]),
            ""
        );
    }
    for i in 0..30 {
        let q = h.query(&["item", &format!("item{i}")]);
        assert_eq!(q["label"]["value"], "L");
        assert_eq!(q["icon"]["padding_left"], 2);
    }
    assert_eq!(
        h.msg(&["--set", "/nomatch$/", "label=x"]),
        "[?] Regex: No match found for regex '/nomatch$/'\n"
    );
    // A bad pattern is reported every time (failures are not cached).
    for _ in 0..2 {
        assert_eq!(
            h.msg(&["--set", "/item\\(/", "label=x"]),
            "[!] Regex: Could not compile regex '/item\\(/'\n"
        );
    }
}

#[test]
fn perf6_regex_set_survives_items_moving_between_tokens() {
    let mut h = H::new();
    h.msg(&["--add", "item", "host", "left"]);
    for i in 0..4 {
        let n = format!("m{i}");
        h.msg(&["--add", "item", &n, "left"]);
    }
    // `position=popup.host` moves each item into the popup (reorders the item list)
    // before the next token is applied to it.
    assert_eq!(
        h.msg(&["--set", "/^m/", "position=popup.host", "label=after"]),
        ""
    );
    for i in 0..4 {
        let q = h.query(&["item", &format!("m{i}")]);
        assert_eq!(q["label"]["value"], "after");
        assert_eq!(q["geometry"]["position"], "popup");
    }
}
