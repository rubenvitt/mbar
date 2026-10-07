//! Geometry and scene generation — **WP-A** (`docs/IMPLEMENTATION-PLAN.md`).
//!
//! Spec: `docs/spec/bar.md` §4 (bar frame, horizontal/vertical layout, brackets, popup
//! anchors), `docs/spec/item.md` §4–6 (item internals, popups), `docs/spec/components.md`
//! §4.8–§10 (component bounds and drawing). Deviations: D2 (consistent slider hit test),
//! D3 (popups hidden with their host), D4 (nested popups anchored in the same pass),
//! D10 (negative unsigned results clamp to 0), D11 (`get_height` uses the current pass).
//!
//! Coordinate conventions:
//! * Screen: points, top-left origin (CG global). Bar/popup/item frames are screen rects.
//! * Component `bounds` fields: item-local drawing coordinates like C (origin bottom-left
//!   of the item's virtual window, y up) so the C formulas transcribe 1:1.
//! * Scenes: window-local, top-left origin (`y_top = window_h - y_cg`; rects:
//!   `y_top = window_h - (y + h)`). A bar scene contains every item drawn on that bar,
//!   translated by `item_frame.origin - bar_frame.origin`.
//!
//! Every item gets a virtual window frame on every bar (`BarItem::set_frame`), parked at
//! [`NIRVANA`] when not drawn there, so `bounding_rects` matches SketchyBar.
//!
//! Numeric conventions (C emulation):
//! * Pure `uint32_t` integer arithmetic is reproduced with wrapping operations where the
//!   wrap is part of the algorithm (the RTL cursor clamp, cursor advances).
//! * D10: every *conversion* of a negative (or NaN) value to an unsigned type clamps to 0
//!   (Rust's saturating `as u32`, the arm64 behaviour); integer subtractions that the spec
//!   lists as "wrapping into invisibility" (background `y - h/2`, `y + y_offset`, the
//!   auto heights `bar_height - (border_width + 1)`) saturate at 0 as well.

use crate::bar::{BarProps, BarState};
use crate::color::Color;
use crate::components::{Background, Image, Slider, Text};
use crate::geometry::{Point, Rect, Size};
use crate::item::{BarItem, ItemId, ItemType, Position};
use crate::model::Model;
use crate::platform::{level, DisplayInfo, ImageInfo, Resources, TextKey};
use crate::scene::{Primitive, Scene};

/// `g_nirvana`: where invisible windows are parked.
pub const NIRVANA: Point = Point {
    x: -9999.0,
    y: -9999.0,
};

/// Recursion limit for nested popups (C recurses without limit and crashes on cycles).
const MAX_POPUP_DEPTH: u32 = 16;

/// Vertical padding (each side) of an `app_menu` title cell around the text box.
const APP_MENU_PAD: f32 = 3.0;

/// Result of one layout pass over all bars and open popups.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub bars: Vec<BarLayout>,
    pub popups: Vec<PopupLayout>,
}

/// One bar window.
#[derive(Debug, Clone, PartialEq)]
pub struct BarLayout {
    pub adid: u32,
    /// Bar window frame (screen); parked at nirvana when hidden / not shown.
    pub frame: Rect,
    /// Items drawn on this bar (`bar_draws_item` true), in global order; brackets included
    /// (their frame spans the members). The runtime sets `associated_bar` bit `adid-1` for
    /// these and clears it for all others. Popup members drawn on the active bar are listed
    /// too (C sets their bar bit as well); they are painted by [`popup_scene`], not by
    /// [`bar_scene`].
    pub items: Vec<PlacedItem>,
    /// Measured `app_menu` title lines of the `app_menu` items in `items` (extension), so
    /// the pure scene builder can reference the platform's text handles.
    pub menu_lines: Vec<(ItemId, Vec<MenuLine>)>,
}

/// An item's placement in a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedItem {
    pub id: ItemId,
    /// Virtual window frame (screen), incl. shadow extents.
    pub frame: Rect,
}

/// One open popup window (`popup.drawing`, anchored, ≥ 1 item).
#[derive(Debug, Clone, PartialEq)]
pub struct PopupLayout {
    pub host: ItemId,
    pub adid: u32,
    /// Popup window frame (screen) = `anchor` + background size.
    pub frame: Rect,
    /// Window level (`popup.topmost`).
    pub level: i32,
    /// Drawn members in popup order (brackets included).
    pub items: Vec<PlacedItem>,
    /// Measured `app_menu` title lines of `app_menu` members (extension).
    pub menu_lines: Vec<(ItemId, Vec<MenuLine>)>,
}

/// One measured `app_menu` title (extension): the platform text handle and the descent
/// needed to place its baseline inside the title cell (`AppMenu::title_bounds`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuLine {
    pub key: TextKey,
    pub descent: f32,
}

/// What lies under a screen point, emulating SketchyBar's window z-order (popup windows
/// and their items above bar item windows, later items above earlier ones, brackets below
/// the first item, the bar window at the bottom). Used where C used `wid` lookups.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowHit {
    /// An item's virtual window (`get_item_by_wid`).
    Item(ItemId),
    /// A popup window background (`get_popup_by_wid`), by host.
    Popup(ItemId),
    /// A bar window background (`get_bar_by_wid`), by adid.
    Bar(u32),
    None,
}

// ---------------------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------------------

/// `(uint32_t)<double>` with D10 semantics (negative / NaN → 0, saturating above).
fn to_u32(v: f64) -> u32 {
    v as u32
}

/// `u32 + i32` that C would wrap below 0; D10: clamp at 0.
fn add_signed(a: u32, b: i32) -> u32 {
    (a as i64 + b as i64).clamp(0, u32::MAX as i64) as u32
}

/// `CGRectContainsPoint`: half-open on the max edges.
fn contains_half_open(r: &Rect, p: Point) -> bool {
    p.x >= r.x && p.x < r.x + r.width && p.y >= r.y && p.y < r.y + r.height
}

fn nirvana_rect(size: Size) -> Rect {
    Rect::new(NIRVANA.x, NIRVANA.y, size.width, size.height)
}

fn artwork_size(model: &Model) -> Option<Size> {
    model.current_artwork.map(|a| a.size)
}

// ---------------------------------------------------------------------------------------
// Item metrics used by layout (app_menu extension + D11)
// ---------------------------------------------------------------------------------------

/// Width of the `app_menu` title cells measured this pass (the "middle" component).
fn app_menu_len(item: &BarItem) -> u32 {
    if item.item_type != ItemType::AppMenu {
        return 0;
    }
    item.app_menu
        .title_bounds
        .iter()
        .fold(0u32, |a, r| a.wrapping_add(to_u32(r.width as f64)))
}

/// `bar_item_get_content_length` (includes `app_menu` title cells, see `BarItem::content_length`).
fn content_len(item: &BarItem) -> u32 {
    item.content_length()
}

/// `bar_item_get_length(item, ignore_override)` (`item.md` §4.1) on top of [`content_len`];
/// identical to `BarItem::length` for every non-`app_menu` item.
fn item_len(item: &BarItem, ignore_override: bool) -> u32 {
    let mut c = content_len(item);
    if item.background.enabled && item.background.image.enabled {
        let iw = item.background.image.reserved_size().width;
        if iw > c as f32 {
            c = to_u32(iw as f64);
        }
    }
    if item.has_const_width && (!ignore_override || item.custom_width > c) {
        return item.custom_width;
    }
    c
}

/// `bar_item_get_height` with D11: C reads the item background height of the *previous*
/// pass. The current pass gives an automatic background `container - (border_width + 1)`,
/// which never exceeds the container (popup cell / vertical slot) that this very height
/// determines, so an automatic background contributes nothing; an explicit `height=`
/// and the background image still count (`item.md` §4.1).
fn layout_height(item: &BarItem) -> u32 {
    let text = item.label.height().max(item.icon.height());
    let alias = if item.has_alias() {
        item.alias.height()
    } else {
        0
    };
    let bg = if item.background.enabled {
        let ih = if item.background.image.enabled {
            to_u32(item.background.image.reserved_size().height as f64)
        } else {
            0
        };
        let bh = if item.background.overrides_height {
            item.background.height
        } else {
            0
        };
        ih.max(bh)
    } else {
        0
    };
    let menu = if item.item_type == ItemType::AppMenu {
        item.app_menu
            .title_bounds
            .iter()
            .map(|r| to_u32(r.height as f64))
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    text.max(alias).max(bg).max(menu)
}

/// The `app_menu` entries drawn by an item: `visible_titles()`; entry `1` (the
/// application menu, whose title is the app name) is drawn with `app_font` and the
/// `app_name` text (extension, `docs/EXTENSIONS.md`).
fn app_menu_entries(item: &BarItem) -> Vec<(usize, String, bool)> {
    let am = &item.app_menu;
    am.visible_titles()
        .into_iter()
        .map(|(i, t)| {
            if i == 1 && !am.app_name.is_empty() {
                (i, am.app_name.clone(), true)
            } else {
                (i, t.to_string(), i == 1)
            }
        })
        .collect()
}

/// Measures the `app_menu` titles: one cell per entry, `typographic width + spacing` wide
/// and `ascent + descent + 2·APP_MENU_PAD` tall (positions are set by
/// [`item_calculate_bounds`]). Returns the measured lines.
fn prepare_app_menu(item: &mut BarItem, res: &mut dyn Resources) -> Vec<MenuLine> {
    if item.item_type != ItemType::AppMenu {
        return Vec::new();
    }
    let title_font = item.app_menu.title_font(&item.label.font);
    let app_font = item.app_menu.app_name_font(&item.label.font);
    let spacing = item.app_menu.spacing.max(0) as f32;
    let mut rects = Vec::new();
    let mut lines = Vec::new();
    for (_, text, bold) in app_menu_entries(item) {
        let font = if bold { &app_font } else { &title_font };
        let m = res.text_metrics(font, &text);
        let w = to_u32(m.typographic_width as f64 + 0.5) as f32 + spacing;
        let h = m.ascent + m.descent + 2.0 * APP_MENU_PAD;
        rects.push(Rect::new(0.0, 0.0, w, h));
        lines.push(MenuLine {
            key: m.key,
            descent: m.descent,
        });
    }
    item.app_menu.title_bounds = rects;
    lines
}

// ---------------------------------------------------------------------------------------
// Bar frame
// ---------------------------------------------------------------------------------------

/// `bar_get_frame` (`bar.md` §4.1): horizontal (`t`/`b`/other) and vertical (`l`/`r`) bar
/// window frames incl. margin, `y_offset` (bottom uses `2*y_offset`), notch offset/height on
/// built-in displays, and the menu-bar offset (`menu_bar_visible && !topmost`).
pub fn bar_frame(bar: &BarProps, display: &DisplayInfo, menu_bar_visible: bool) -> Rect {
    let b = display.frame;
    let (bx, by, bw, bh) = (b.x as f64, b.y as f64, b.width as f64, b.height as f64);
    let h = bar.height() as f64;
    let m = bar.margin as f64;
    let y_off = bar.background.y_offset as f64;
    let (notch_off, notch_h) = if display.builtin {
        (bar.notch_offset as f64, bar.notch_display_height)
    } else {
        (0.0, 0)
    };
    let mh = display.menu_bar_height as f64;
    let avoid_menu = menu_bar_visible && !bar.topmost;

    if bar.is_vertical() {
        // Notch values are ignored for vertical bars.
        let mut fh = bh - 2.0 * y_off;
        let x = bx + if bar.position == b'r' { bw - h - m } else { m };
        let mut y = by + y_off;
        if avoid_menu {
            y += mh;
            fh -= mh;
        }
        Rect::new(x as f32, y as f32, h as f32, fh as f32)
    } else {
        let x = bx + m;
        let w = bw - 2.0 * m;
        let y = if bar.position == b'b' {
            // Quirk: 2*y_offset, and H even when notch_display_height > 0.
            by + bh - h - 2.0 * y_off - notch_off
        } else {
            let mut y = by + y_off + notch_off;
            if avoid_menu {
                y += mh;
            }
            y
        };
        let fh = if notch_h > 0 { notch_h as f64 } else { h };
        Rect::new(x as f32, y as f32, w as f32, fh as f32)
    }
}

// ---------------------------------------------------------------------------------------
// Full pass
// ---------------------------------------------------------------------------------------

/// Full layout pass (`bar_calculate_bounds` for every bar + popups), writing item frames
/// (`set_frame`), component bounds, `graph.rtl`, popup `cell_size`/`anchor`/`adid`/`frame`,
/// and the auto background heights. Calls `BarItem::ensure_layout` on every item first.
/// Bars with `sid < 1 || adid < 1` are skipped (never drawn). Hidden / not-shown bars park
/// their frame at nirvana.
///
/// After the pass `popup.frame` is `Some` exactly for the popups in [`Layout::popups`]
/// (D3: a popup whose host is not drawn has no window).
pub fn layout(model: &mut Model, res: &mut dyn Resources) -> Layout {
    prepare(model, res);
    let vertical = model.bar.is_vertical();
    let mut out = Layout::default();
    for i in 0..model.bars.len() {
        let b = &model.bars[i];
        if b.sid < 1 || b.adid < 1 {
            continue;
        }
        let bl = if vertical {
            layout_bar_vertical(model, i, res)
        } else {
            layout_bar_horizontal(model, i, res)
        };
        out.bars.push(bl);
    }
    out.popups = collect_popups(model, res);
    out
}

/// `ensure_layout` + app_menu measurement for every item.
fn prepare(model: &mut Model, res: &mut dyn Resources) {
    for item in model.items.iter_mut() {
        item.ensure_layout(res);
        prepare_app_menu(item, res);
    }
}

/// Window frame of a bar as placed on screen (`bar_resize`: hidden / not shown → nirvana).
fn bar_window_frame(bar: &BarState) -> Rect {
    if bar.hidden || !bar.shown {
        nirvana_rect(bar.frame.size())
    } else {
        bar.frame
    }
}

fn is_builtin(bar: &BarState, res: &dyn Resources) -> bool {
    let displays = res.displays();
    displays
        .iter()
        .find(|d| d.id == bar.display)
        .or_else(|| displays.iter().find(|d| d.adid == bar.adid))
        .is_some_and(|d| d.builtin)
}

/// The cursor-advancing part shared by both orientations: `cur = rtl ? min(cur - len - pr,
/// limit - len) : max(cur + pl, 0)` with C's unsigned arithmetic (`bar.md` §4.4).
fn place_cursor(cur: u32, len: u32, pl: i32, pr: i32, rtl: bool, limit: f64) -> u32 {
    if rtl {
        // `(u32)(cur - len - (u32)pr)` wraps; a wrapped candidate is huge, so the double
        // comparison selects `limit - len` (overflowing right items jump to the edge).
        let cand = cur.wrapping_sub(len).wrapping_sub(pr as u32);
        let lim = limit - len as f64;
        if (cand as f64) < lim {
            cand
        } else {
            to_u32(lim) // D10: negative → 0
        }
    } else {
        // `cur + max(-(int)cur, padding_left)` == `max(cur + pl, 0)`.
        cur.wrapping_add((cur as i32).wrapping_neg().max(pl) as u32)
    }
}

/// Cursor advance after an item (`bar.md` §4.4): `len` is the display length (horizontal)
/// or the item height (vertical), `slot` the calculate_bounds result (horizontal) or the
/// height (vertical).
fn advance_cursor(cur: u32, item: &BarItem, rtl: bool, len: u32, slot: u32) -> u32 {
    let pl = item.background.padding_left;
    let pr = item.background.padding_right;
    let cw = item.custom_width;
    let delta = match (rtl, item.has_const_width) {
        (true, true) => len.wrapping_add(pr as u32).wrapping_sub(cw),
        (true, false) => pl.wrapping_neg() as u32,
        (false, true) => cw.wrapping_sub(pl as u32),
        (false, false) => slot.wrapping_add(pr as u32),
    };
    cur.wrapping_add(delta)
}

/// `bar_calculate_bounds_top_bottom` (`bar.md` §4.4, `item.md` §4.5): cursors per position
/// (`l`, `c`, `r`, `q`/`e` around the notch on built-in displays), RTL for `r`/`q`,
/// padding/const-width advance rules, unsigned-wrap emulation of the RTL clamp (D10: clamp
/// at 0 instead of UB), then the bracket pass (§4.6). Returns the placed items.
pub fn layout_bar_horizontal(
    model: &mut Model,
    bar_index: usize,
    res: &mut dyn Resources,
) -> BarLayout {
    prepare(model, res);
    let bar = model.bars[bar_index].clone();
    if bar.sid < 1 || bar.adid < 1 {
        return empty_bar_layout(&bar);
    }
    let art = artwork_size(model);
    let w = bar.frame.width as f64;
    let h = bar.frame.height as f64;
    let notch = if is_builtin(&bar, res) {
        model.bar.notch_width as f64
    } else {
        0.0
    };
    let center_len = side_length(model, &bar, Position::Center, false) as f64;
    let bbg = &model.bar.background;
    let bbw = bbg.border_width;
    let mut cur_l = bbg.padding_left.max(0) as u32;
    let mut cur_r = to_u32(w - bbg.padding_right.max(0) as f64);
    let mut cur_c = to_u32((w - center_len) / 2.0);
    let mut cur_e = to_u32((w + notch) / 2.0);
    let mut cur_q = to_u32((w - notch) / 2.0);
    let y = to_u32(h / 2.0);
    let bar_height = to_u32(h - (bbw as f64 + 1.0));

    for i in 0..model.items.len() {
        let eligible = {
            let it = &model.items[i];
            !it.is_bracket() && it.position != Position::Popup && model.draws_item(&bar, it)
        };
        if !eligible {
            continue;
        }
        let item = &mut model.items[i];
        let cur = match item.position {
            Position::Left => &mut cur_l,
            Position::Center => &mut cur_c,
            Position::Right => &mut cur_r,
            Position::CenterRight => &mut cur_e,
            Position::CenterLeft => &mut cur_q,
            Position::Popup => continue,
        };
        let disp = item_len(item, true);
        let rtl = item.position.is_rtl();
        let (pl, pr) = (item.background.padding_left, item.background.padding_right);
        *cur = place_cursor(*cur, disp, pl, pr, rtl, w);
        item.graph.rtl = rtl;
        let (sl, sr) = item.shadow_extents();
        let l = sl.max(0);
        let slot = calc_bounds(item, bar_height, l as u32, y, bbw, art);
        let frame = Rect::new(
            (bar.frame.x as f64 + *cur as f64 - l as f64) as f32,
            bar.frame.y,
            (disp as f64 + sl as f64 + sr as f64) as f32,
            h as f32,
        );
        item.set_frame(bar.adid, frame);
        *cur = advance_cursor(*cur, item, rtl, disp, slot);
        let (id, popup) = (item.id, item.popup.drawing);
        if popup {
            anchor_bar_popup(model, id, &bar, res);
        }
    }

    // Second pass: brackets (`group_calculate_bounds`, §4.6).
    for i in 0..model.items.len() {
        let (id, eligible) = {
            let it = &model.items[i];
            (
                it.id,
                it.is_bracket() && it.position != Position::Popup && model.draws_item(&bar, it),
            )
        };
        if !eligible {
            continue;
        }
        let r = bracket_bounds_impl(model, id, &bar, y, art);
        model.items[i].set_frame(bar.adid, r);
        if model.items[i].popup.drawing {
            anchor_bar_popup(model, id, &bar, res);
        }
    }
    finish_bar(model, &bar, res)
}

/// `bar_calculate_bounds_left_right` (`bar.md` §4.5, `item.md` §4.6): vertical bars; centre
/// start `(H - 2*margin - len)/2 - 1`; no bracket pass; window width = bar thickness.
pub fn layout_bar_vertical(
    model: &mut Model,
    bar_index: usize,
    res: &mut dyn Resources,
) -> BarLayout {
    prepare(model, res);
    let bar = model.bars[bar_index].clone();
    if bar.sid < 1 || bar.adid < 1 {
        return empty_bar_layout(&bar);
    }
    let art = artwork_size(model);
    let hwin = bar.frame.height as f64;
    // `background.bounds.size.height`: the bar thickness setting (window width).
    let t = model.bar.background.height as f64;
    let margin = model.bar.margin as f64;
    let center_len = side_length(model, &bar, Position::Center, true) as f64;
    let bbg = &model.bar.background;
    let bbw = bbg.border_width;
    let mut cur_l = bbg.padding_left.max(0) as u32;
    let mut cur_r = to_u32(hwin - bbg.padding_right.max(0) as f64);
    // Quirk: subtracts 2*margin and 1.
    let mut cur_c = to_u32((hwin - 2.0 * margin - center_len) / 2.0 - 1.0);
    let mut cur_e = to_u32(hwin / 2.0);
    let mut cur_q = cur_e;

    for i in 0..model.items.len() {
        let eligible = {
            let it = &model.items[i];
            !it.is_bracket() && it.position != Position::Popup && model.draws_item(&bar, it)
        };
        if !eligible {
            continue;
        }
        let item = &mut model.items[i];
        let cur = match item.position {
            Position::Left => &mut cur_l,
            Position::Center => &mut cur_c,
            Position::Right => &mut cur_r,
            Position::CenterRight => &mut cur_e,
            Position::CenterLeft => &mut cur_q,
            Position::Popup => continue,
        };
        let ih = layout_height(item);
        let disp = item_len(item, true);
        let rtl = item.position.is_rtl();
        let (pl, pr) = (item.background.padding_left, item.background.padding_right);
        *cur = place_cursor(*cur, ih, pl, pr, rtl, hwin);
        item.graph.rtl = rtl;
        let (sl, _) = item.shadow_extents();
        let l = sl.max(0);
        let x = to_u32((t - disp as f64) / 2.0 + l as f64);
        let yy = to_u32(ih as f64 / 2.0);
        calc_bounds(item, ih, x, yy, bbw, art);
        let frame = Rect::new(
            (bar.frame.x as f64 - l as f64) as f32,
            (bar.frame.y as f64 + *cur as f64 - (-item.y_offset).max(0) as f64) as f32,
            t as f32,
            (ih as f64 + (item.y_offset as f64).abs()) as f32,
        );
        item.set_frame(bar.adid, frame);
        // Const-width semantics are applied to heights (quirk).
        *cur = advance_cursor(*cur, item, rtl, ih, ih);
        let (id, popup) = (item.id, item.popup.drawing);
        if popup {
            anchor_bar_popup(model, id, &bar, res);
        }
    }
    // Quirk: no bracket pass; bracket windows keep their last frame.
    finish_bar(model, &bar, res)
}

fn empty_bar_layout(bar: &BarState) -> BarLayout {
    BarLayout {
        adid: bar.adid,
        frame: bar_window_frame(bar),
        items: Vec::new(),
        menu_lines: Vec::new(),
    }
}

/// `bar_draw` step 3 geometry: items not drawn on this bar are parked at nirvana (size
/// kept; new windows are 1×1), drawn items without a window get one at nirvana; then the
/// placed list (global order) is built.
fn finish_bar(model: &mut Model, bar: &BarState, res: &mut dyn Resources) -> BarLayout {
    let adid = bar.adid;
    let draws: Vec<bool> = model
        .items
        .iter()
        .map(|i| model.draws_item(bar, i))
        .collect();
    let mut out = empty_bar_layout(bar);
    for (item, drawn) in model.items.iter_mut().zip(draws) {
        let prev = item.frame(adid);
        if drawn {
            let frame = prev.unwrap_or_else(|| nirvana_rect(Size::new(1.0, 1.0)));
            item.set_frame(adid, frame);
            out.items.push(PlacedItem { id: item.id, frame });
            if item.item_type == ItemType::AppMenu {
                let lines = prepare_app_menu_lines(item, res);
                out.menu_lines.push((item.id, lines));
            }
        } else {
            let size = prev.map_or(Size::new(1.0, 1.0), |r| r.size());
            item.set_frame(adid, nirvana_rect(size));
        }
    }
    out
}

/// Measures the app_menu lines without disturbing the laid-out title cells.
fn prepare_app_menu_lines(item: &mut BarItem, res: &mut dyn Resources) -> Vec<MenuLine> {
    let cells = item.app_menu.title_bounds.clone();
    let lines = prepare_app_menu(item, res);
    if cells.len() == item.app_menu.title_bounds.len() {
        item.app_menu.title_bounds = cells;
    }
    lines
}

/// `bar_manager_length_for_bar_side` (`bar.md` §4.3): sum over items with the given
/// position, not brackets, drawn on `bar`, of `len + (const ? 0 : pl + pr)` (`len` = height
/// for vertical bars; D11 height, see `layout_height`).
pub fn side_length(
    model: &Model,
    bar: &BarState,
    side: crate::item::Position,
    vertical: bool,
) -> u32 {
    let mut sum = 0u32;
    for item in &model.items {
        if item.position != side || item.is_bracket() || !model.draws_item(bar, item) {
            continue;
        }
        let len = if vertical {
            layout_height(item)
        } else {
            item_len(item, false)
        };
        let pad = if item.has_const_width {
            0
        } else {
            (item.background.padding_left as u32).wrapping_add(item.background.padding_right as u32)
        };
        sum = sum.wrapping_add(len).wrapping_add(pad);
    }
    sum
}

// ---------------------------------------------------------------------------------------
// Item internals
// ---------------------------------------------------------------------------------------

/// `bar_item_calculate_bounds(item, bar_height, x, y)` (`item.md` §4.3): aligns content in a
/// widened slot, places icon / graph|alias|slider / label, the item background (auto height
/// `bar_height - (bar.border_width + 1)`, double-subtraction quirk) and returns the slot
/// length (`length(false)`). `bar_border_width` is the bar background's border width.
pub fn item_calculate_bounds(
    item: &mut BarItem,
    bar_height: u32,
    x: u32,
    y: u32,
    bar_border_width: u32,
) -> u32 {
    calc_bounds(item, bar_height, x, y, bar_border_width, None)
}

fn calc_bounds(
    item: &mut BarItem,
    bar_height: u32,
    x: u32,
    y: u32,
    bbw: u32,
    art: Option<Size>,
) -> u32 {
    let length = item_len(item, false);
    let content = content_len(item);
    let mut content_x = x;
    if length > content {
        match item.align {
            b'c' => content_x = content_x.wrapping_add((length - content) / 2),
            b'r' => content_x = content_x.wrapping_add(length - content),
            _ => {}
        }
    }
    let icon_x = content_x;
    let mid_x = icon_x.wrapping_add(item.icon.length(false));
    let mid_len = if item.has_graph() {
        if item.graph.enabled {
            item.graph.width
        } else {
            0
        }
    } else if item.has_alias() {
        item.alias.length()
    } else if item.has_slider() {
        item.slider.width
    } else {
        app_menu_len(item)
    };
    let label_x = mid_x.wrapping_add(mid_len);
    let ty = add_signed(y, item.y_offset); // D10

    text_bounds(&mut item.icon, icon_x, ty, art);
    text_bounds(&mut item.label, label_x, ty, art);
    if item.has_alias() {
        image_bounds(&mut item.alias.image, mid_x as f32, ty as f32, None);
    }
    if item.has_slider() {
        slider_bounds(&mut item.slider, mid_x, ty, art);
    }
    // Double-subtraction quirk: the caller passed `H - (bw + 1)` already.
    let auto_h = bar_height.saturating_sub(bbw.saturating_add(1));
    let bh = if item.background.overrides_height {
        item.background.height
    } else {
        auto_h
    };
    if item.has_graph() {
        // D11: the item background height of *this* pass (C: the previous one).
        let gh = if item.background.enabled {
            to_u32(bh as f64 - item.background.border_width as f64 - 1.0)
        } else {
            auto_h
        };
        let g = &mut item.graph;
        g.bounds = Rect::new(
            mid_x as f32,
            (ty as f64 - gh as f64 / 2.0 + g.line_width as f64) as f32,
            g.width as f32,
            gh as f32,
        );
    }
    if item.item_type == ItemType::AppMenu {
        let mut cx = mid_x as f32;
        for r in item.app_menu.title_bounds.iter_mut() {
            r.x = cx;
            r.y = ty as f32 - r.height / 2.0;
            cx += r.width;
        }
    }
    if item.background.enabled {
        // Note: the unaligned `x`, spanning the whole slot.
        bg_bounds(&mut item.background, x, ty, length, bh, art);
    }
    length
}

/// `text_calculate_bounds(text, x, y)` (`components.md` §4.8): aligned origin (const width
/// only), baseline `(u32)(y - (ascent - descent)/2)`, text background at the unaligned `x`.
pub fn text_calculate_bounds(text: &mut Text, x: u32, y: u32) {
    text_bounds(text, x, y, None)
}

fn text_bounds(text: &mut Text, x: u32, y: u32, art: Option<Size>) {
    let natural = text.length(true) as i32;
    let cw = text.custom_width as i32;
    text.bounds.x = if text.align == b'c' && text.has_const_width {
        (x as i32).wrapping_add(cw.wrapping_sub(natural) / 2) as f32
    } else if text.align == b'r' && text.has_const_width {
        (x as i32).wrapping_add(cw).wrapping_sub(natural) as f32
    } else {
        x as f32
    };
    text.bounds.y = to_u32(y as f64 - (text.ascent as f64 - text.descent as f64) / 2.0) as f32;
    if text.background.enabled {
        let h = if text.background.overrides_height {
            text.background.height
        } else {
            to_u32(text.bounds.height as f64)
        };
        let len = text.length(false);
        bg_bounds(&mut text.background, x, y, len, h, art);
    }
}

/// `background_calculate_bounds(bg, x, y, w, h)` (`components.md` §5.3): `y - h/2` with
/// integer division (D10: clamp at 0 instead of the u32 wrap), then the image at `(x, y)`.
/// Writes `bg.height = h` back like C's shared field.
pub fn background_calculate_bounds(bg: &mut Background, x: u32, y: u32, w: u32, h: u32) {
    bg_bounds(bg, x, y, w, h, None)
}

fn bg_bounds(bg: &mut Background, x: u32, y: u32, w: u32, h: u32, art: Option<Size>) {
    bg.bounds = Rect::new(x as f32, y.saturating_sub(h / 2) as f32, w as f32, h as f32);
    bg.height = h;
    if bg.image.enabled {
        image_bounds(&mut bg.image, x as f32, y as f32, art);
    }
}

/// `image_calculate_bounds(image, x, y)` (`components.md` §6.4) incl. the media-artwork
/// 32 pt normalisation (`artwork` = size of the shared artwork when `image.link`).
pub fn image_calculate_bounds(
    image: &mut Image,
    x: f32,
    y: f32,
    artwork: Option<crate::geometry::Size>,
) {
    image_bounds(image, x, y, artwork)
}

fn image_bounds(image: &mut Image, x: f32, y: f32, artwork: Option<Size>) {
    if image.link {
        if let Some(a) = artwork.filter(|a| a.height > 0.0) {
            let k = 32.0 / a.height as f64;
            image.size = Size::new((a.width as f64 * k) as f32, 32.0);
            image.bounds.width = image.size.width * image.scale;
            image.bounds.height = image.size.height * image.scale;
        }
    }
    image.bounds.x = x + image.padding_left as f32;
    image.bounds.y = y - image.bounds.height / 2.0 + image.y_offset as f32;
}

/// `slider_calculate_bounds(x, y)` (`components.md` §7.3).
fn slider_bounds(s: &mut Slider, x: u32, y: u32, art: Option<Size>) {
    let w = s.width;
    let h = s.track.height;
    bg_bounds(&mut s.track, x, y, w, h, art);
    let fw = to_u32(w as f64 * s.percentage as f64 / 100.0);
    bg_bounds(&mut s.fill, x, y, fw, h, art);
    let kw = s.knob.bounds.width as f64;
    let raw = (s.percentage as f64 / 100.0 * w as f64 - kw / 2.0) as i32;
    let off = (raw as f64).min(w as f64 - (kw + 1.0)).max(0.0);
    text_bounds(&mut s.knob, x.wrapping_add(to_u32(off)), y, art);
}

// ---------------------------------------------------------------------------------------
// Brackets
// ---------------------------------------------------------------------------------------

/// `group_calculate_bounds` (`item.md` §5.2, `bar.md` §4.6): bracket frame from the first /
/// last drawn member windows; bracket background at `max(L,0)`, height = its current
/// `background.height` (0 unless set). Returns the bracket window frame (nirvana origin when
/// no member is drawn).
pub fn bracket_bounds(model: &mut Model, bracket: ItemId, bar: &BarState, y: u32) -> Rect {
    let art = artwork_size(model);
    bracket_bounds_impl(model, bracket, bar, y, art)
}

fn bracket_bounds_impl(
    model: &mut Model,
    bracket: ItemId,
    bar: &BarState,
    y: u32,
    art: Option<Size>,
) -> Rect {
    let Some(bi) = model.index_of(bracket) else {
        return nirvana_rect(Size::default());
    };
    let adid = bar.adid;
    let mut first: Option<(Rect, i32)> = None;
    let mut last: Option<(Rect, i32)> = None;
    for m in &model.items[bi].bracket_members {
        let Some(mi) = model.item(*m) else { continue };
        if !model.draws_item(bar, mi) {
            continue;
        }
        // A member without a window yet gets one at nirvana (C creates it lazily).
        let f = mi
            .frame(adid)
            .unwrap_or_else(|| nirvana_rect(Size::new(1.0, 1.0)));
        if first.map_or(true, |(r, _)| f.x < r.x) {
            first = Some((f, mi.background.padding_left));
        }
        if last.map_or(true, |(r, _)| f.x + f.width > r.x + r.width) {
            last = Some((f, mi.background.padding_right));
        }
    }
    let item = &mut model.items[bi];
    let (Some((ff, first_pl)), Some((lf, last_pr))) = (first, last) else {
        // `group.bounds.origin = g_nirvana`, size unchanged (approximated by this bar's
        // last bracket window size).
        let size = item.frame(adid).map_or(Size::default(), |r| r.size());
        return nirvana_rect(size);
    };
    let len = to_u32(
        (lf.x as f64 + lf.width as f64 + last_pr as f64 + first_pl as f64 - ff.x as f64).max(0.0),
    );
    let (sl, sr) = item.shadow_extents();
    let bounds = Rect::new(
        (ff.x as f64 - first_pl as f64) as f32,
        ff.y,
        (len as f64 + sl as f64 + sr as f64) as f32,
        ff.height,
    );
    let by = add_signed(y, item.y_offset);
    let h = item.background.height;
    bg_bounds(&mut item.background, sl.max(0) as u32, by, len, h, art);
    bounds
}

// ---------------------------------------------------------------------------------------
// Popups
// ---------------------------------------------------------------------------------------

/// `bar_calculate_popup_anchor_for_bar_item` + `popup_calculate_popup_anchor_for_bar_item`
/// (`item.md` §6.3, `bar.md` §4.7). D4: nested popups are anchored before their bounds are
/// computed (no one-refresh lag).
pub fn anchor_popup(model: &mut Model, host: ItemId, bar: &BarState, res: &mut dyn Resources) {
    anchor_bar_popup(model, host, bar, res)
}

/// Horizontal alignment of an anchor (`align` `c` centred, `l` left minus the host's
/// `padding_left`, anything else right-aligned).
fn align_anchor(a: f32, win_len: f32, popup_len: f32, align: u8, host_pl: i32) -> f32 {
    match align {
        b'c' => a + (win_len - popup_len) / 2.0,
        b'l' => a - host_pl as f32,
        _ => a + win_len - popup_len,
    }
}

fn anchor_bar_popup(model: &mut Model, host: ItemId, bar: &BarState, res: &mut dyn Resources) {
    if bar.adid != model.active_adid {
        return;
    }
    let Some(hi) = model.index_of(host) else {
        return;
    };
    let Some(win) = model.items[hi].frame(bar.adid) else {
        return;
    };
    let vertical = model.bar.is_vertical();
    {
        let p = &mut model.items[hi].popup;
        if !p.overrides_cell_size {
            p.cell_size = to_u32(if vertical { win.width } else { win.height } as f64);
        }
    }
    popup_bounds_impl(model, host, res, 0); // pass 1: the size
    let item = &model.items[hi];
    let pb = item.popup.background.bounds.size();
    let mut a = win.origin();
    let (align, pl) = (item.popup.align, item.background.padding_left);
    if !vertical {
        a.x = align_anchor(a.x, win.width, pb.width, align, pl);
        a.y += if model.bar.position == b'b' {
            -pb.height
        } else {
            win.height
        };
    } else {
        a.y = align_anchor(a.y, win.height, pb.height, align, pl);
        a.x += if model.bar.position == b'r' {
            -pb.width
        } else {
            win.width
        };
    }
    set_anchor(model, hi, a, bar.adid);
    popup_bounds_impl(model, host, res, 0); // pass 2: the frames
}

/// `popup_calculate_popup_anchor_for_bar_item` (`item.md` §6.3, nested popups) with D4:
/// the sub-popup is laid out again after its anchor is set.
fn anchor_nested_popup(
    model: &mut Model,
    item_id: ItemId,
    parent_host: ItemId,
    res: &mut dyn Resources,
    depth: u32,
) {
    let Some(ii) = model.index_of(item_id) else {
        return;
    };
    let Some(parent) = model.item(parent_host) else {
        return;
    };
    let (pp_adid, pp_horizontal) = (parent.popup.adid, parent.popup.horizontal);
    if pp_adid == 0 || pp_adid != model.active_adid {
        return;
    }
    let Some(win) = model.items[ii].frame(pp_adid) else {
        return;
    };
    {
        let p = &mut model.items[ii].popup;
        if !p.overrides_cell_size {
            p.cell_size = to_u32(win.height as f64);
        }
    }
    popup_bounds_impl(model, item_id, res, depth);
    let item = &model.items[ii];
    let pb = item.popup.background.bounds.size();
    let mut a = win.origin();
    if item.position != Position::Popup || pp_horizontal {
        a.x = align_anchor(
            a.x,
            win.width,
            pb.width,
            item.popup.align,
            item.background.padding_left,
        );
        a.y += if model.bar.position == b'b' {
            -pb.height
        } else {
            win.height
        };
    } else if let Some(hp) = item.parent.and_then(|p| model.item(p)).map(|p| &p.popup) {
        // Flyout to the left or right of the parent popup window.
        let hf = hp
            .frame
            .unwrap_or_else(|| Rect::new(hp.anchor.x, hp.anchor.y, 0.0, 0.0));
        a.x = hf.x
            + if item.popup.align == b'l' {
                -pb.width
            } else {
                hf.width
            };
        a.y -= hp.background.border_width as f32;
    }
    set_anchor(model, ii, a, pp_adid);
    popup_bounds_impl(model, item_id, res, depth); // D4
}

/// `popup_set_anchor`: `anchor = a + (0, y_offset)`; a changed `adid` requires ordering and
/// redraws all members.
fn set_anchor(model: &mut Model, hi: usize, a: Point, adid: u32) {
    let p = &mut model.items[hi].popup;
    p.anchor = Point::new(a.x, a.y + p.y_offset as f32);
    if p.adid != adid {
        p.adid = adid;
        p.needs_ordering = true;
        let members = p.items.clone();
        for m in members {
            if let Some(mi) = model.item_mut(m) {
                mi.needs_update = true;
            }
        }
    }
}

/// `popup_calculate_bounds` (`item.md` §6.4): stacks/rows member items (cell =
/// `max(item height, cell_size)`), border quirk (width +bw once, height +2·bw), background
/// image sizing, bracket members (§5.3). Returns the popup layout when anchored.
pub fn popup_bounds(
    model: &mut Model,
    host: ItemId,
    res: &mut dyn Resources,
) -> Option<PopupLayout> {
    popup_bounds_impl(model, host, res, 0)
}

fn popup_bounds_impl(
    model: &mut Model,
    host: ItemId,
    res: &mut dyn Resources,
    depth: u32,
) -> Option<PopupLayout> {
    if depth > MAX_POPUP_DEPTH {
        return None;
    }
    let hi = model.index_of(host)?;
    let art = artwork_size(model);
    let bbw = model.bar.background.border_width;
    let (bw, horizontal, cell_size, adid, anchor, has_img, img, members) = {
        let p = &model.items[hi].popup;
        let bg = &p.background;
        (
            bg.border_width,
            p.horizontal,
            p.cell_size,
            p.adid,
            p.anchor,
            bg.enabled && bg.image.enabled,
            bg.image.reserved_size(),
            p.items.clone(),
        )
    };
    let laid: Vec<usize> = members
        .iter()
        .filter_map(|m| model.index_of(*m))
        .filter(|&i| model.items[i].drawing && !model.items[i].is_bracket())
        .collect();

    let mut y: u32 = bw;
    let mut x: u32 = 0;
    let mut width: u32 = 0;
    let mut row_h: u32 = 0;
    if has_img {
        width = to_u32(img.width as f64 + 2.0 * bw as f64);
    }
    if horizontal {
        let mut total: i64 = 0;
        for &i in &laid {
            let it = &model.items[i];
            total += it.background.padding_left as i64
                + it.background.padding_right as i64
                + item_len(it, false) as i64;
            row_h = row_h.max(layout_height(it).max(cell_size));
        }
        if has_img {
            row_h = row_h.max(to_u32(img.height as f64));
            // D10: `(width - total)/2` clamps instead of wrapping.
            x = ((width as i64 - total).max(0) / 2) as u32;
        }
    }
    for &i in &laid {
        let it = &mut model.items[i];
        let cell = layout_height(it).max(cell_size);
        let pl = it.background.padding_left;
        let pr = it.background.padding_right;
        let ix = (x as i32).wrapping_add(pl).max(0);
        let ih = if horizontal { row_h } else { cell };
        // x = 0: shadow extents are ignored in popups.
        let slot = calc_bounds(it, ih, 0, ih / 2, bbw, art);
        let iw = (pl as i64 + pr as i64 + slot as i64).clamp(0, u32::MAX as i64) as u32;
        if adid > 0 {
            let f = Rect::new(
                anchor.x + ix as f32,
                anchor.y + y as f32,
                item_len(it, true) as f32,
                ih as f32,
            );
            it.set_frame(adid, f);
        }
        if horizontal {
            x = x.wrapping_add(iw);
        } else {
            width = width.max(iw);
            y = y.wrapping_add(cell);
        }
    }

    // Bracket members (§5.3), only while anchored.
    if adid > 0 {
        if let Some(bar) = model.bar(adid).cloned() {
            for m in &members {
                let Some(bi) = model.index_of(*m) else {
                    continue;
                };
                let b = &model.items[bi];
                if !b.drawing || !b.is_bracket() {
                    continue;
                }
                let mut cell = cell_size;
                if b.bracket_members.len() > 1 {
                    if let Some(m1) = model.item(b.bracket_members[0]) {
                        cell = m1.height().max(cell_size);
                    }
                }
                let ih = if horizontal { row_h } else { cell };
                let r = bracket_bounds_impl(model, *m, &bar, ih / 2, art);
                model.items[bi].set_frame(adid, r);
            }
        }
    }

    if horizontal {
        if !has_img {
            width = x.wrapping_add(bw);
        }
        y = y.wrapping_add(row_h);
    } else if !has_img {
        width = width.wrapping_add(bw);
    }
    y = y.wrapping_add(bw);
    {
        let p = &mut model.items[hi].popup;
        p.background.bounds = Rect::new(0.0, 0.0, width as f32, y as f32);
        p.background.height = y;
        let ih = p.background.image.bounds.height;
        image_bounds(
            &mut p.background.image,
            bw as f32,
            bw as f32 + ih / 2.0,
            art,
        );
        if adid > 0 {
            p.frame = Some(Rect::new(anchor.x, anchor.y, width as f32, y as f32));
        }
    }

    // D4: nested popups are anchored once this popup's own frame is final (C anchors them
    // inside the member loop, against the previous frames).
    if adid > 0 && adid == model.active_adid {
        for &i in &laid {
            let (id, drawing) = (model.items[i].id, model.items[i].popup.drawing);
            if drawing {
                anchor_nested_popup(model, id, host, res, depth + 1);
            }
        }
    }
    (adid > 0).then(|| popup_snapshot(model, host, res))
}

/// The current state of `host`'s popup as a [`PopupLayout`] (no recomputation).
fn popup_snapshot(model: &mut Model, host: ItemId, res: &mut dyn Resources) -> PopupLayout {
    let (adid, frame, level, members) = {
        let p = &model.item(host).expect("popup host").popup;
        let frame = p.frame.unwrap_or_else(|| {
            Rect::new(
                p.anchor.x,
                p.anchor.y,
                p.background.bounds.width,
                p.background.bounds.height,
            )
        });
        let level = if p.topmost {
            level::POPUP_MENU
        } else {
            level::BACKSTOP_MENU + 1
        };
        (p.adid, frame, level, p.items.clone())
    };
    let bar = model.bar(adid).cloned();
    let mut out = PopupLayout {
        host,
        adid,
        frame,
        level,
        items: Vec::new(),
        menu_lines: Vec::new(),
    };
    for m in members {
        let Some(mi) = model.index_of(m) else {
            continue;
        };
        let it = &model.items[mi];
        let drawn = match &bar {
            Some(b) => model.draws_item(b, it),
            None => it.drawing,
        };
        if !drawn {
            continue;
        }
        let Some(f) = it.frame(adid) else { continue };
        out.items.push(PlacedItem { id: m, frame: f });
        if it.item_type == ItemType::AppMenu {
            let lines = prepare_app_menu_lines(&mut model.items[mi], res);
            out.menu_lines.push((m, lines));
        }
    }
    out
}

/// Popups shown after the pass (`popup_draw` preconditions: host drawn on the active bar,
/// `popup.drawing`, `adid ≥ 1`, ≥ 1 item), parents before nested popups. Every other
/// popup loses its window frame (D3).
fn collect_popups(model: &mut Model, res: &mut dyn Resources) -> Vec<PopupLayout> {
    let active = model
        .bar(model.active_adid)
        .filter(|b| b.sid >= 1 && b.adid >= 1)
        .cloned();
    let mut shown: Vec<(u32, ItemId)> = Vec::new();
    if let Some(bar) = &active {
        for item in &model.items {
            let p = &item.popup;
            if p.drawing && p.adid >= 1 && !p.items.is_empty() && model.draws_item(bar, item) {
                shown.push((popup_depth(model, item), item.id));
            }
        }
    }
    shown.sort_by_key(|(d, _)| *d);
    for item in model.items.iter_mut() {
        if !shown.iter().any(|(_, id)| *id == item.id) {
            item.popup.frame = None;
        }
    }
    shown
        .into_iter()
        .map(|(_, id)| popup_snapshot(model, id, res))
        .collect()
}

/// Nesting depth of an item's popup (number of popup parents above it).
fn popup_depth(model: &Model, item: &BarItem) -> u32 {
    let mut d = 0;
    let mut cur = item;
    while cur.position == Position::Popup && d < MAX_POPUP_DEPTH {
        let Some(p) = cur.parent.and_then(|p| model.item(p)) else {
            break;
        };
        d += 1;
        cur = p;
    }
    d
}

// ---------------------------------------------------------------------------------------
// Scenes
// ---------------------------------------------------------------------------------------

/// CG item-local (y up) → window-local top-left conversion.
#[derive(Clone, Copy)]
struct Conv {
    ox: f32,
    oy: f32,
    h: f32,
}

impl Conv {
    fn rect(&self, r: Rect) -> Rect {
        Rect::new(
            self.ox + r.x,
            self.oy + self.h - (r.y + r.height),
            r.width,
            r.height,
        )
    }
    fn pt(&self, x: f32, y: f32) -> Point {
        Point::new(self.ox + x, self.oy + self.h - y)
    }
}

/// `draw_rect` radius clamp (`components.md` §5.4): against the rect inset by `lw/2`,
/// truncated to an integer.
fn clamp_radius(r: &Rect, radius: u32, lw: f32) -> f32 {
    let iw = r.width - lw;
    let ih = r.height - lw;
    let rad = radius as f32;
    if rad > ih / 2.0 || rad > iw / 2.0 {
        to_u32(if ih > iw { iw / 2.0 } else { ih / 2.0 } as f64) as f32
    } else {
        rad
    }
}

/// `background_draw` (`components.md` §5.5).
fn draw_background(bg: &Background, cv: &Conv, art: Option<ImageInfo>, scene: &mut Scene) {
    if !bg.enabled {
        return;
    }
    if (bg.border_color.a == 0.0 || bg.border_width == 0)
        && bg.color.a == 0.0
        && !bg.shadow.enabled
        && !bg.image.enabled
    {
        return;
    }
    let r = bg.bounds.offset(bg.x_offset as f32, bg.y_offset as f32);
    let lw = bg.border_width as f32;
    if bg.shadow.enabled {
        // Solid silhouette (fill and stroke in the shadow colour), even for alpha-0 fills.
        let sr = r.offset(bg.shadow.offset.x, bg.shadow.offset.y);
        scene.push(Primitive::Shadow {
            rect: cv.rect(sr),
            color: bg.shadow.color,
            corner_radius: clamp_radius(&sr, bg.corner_radius, lw),
            border_width: lw,
        });
    }
    scene.push(Primitive::Rect {
        rect: cv.rect(r),
        color: bg.color,
        corner_radius: clamp_radius(&r, bg.corner_radius, lw),
        border_width: lw,
        border_color: bg.border_color,
    });
    if bg.image.enabled {
        // The image ignores the background's offsets.
        draw_image(&bg.image, cv, art, scene);
    }
}

/// `image_draw` (`components.md` §6.5).
fn draw_image(image: &Image, cv: &Conv, art: Option<ImageInfo>, scene: &mut Scene) {
    let key = if image.link {
        art.map(|a| a.key)
    } else {
        image.key
    };
    let Some(key) = key else { return };
    let r = image.bounds;
    let cr = image.corner_radius as f32;
    if image.shadow.enabled {
        let sr = r.offset(image.shadow.offset.x, image.shadow.offset.y);
        // Q2: the unclamped rounded-rect path is skipped when the radius does not fit.
        if !(2.0 * cr > sr.width || 2.0 * cr > sr.height) {
            scene.push(Primitive::Shadow {
                rect: cv.rect(sr),
                color: image.shadow.color,
                corner_radius: cr,
                border_width: 0.0,
            });
        }
    }
    let rounded = r.height > 2.0 * cr && r.width > 2.0 * cr;
    scene.push(Primitive::Image {
        rect: cv.rect(r),
        key,
        corner_radius: cr,
        rounded,
        border_width: image.border_width,
        border_color: image.border_color,
    });
}

/// `text_draw` (`components.md` §4.9).
fn draw_text(text: &Text, cv: &Conv, art: Option<ImageInfo>, scene: &mut Scene) {
    if !text.drawing {
        return;
    }
    if text.background.enabled {
        draw_background(&text.background, cv, art, scene);
    }
    let Some(key) = text.line else { return };
    let b = text.bounds;
    let pl = text.padding_left as f32;
    let yo = text.y_offset as f32;
    // Horizontal-only clip for max_chars truncation.
    let clip = (text.max_chars > 0)
        .then(|| cv.rect(Rect::new(b.x + pl, -9999.0, text.width as f32, 19998.0)));
    if text.shadow.enabled {
        // Quirk: the shadow ignores `scroll`.
        scene.push(Primitive::Text {
            origin: cv.pt(
                b.x + text.shadow.offset.x + pl,
                b.y + text.shadow.offset.y + yo,
            ),
            key,
            color: text.shadow.color,
            clip,
        });
    }
    scene.push(Primitive::Text {
        origin: cv.pt(b.x + pl - text.scroll, b.y + yo),
        key,
        color: if text.highlight {
            text.highlight_color
        } else {
            text.color
        },
        clip,
    });
}

/// `graph_draw` (`components.md` §8.5): exact path, u32 x arithmetic.
fn draw_graph(item: &BarItem, cv: &Conv, scene: &mut Scene) {
    let g = &item.graph;
    let n = g.samples.len().min(g.width as usize);
    if n == 0 {
        return;
    }
    let mut x = to_u32(g.bounds.x as f64 + if g.rtl { g.width as f64 } else { 0.0 });
    let y = to_u32(g.bounds.y as f64);
    let h = to_u32(g.bounds.height as f64);
    let start_x = x;
    let pt = |x: u32, v: f32| cv.pt(x as f32, y as f32 + v * h as f32);
    let mut line = Vec::with_capacity(n + 1);
    if g.rtl {
        line.push(pt(x, g.sample(n - 1)));
        for i in (1..n).rev() {
            line.push(pt(x, g.sample(i)));
            x = x.wrapping_sub(1);
        }
    } else {
        line.push(pt(x, g.sample(0)));
        for i in (1..n).rev() {
            line.push(pt(x, g.sample(i)));
            x = x.wrapping_add(1);
        }
    }
    let mut fill = line.clone();
    let close_x = if g.rtl {
        x.wrapping_add(1)
    } else {
        x.wrapping_sub(1)
    };
    fill.push(cv.pt(close_x as f32, y as f32));
    fill.push(cv.pt(start_x as f32, y as f32));
    scene.push(Primitive::Graph {
        line,
        fill,
        line_color: g.line_color,
        fill_color: g.effective_fill(),
        line_width: g.line_width,
    });
}

/// `app_menu` titles (extension): highlight of the hovered title, then each title.
fn draw_app_menu(item: &BarItem, cv: &Conv, lines: Option<&[MenuLine]>, scene: &mut Scene) {
    let am = &item.app_menu;
    let entries = am.visible_titles();
    let half = am.spacing.max(0) as f32 / 2.0;
    for (k, r) in am.title_bounds.iter().enumerate() {
        let idx = entries.get(k).map(|e| e.0);
        if idx.is_some() && am.hovered == idx {
            scene.push(Primitive::Rect {
                rect: cv.rect(*r),
                color: am.highlight_color,
                corner_radius: clamp_radius(r, am.corner_radius, 0.0),
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
            });
        }
        if let Some(line) = lines.and_then(|l| l.get(k)) {
            scene.push(Primitive::Text {
                origin: cv.pt(r.x + half, r.y + APP_MENU_PAD + line.descent),
                key: line.key,
                color: am.color,
                clip: None,
            });
        }
    }
}

/// Scene of one bar window: the bar background (`bar_draw`: forced enabled, no shadow,
/// `y_offset` cancelled), clip holes of clipping item/icon/label backgrounds
/// (`components.md` §5.6), then every placed item in order via [`item_scene`].
pub fn bar_scene(model: &Model, layout: &BarLayout) -> Scene {
    let size = layout.frame.size();
    let mut scene = Scene::new(size);
    let cv = Conv {
        ox: 0.0,
        oy: 0.0,
        h: size.height,
    };
    let art = model.current_artwork;

    let mut bg = model.bar.background.clone();
    // `bg.bounds = window frame; origin.y -= y_offset` cancels the +y_offset of drawing.
    bg.bounds = Rect::new(0.0, -(bg.y_offset as f32), size.width, size.height);
    bg.shadow.enabled = false;
    bg.enabled = true;
    draw_background(&bg, &cv, art, &mut scene);

    // Clip holes (`bar_item_clip_bar`) for every item drawn on this bar, popup members
    // included (C does the same, with their x relative to the bar window). Q11: the hole
    // outline is stroked with the bar's border width / border alpha.
    let stroke_width = model.bar.background.border_width as f32;
    let stroke_alpha = model.bar.background.border_color.a;
    for p in &layout.items {
        let Some(item) = model.item(p.id) else {
            continue;
        };
        let dx = p.frame.x - layout.frame.x;
        for b in [
            &item.background,
            &item.icon.background,
            &item.label.background,
        ] {
            if !b.clips_bar() {
                continue;
            }
            let r = b.bounds.offset(dx + b.x_offset as f32, b.y_offset as f32);
            scene.push(Primitive::ClipHole {
                rect: cv.rect(r),
                corner_radius: clamp_radius(&r, b.corner_radius, 0.0),
                alpha: b.clip,
                stroke_width,
                stroke_alpha,
            });
        }
    }

    let placed: Vec<(&PlacedItem, &BarItem)> = layout
        .items
        .iter()
        .filter_map(|p| model.item(p.id).map(|i| (p, i)))
        .filter(|(_, i)| i.position != Position::Popup)
        .collect();
    paint_windows(
        &placed,
        layout.frame.origin(),
        &layout.menu_lines,
        art,
        &mut scene,
    );
    scene
}

/// Paints item windows in window z-order: brackets (below the first item window) in
/// global order, then the other items in order; each clipped to its window rect.
fn paint_windows(
    placed: &[(&PlacedItem, &BarItem)],
    window_origin: Point,
    menu_lines: &[(ItemId, Vec<MenuLine>)],
    art: Option<ImageInfo>,
    scene: &mut Scene,
) {
    let brackets = placed.iter().filter(|(_, i)| i.is_bracket());
    let others = placed.iter().filter(|(_, i)| !i.is_bracket());
    for (p, item) in brackets.chain(others) {
        let local = Rect::new(
            p.frame.x - window_origin.x,
            p.frame.y - window_origin.y,
            p.frame.width,
            p.frame.height,
        );
        let lines = menu_lines
            .iter()
            .find(|(id, _)| *id == item.id)
            .map(|(_, l)| l.as_slice());
        scene.push(Primitive::PushClip {
            rect: local,
            corner_radius: 0.0,
        });
        item_scene_impl(item, local, art, lines, scene);
        scene.push(Primitive::PopClip);
    }
}

/// Scene of one popup window: its background with the shadow forced off, then the member
/// items.
pub fn popup_scene(model: &Model, layout: &PopupLayout) -> Scene {
    let size = layout.frame.size();
    let mut scene = Scene::new(size);
    let cv = Conv {
        ox: 0.0,
        oy: 0.0,
        h: size.height,
    };
    let art = model.current_artwork;
    if let Some(host) = model.item(layout.host) {
        let mut bg = host.popup.background.clone();
        bg.shadow.enabled = false;
        draw_background(&bg, &cv, art, &mut scene);
    }
    let placed: Vec<(&PlacedItem, &BarItem)> = layout
        .items
        .iter()
        .filter_map(|p| model.item(p.id).map(|i| (p, i)))
        .collect();
    paint_windows(
        &placed,
        layout.frame.origin(),
        &layout.menu_lines,
        art,
        &mut scene,
    );
    scene
}

/// `bar_item_draw` (`components.md` §10.3) into `scene`, with the item's virtual window
/// placed at `origin` (window-local, top-left) and `window_h` its height: item background
/// (+ blur region if `blur_radius > 0`); brackets stop here; icon (background, shadow,
/// glyphs with `-scroll`, `max_chars` clip), label, alias (tint mask), graph (exact path
/// §8.5), slider (track, fill, knob), app_menu titles.
pub fn item_scene(
    item: &BarItem,
    origin: Point,
    window_h: f32,
    artwork: Option<crate::platform::ImageInfo>,
    scene: &mut Scene,
) {
    // Window width as laid out in a horizontal bar: display length + shadow extents.
    let (sl, sr) = item.shadow_extents();
    let w = item_len(item, true) as f32 + sl as f32 + sr as f32;
    let window = Rect::new(origin.x, origin.y, w, window_h);
    item_scene_impl(item, window, artwork, None, scene)
}

fn item_scene_impl(
    item: &BarItem,
    window: Rect,
    art: Option<ImageInfo>,
    menu_lines: Option<&[MenuLine]>,
    scene: &mut Scene,
) {
    let cv = Conv {
        ox: window.x,
        oy: window.y,
        h: window.height,
    };
    if item.blur_radius > 0 {
        // The item window's background blur (`window_set_blur_radius`) covers the whole
        // (rectangular) window.
        scene.push(Primitive::BlurRegion {
            rect: window,
            corner_radius: 0.0,
            radius: item.blur_radius,
        });
    }
    draw_background(&item.background, &cv, art, scene);
    if item.is_bracket() {
        return;
    }
    draw_text(&item.icon, &cv, art, scene);
    draw_text(&item.label, &cv, art, scene);
    if item.has_alias() {
        let a = &item.alias;
        draw_image(&a.image, &cv, None, scene);
        if a.color_override {
            if let Some(key) = a.image.key {
                scene.push(Primitive::ImageMask {
                    rect: cv.rect(a.image.bounds),
                    key,
                    color: a.color,
                });
            }
        }
    }
    if item.has_graph() {
        draw_graph(item, &cv, scene);
    }
    if item.has_slider() {
        let s = &item.slider;
        draw_background(&s.track, &cv, art, scene);
        draw_background(&s.fill, &cv, art, scene);
        draw_text(&s.knob, &cv, art, scene);
    }
    if item.item_type == ItemType::AppMenu {
        draw_app_menu(item, &cv, menu_lines, scene);
    }
}

// ---------------------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------------------

/// Window under `p` (see [`WindowHit`]); only items with `drawing=on`.
///
/// Z-order emulation (`bar.md` §3.7, `item.md` §6.5/§7.4): windows are grouped by level
/// (bar windows at `--bar topmost`'s level, popups at `popup.topmost`'s level); within a
/// bar group the bar window is at the bottom, brackets above it (below the first item
/// window), then items in global order (later above earlier); within a popup group the
/// popup window, its brackets, then its members in popup order. Between groups of equal
/// level, later groups (nested popups after their parents) are above. Window rects are
/// half-open.
pub fn window_at(model: &Model, layout: &Layout, p: Point) -> WindowHit {
    struct Group {
        level: i32,
        seq: usize,
        windows: Vec<(Rect, WindowHit)>,
    }
    fn push_items(
        model: &Model,
        items: &[PlacedItem],
        popup: bool,
        ws: &mut Vec<(Rect, WindowHit)>,
    ) {
        let resolved: Vec<(&PlacedItem, &BarItem)> = items
            .iter()
            .filter_map(|pi| model.item(pi.id).map(|i| (pi, i)))
            .filter(|(_, i)| i.drawing && (popup || i.position != Position::Popup))
            .collect();
        for brackets in [true, false] {
            for (pi, i) in &resolved {
                if i.is_bracket() == brackets {
                    ws.push((pi.frame, WindowHit::Item(i.id)));
                }
            }
        }
    }
    let mut groups = Vec::new();
    for (s, bl) in layout.bars.iter().enumerate() {
        let mut windows = vec![(bl.frame, WindowHit::Bar(bl.adid))];
        push_items(model, &bl.items, false, &mut windows);
        groups.push(Group {
            level: model.bar.window_level,
            seq: s,
            windows,
        });
    }
    for (s, pl) in layout.popups.iter().enumerate() {
        let mut windows = vec![(pl.frame, WindowHit::Popup(pl.host))];
        push_items(model, &pl.items, true, &mut windows);
        groups.push(Group {
            level: pl.level,
            seq: layout.bars.len() + s,
            windows,
        });
    }
    groups.sort_by_key(|g| (g.level, g.seq));
    for g in groups.iter().rev() {
        for (r, hit) in g.windows.iter().rev() {
            if contains_half_open(r, p) {
                return *hit;
            }
        }
    }
    WindowHit::None
}

/// `get_item_by_point`: first item in **global order** with `drawing=on` whose virtual
/// window contains `p` (inclusive edges).
pub fn item_at_point(model: &Model, p: Point) -> Option<ItemId> {
    model
        .items
        .iter()
        .find(|i| i.drawing && i.contains_point(p))
        .map(|i| i.id)
}

/// `get_bar_by_point` (half-open `CGRectContainsPoint`). Hidden / not-shown bars have their
/// window parked at nirvana and never match.
pub fn bar_at_point(model: &Model, p: Point) -> Option<u32> {
    model
        .bars
        .iter()
        .find(|b| !b.hidden && b.shown && contains_half_open(&b.frame, p))
        .map(|b| b.adid)
}

/// `get_popup_by_point`: popups of items with `drawing && popup.drawing` (half-open).
pub fn popup_at_point(model: &Model, p: Point) -> Option<ItemId> {
    model
        .items
        .iter()
        .find(|i| {
            i.drawing && i.popup.drawing && i.popup.frame.is_some_and(|f| contains_half_open(&f, p))
        })
        .map(|i| i.id)
}

/// Converts a screen point to the item-local drawing space of `item` on `adid` (y up),
/// consistently (D2), for slider hit tests / drags and app_menu titles. `None` if the item
/// has no frame there.
pub fn item_local_point(item: &BarItem, adid: u32, p: Point) -> Option<Point> {
    let f = item.frame(adid)?;
    Some(Point::new(p.x - f.x, f.height - (p.y - f.y)))
}

/// Slider track hit test in item-local space (`CGRectContainsPoint(track.bounds, p)`).
pub fn slider_track_contains(item: &BarItem, local: Point) -> bool {
    contains_half_open(&item.slider.track.bounds, local)
}

/// Index into `app_menu.titles` of the title under `local` (extension).
pub fn app_menu_title_at(item: &BarItem, local: Point) -> Option<usize> {
    if item.item_type != ItemType::AppMenu {
        return None;
    }
    let entries = item.app_menu.visible_titles();
    item.app_menu
        .title_bounds
        .iter()
        .position(|r| contains_half_open(r, local))
        .and_then(|k| entries.get(k).map(|e| e.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_clamp_d10() {
        assert_eq!(to_u32(-3.5), 0);
        assert_eq!(to_u32(f64::NAN), 0);
        assert_eq!(to_u32(7.9), 7);
        assert_eq!(add_signed(5, -9), 0);
        assert_eq!(add_signed(5, 4), 9);
    }

    #[test]
    fn cursor_rules() {
        // LTR: max(cur + pl, 0).
        assert_eq!(place_cursor(20, 10, 5, 0, false, 100.0), 25);
        assert_eq!(place_cursor(20, 10, -50, 0, false, 100.0), 0);
        // RTL: candidate, wrap → W - len, negative W - len → 0.
        assert_eq!(place_cursor(80, 10, 0, 5, true, 100.0), 65);
        assert_eq!(place_cursor(5, 10, 0, 0, true, 100.0), 90);
        assert_eq!(place_cursor(5, 120, 0, 0, true, 100.0), 0);
        // Negative padding_right: (u32)pr wraps the subtraction back into range, clamp.
        assert_eq!(place_cursor(95, 10, 0, -20, true, 100.0), 90);
    }

    #[test]
    fn half_open_and_radius() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(contains_half_open(&r, Point::new(0.0, 0.0)));
        assert!(!contains_half_open(&r, Point::new(10.0, 5.0)));
        assert!(!contains_half_open(&r, Point::new(5.0, 10.0)));
        // radius clamped against the inset rect, truncated
        assert_eq!(clamp_radius(&Rect::new(0.0, 0.0, 18.0, 23.0), 50, 2.0), 8.0);
        assert_eq!(clamp_radius(&Rect::new(0.0, 0.0, 9.0, 23.0), 50, 0.0), 4.0);
        assert_eq!(clamp_radius(&Rect::new(0.0, 0.0, 18.0, 23.0), 3, 2.0), 3.0);
        assert_eq!(clamp_radius(&Rect::new(0.0, 0.0, 1.0, 1.0), 3, 4.0), 0.0);
    }

    #[test]
    fn conv_flips_y() {
        let cv = Conv {
            ox: 10.0,
            oy: 5.0,
            h: 25.0,
        };
        assert_eq!(
            cv.rect(Rect::new(1.0, 2.0, 3.0, 4.0)),
            Rect::new(11.0, 5.0 + 25.0 - 6.0, 3.0, 4.0)
        );
        assert_eq!(cv.pt(1.0, 7.0), Point::new(11.0, 23.0));
    }
}
