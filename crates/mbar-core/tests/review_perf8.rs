//! PERF-8: popup visibility (`Model::draws_item`) resolved each member's parent with a
//! linear scan, making every refresh/layout O(members x items). Item lookups now go
//! through a validated id index; these tests pin both the O(1) steady state and the
//! visibility semantics across structural changes.

mod wpc_common;

use wpc_common::H;

/// 200 bar items plus 50 hosts with 4 open popup members each.
fn popup_heavy() -> H {
    let mut h = H::new();
    for i in 0..200 {
        h.msg(&["--add", "item", &format!("b{i}"), "left"]);
    }
    for i in 0..50 {
        let host = format!("h{i}");
        h.msg(&["--add", "item", &host, "right"]);
        for j in 0..4 {
            let m = format!("m{i}_{j}");
            h.msg(&["--add", "item", &m, &format!("popup.{host}")]);
            h.msg(&["--set", &m, &format!("label={m}")]);
        }
        h.msg(&["--set", &host, "popup.drawing=on"]);
    }
    h.frame();
    h.frame();
    h
}

#[test]
fn idle_frames_and_sets_do_not_rescan_items() {
    let mut h = popup_heavy();
    let before = h.rt.model.index_rebuilds();
    for _ in 0..50 {
        h.frame();
    }
    for n in 0..50 {
        h.msg(&["--set", "b0", &format!("label={n}")]);
        h.frame();
    }
    assert_eq!(
        h.rt.model.index_rebuilds(),
        before,
        "item lookups rebuilt the id index without a structural change"
    );
}

#[test]
fn popup_visibility_follows_host_across_reorder_and_removal() {
    let mut h = popup_heavy();
    let drawn = |h: &H, name: &str| {
        let m = &h.rt.model;
        let it = m.item(m.find(name).unwrap()).unwrap();
        m.bars.iter().any(|b| m.draws_item(b, it))
    };
    assert!(drawn(&h, "m3_0"));
    // Reordering moves the host; members must still resolve it.
    assert_eq!(h.msg(&["--reorder", "h3", "b0"]), "");
    h.frame();
    assert!(drawn(&h, "m3_0"));
    assert_eq!(h.msg(&["--move", "h3", "after", "b10"]), "");
    h.frame();
    assert!(drawn(&h, "m3_1"));
    // D3: hiding the host hides its popup members.
    h.msg(&["--set", "h3", "drawing=off"]);
    h.frame();
    assert!(!drawn(&h, "m3_2"));
    h.msg(&["--set", "h3", "drawing=on"]);
    h.frame();
    assert!(drawn(&h, "m3_2"));
    // Removing another host does not confuse lookups of the shifted items.
    assert_eq!(h.msg(&["--remove", "h0"]), "");
    h.frame();
    assert!(h.rt.model.find("h0").is_none());
    assert!(drawn(&h, "m3_3"));
    assert!(drawn(&h, "m49_3"));
    h.msg(&["--set", "h49", "popup.drawing=off"]);
    h.frame();
    assert!(!drawn(&h, "m49_3"));
}
