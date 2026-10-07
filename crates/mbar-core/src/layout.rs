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

use crate::bar::{BarProps, BarState};
use crate::components::{Background, Image, Text};
use crate::geometry::{Point, Rect};
use crate::item::{BarItem, ItemId};
use crate::model::Model;
use crate::platform::{DisplayInfo, Resources};
use crate::scene::Scene;

/// `g_nirvana`: where invisible windows are parked.
pub const NIRVANA: Point = Point {
    x: -9999.0,
    y: -9999.0,
};

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
    /// these and clears it for all others.
    pub items: Vec<PlacedItem>,
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
    pub items: Vec<PlacedItem>,
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

/// `bar_get_frame` (`bar.md` §4.1): horizontal (`t`/`b`/other) and vertical (`l`/`r`) bar
/// window frames incl. margin, `y_offset` (bottom uses `2*y_offset`), notch offset/height on
/// built-in displays, and the menu-bar offset (`menu_bar_visible && !topmost`).
pub fn bar_frame(bar: &BarProps, display: &DisplayInfo, menu_bar_visible: bool) -> Rect {
    let _ = (bar, display, menu_bar_visible);
    todo!("WP-A: bar.md §4.1")
}

/// Full layout pass (`bar_calculate_bounds` for every bar + popups), writing item frames
/// (`set_frame`), component bounds, `graph.rtl`, popup `cell_size`/`anchor`/`adid`/`frame`,
/// and the auto background heights. Calls `BarItem::ensure_layout` on every item first.
/// Bars with `sid < 1 || adid < 1` are skipped (never drawn). Hidden / not-shown bars park
/// their frame at nirvana.
pub fn layout(model: &mut Model, res: &mut dyn Resources) -> Layout {
    let _ = (model, res);
    todo!("WP-A: bar.md §4, item.md §4–6")
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
    let _ = (model, bar_index, res);
    todo!("WP-A: bar.md §4.4")
}

/// `bar_calculate_bounds_left_right` (`bar.md` §4.5, `item.md` §4.6): vertical bars; centre
/// start `(H - 2*margin - len)/2 - 1`; no bracket pass; window width = bar thickness.
pub fn layout_bar_vertical(
    model: &mut Model,
    bar_index: usize,
    res: &mut dyn Resources,
) -> BarLayout {
    let _ = (model, bar_index, res);
    todo!("WP-A: bar.md §4.5")
}

/// `bar_manager_length_for_bar_side` (`bar.md` §4.3): sum over items with the given
/// position, not brackets, drawn on `bar`, of `len + (const ? 0 : pl + pr)` (`len` = height
/// for vertical bars).
pub fn side_length(
    model: &Model,
    bar: &BarState,
    side: crate::item::Position,
    vertical: bool,
) -> u32 {
    let _ = (model, bar, side, vertical);
    todo!("WP-A: bar.md §4.3")
}

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
    let _ = (item, bar_height, x, y, bar_border_width);
    todo!("WP-A: item.md §4.3")
}

/// `text_calculate_bounds(text, x, y)` (`components.md` §4.8): aligned origin (const width
/// only), baseline `(u32)(y - (ascent - descent)/2)`, text background at the unaligned `x`.
pub fn text_calculate_bounds(text: &mut Text, x: u32, y: u32) {
    let _ = (text, x, y);
    todo!("WP-A: components.md §4.8")
}

/// `background_calculate_bounds(bg, x, y, w, h)` (`components.md` §5.3): `y - h/2` with
/// integer division (D10: clamp at 0 instead of the u32 wrap), then the image at `(x, y)`.
/// Writes `bg.height = h` back like C's shared field.
pub fn background_calculate_bounds(bg: &mut Background, x: u32, y: u32, w: u32, h: u32) {
    let _ = (bg, x, y, w, h);
    todo!("WP-A: components.md §5.3")
}

/// `image_calculate_bounds(image, x, y)` (`components.md` §6.4) incl. the media-artwork
/// 32 pt normalisation (`artwork` = size of the shared artwork when `image.link`).
pub fn image_calculate_bounds(
    image: &mut Image,
    x: f32,
    y: f32,
    artwork: Option<crate::geometry::Size>,
) {
    let _ = (image, x, y, artwork);
    todo!("WP-A: components.md §6.4")
}

/// `group_calculate_bounds` (`item.md` §5.2, `bar.md` §4.6): bracket frame from the first /
/// last drawn member windows; bracket background at `max(L,0)`, height = its current
/// `background.height` (0 unless set). Returns the bracket window frame (nirvana origin when
/// no member is drawn).
pub fn bracket_bounds(model: &mut Model, bracket: ItemId, bar: &BarState, y: u32) -> Rect {
    let _ = (model, bracket, bar, y);
    todo!("WP-A: item.md §5.2")
}

/// `bar_calculate_popup_anchor_for_bar_item` + `popup_calculate_popup_anchor_for_bar_item`
/// (`item.md` §6.3, `bar.md` §4.7). D4: nested popups are anchored before their bounds are
/// computed (no one-refresh lag).
pub fn anchor_popup(model: &mut Model, host: ItemId, bar: &BarState, res: &mut dyn Resources) {
    let _ = (model, host, bar, res);
    todo!("WP-A: item.md §6.3")
}

/// `popup_calculate_bounds` (`item.md` §6.4): stacks/rows member items (cell =
/// `max(item height, cell_size)`), border quirk (width +bw once, height +2·bw), background
/// image sizing, bracket members (§5.3). Returns the popup layout when anchored.
pub fn popup_bounds(
    model: &mut Model,
    host: ItemId,
    res: &mut dyn Resources,
) -> Option<PopupLayout> {
    let _ = (model, host, res);
    todo!("WP-A: item.md §6.4")
}

/// Scene of one bar window: the bar background (`bar_draw`: forced enabled, no shadow,
/// `y_offset` cancelled), clip holes of clipping item/icon/label backgrounds
/// (`components.md` §5.6), then every placed item in order via [`item_scene`].
pub fn bar_scene(model: &Model, layout: &BarLayout) -> Scene {
    let _ = (model, layout);
    todo!("WP-A: bar.md §5.1, components.md §5")
}

/// Scene of one popup window: its background with the shadow forced off, then the member
/// items.
pub fn popup_scene(model: &Model, layout: &PopupLayout) -> Scene {
    let _ = (model, layout);
    todo!("WP-A: item.md §6.5")
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
    let _ = (item, origin, window_h, artwork, scene);
    todo!("WP-A: components.md §10.3")
}

/// Window under `p` (see [`WindowHit`]); only items with `drawing=on`.
pub fn window_at(model: &Model, layout: &Layout, p: Point) -> WindowHit {
    let _ = (model, layout, p);
    todo!("WP-A: item.md §9.2")
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

/// `get_bar_by_point` (half-open `CGRectContainsPoint`).
pub fn bar_at_point(model: &Model, p: Point) -> Option<u32> {
    let _ = (model, p);
    todo!("WP-A: item.md §9.2")
}

/// `get_popup_by_point`: popups of items with `drawing && popup.drawing` (half-open).
pub fn popup_at_point(model: &Model, p: Point) -> Option<ItemId> {
    let _ = (model, p);
    todo!("WP-A: item.md §9.2")
}

/// Converts a screen point to the item-local drawing space of `item` on `adid` (y up),
/// consistently (D2), for slider hit tests / drags and app_menu titles. `None` if the item
/// has no frame there.
pub fn item_local_point(item: &BarItem, adid: u32, p: Point) -> Option<Point> {
    let _ = (item, adid, p);
    todo!("WP-A: item.md §9.3, D2")
}

/// Slider track hit test in item-local space (`CGRectContainsPoint(track.bounds, p)`).
pub fn slider_track_contains(item: &BarItem, local: Point) -> bool {
    let _ = (item, local);
    todo!("WP-A: components.md §7.6")
}

/// Index into `app_menu.titles` of the title under `local` (extension).
pub fn app_menu_title_at(item: &BarItem, local: Point) -> Option<usize> {
    let _ = (item, local);
    todo!("WP-A: app_menu extension")
}
