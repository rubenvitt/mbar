//! Regression tests for review finding R3: `shadow_extents` negated `background.x_offset`
//! in i32, overflowing (debug panic in layout) for `i32::MIN`.

mod wpc_common;

use mbar_core::item::{BarItem, ItemId};
use std::time::Duration;
use wpc_common::H;

#[test]
fn shadow_extents_bg_x_offset_i32_min() {
    let mut it = BarItem::new(ItemId(0));
    it.background.enabled = true;
    it.background.x_offset = i32::MIN;
    // 2^31 does not fit in i32; the final float->int cast saturates.
    assert_eq!(it.shadow_extents(), (i32::MAX, 0));
    it.background.x_offset = -7;
    assert_eq!(it.shadow_extents(), (7, 0));
    it.background.x_offset = i32::MAX;
    assert_eq!(it.shadow_extents(), (0, i32::MAX));
}

#[test]
fn bg_x_offset_i32_min_layout_does_not_panic() {
    for v in ["-2147483648", "2147483648", "0x80000000"] {
        let mut h = H::new();
        h.msg(&["--add", "item", "a", "left"]);
        h.msg(&[
            "--set",
            "a",
            "background.drawing=on",
            &format!("background.x_offset={v}"),
        ]);
        h.frame();
    }
}

#[test]
fn bg_x_offset_overshoot_animation_does_not_panic() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "background.drawing=on",
    ]);
    h.msg(&[
        "--animate",
        "overshoot",
        "30",
        "--set",
        "a",
        "background.x_offset=-2147483000",
    ]);
    h.advance_by(Duration::from_secs(1), Duration::from_millis(16));
}
