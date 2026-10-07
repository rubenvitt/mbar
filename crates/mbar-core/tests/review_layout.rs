//! Regression tests for the layout review findings (L1–L6, R4, PERF-1, PERF-9).
//!
//! Metrics are `HeadlessResources`' (font size 14: advance 8.4, ascent 11.2, descent 2.8;
//! "ab" is 18 wide, "abc" 26, text height 15), see `wpa_layout.rs`.

mod wpc_common;

use mbar_core::bar::BarState;
use mbar_core::components::{FontSpec, ImageSource};
use mbar_core::geometry::{Point, Rect};
use mbar_core::item::{BarItem, ItemId, ItemType, Position};
use mbar_core::layout;
use mbar_core::platform::{
    DisplayInfo, HeadlessResources, ImageError, ImageInfo, MenuExtra, Resources, SpaceInfo,
    SystemQuery, SystemValue, TextMetrics,
};
use mbar_core::scene::{Primitive, Scene};
use mbar_core::Model;
use std::time::Instant;
use wpc_common::{runs_of, H};

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(x, y, w, h)
}

struct Fx {
    m: Model,
    res: HeadlessResources,
}

impl Fx {
    /// One display, top bar `0,0,1920,25`, padding 20/20, active display 1.
    fn new() -> Fx {
        let res = HeadlessResources::default();
        let mut m = Model::new();
        m.active_adid = 1;
        let mut bar = BarState::new(1, 1);
        bar.sid = 1;
        bar.frame = r(0.0, 0.0, 1920.0, 25.0);
        m.bars.push(bar);
        Fx { m, res }
    }

    /// Two displays: bar 1 `0,0,1920,40` (built-in display), bar 2 `1920,0,1920,25`.
    fn two_bars() -> Fx {
        let mut fx = Fx::new();
        fx.res.displays[0].builtin = true;
        let mut d2 = fx.res.displays[0].clone();
        d2.id = 2;
        d2.adid = 2;
        d2.builtin = false;
        d2.frame = r(1920.0, 0.0, 1920.0, 1080.0);
        fx.res.displays.push(d2);
        fx.m.bars[0].frame = r(0.0, 0.0, 1920.0, 40.0);
        let mut b2 = BarState::new(2, 2);
        b2.sid = 1;
        b2.frame = r(1920.0, 0.0, 1920.0, 25.0);
        fx.m.bars.push(b2);
        fx
    }

    fn item(&mut self, name: &str, pos: &str, label: &str) -> ItemId {
        let id = self.m.create_item(&mut self.res);
        let res = &mut self.res;
        let it = self.m.item_mut(id).unwrap();
        it.set_name(name);
        it.set_position(pos).unwrap();
        it.icon.drawing = false;
        it.label.set_string(label, true, res);
        id
    }

    fn it(&mut self, id: ItemId) -> &mut BarItem {
        self.m.item_mut(id).unwrap()
    }

    fn run(&mut self) -> layout::Layout {
        layout::layout(&mut self.m, &mut self.res)
    }

    fn add_popup_member(&mut self, host: ItemId, member: ItemId) {
        self.it(member).parent = Some(host);
        self.it(member).position = Position::Popup;
        self.it(host).popup.items.push(member);
    }
}

fn texts(scene: &Scene) -> Vec<Point> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Text { origin, .. } => Some(*origin),
            _ => None,
        })
        .collect()
}

// ----------------------------------------------------------------------------------------
// L1: every bar is painted with its own component bounds
// ----------------------------------------------------------------------------------------

#[test]
fn l1_bar_scene_uses_each_bars_own_bounds() {
    let mut fx = Fx::two_bars();
    let a = fx.item("a", "left", "abc");
    fx.it(a).background.enabled = true;
    fx.it(a).background.color = mbar_core::color::Color::from_hex(0xff00ff00);
    let lay = fx.run();
    assert_eq!(lay.bars.len(), 2);
    let s1 = layout::bar_scene(&fx.m, &lay.bars[0]);
    let s2 = layout::bar_scene(&fx.m, &lay.bars[1]);

    // Bar 1 (40 pt): centre 20, baseline (u32)(20 - 4.2) = 15 → text origin y 40 - 15;
    // auto background height 40 - (0 + 1) - (0 + 1) = 38.
    assert_eq!(texts(&s1), vec![Point::new(20.0, 25.0)]);
    let bg1 = s1.primitives.iter().find_map(|p| match p {
        Primitive::Rect { rect, .. } if rect.width == 26.0 => Some(*rect),
        _ => None,
    });
    assert_eq!(bg1, Some(r(20.0, 1.0, 26.0, 38.0)));
    // Bar 2 (25 pt): centre 12, baseline 7 → origin y 18; background 23 high.
    assert_eq!(texts(&s2), vec![Point::new(20.0, 18.0)]);

    // Same scenes as with each bar laid out on its own.
    for keep in [0usize, 1] {
        let mut solo = Fx::two_bars();
        let a = solo.item("a", "left", "abc");
        solo.it(a).background.enabled = true;
        solo.it(a).background.color = mbar_core::color::Color::from_hex(0xff00ff00);
        solo.m.bars = vec![solo.m.bars[keep].clone()];
        let l = solo.run();
        let expect = layout::bar_scene(&solo.m, &l.bars[0]);
        let got = if keep == 0 { &s1 } else { &s2 };
        assert_eq!(got.primitives, expect.primitives, "bar {}", keep + 1);
    }

    // The model keeps the last bar's bounds; bar 1 carries its own.
    assert!(lay.bars[1].geometry.is_empty());
    let g = lay.geometry_of(1, a).expect("bar 1 bounds differ");
    let mut copy = fx.m.item(a).unwrap().clone();
    g.apply(&mut copy);
    assert_eq!(copy.label.bounds.y, 15.0);
    assert_eq!(fx.m.item(a).unwrap().label.bounds.y, 7.0);
}

#[test]
fn l1_single_bar_has_no_geometry_overrides() {
    let mut fx = Fx::new();
    fx.item("a", "left", "abc");
    let lay = fx.run();
    assert!(lay.bars[0].geometry.is_empty());

    // Two bars of the same height: identical bounds, nothing stored either.
    let mut fx = Fx::two_bars();
    fx.m.bars[0].frame.height = 25.0;
    fx.item("a", "left", "abc");
    let lay = fx.run();
    assert!(lay.bars.iter().all(|b| b.geometry.is_empty()));
}

#[test]
fn l1_slider_click_hit_tests_the_clicked_bars_track() {
    let mut res = HeadlessResources::default();
    res.displays[0].builtin = true;
    let mut d2 = res.displays[0].clone();
    d2.id = 2;
    d2.adid = 2;
    d2.builtin = false;
    d2.uuid = Some("00000000-0000-0000-0000-000000000002".into());
    d2.frame = r(1920.0, 0.0, 1920.0, 1080.0);
    res.displays.push(d2);
    let mut h = H::with_res(res);
    h.msg(&["--bar", "height=25", "notch_display_height=40"]);
    h.msg(&["--add", "slider", "s", "left", "100"]);
    h.msg(&[
        "--set",
        "s",
        "icon.drawing=off",
        "label.drawing=off",
        "slider.background.height=10",
        "slider.background.drawing=on",
        "click_script=echo clicked",
    ]);
    let heights: Vec<f32> = h.rt.model.bars.iter().map(|b| b.frame.height).collect();
    assert_eq!(heights, vec![40.0, 25.0]);
    let f =
        h.rt.model
            .item(h.rt.model.find("s").unwrap())
            .unwrap()
            .frame(1)
            .unwrap();
    // Local y 20: inside bar 1's track (15..25), outside bar 2's (7..17).
    let fx = h.click(f.x + 50.0, f.y + f.height - 20.0);
    let runs = runs_of(&fx, "s");
    assert_eq!(
        runs.len(),
        1,
        "click on bar 1's track runs the click script"
    );
    assert_eq!(runs[0].get("PERCENTAGE"), Some("50"));
}

// ----------------------------------------------------------------------------------------
// L2: app_menu title cells survive another bar's pass
// ----------------------------------------------------------------------------------------

fn app_menu(fx: &mut Fx) -> ItemId {
    let am = fx.item("menu", "left", "");
    let it = fx.it(am);
    it.set_type(ItemType::AppMenu, "/h");
    it.label.drawing = false;
    it.app_menu.app_name = "Finder".into();
    it.app_menu.titles = vec![
        "Apple".into(),
        "Finder".into(),
        "File".into(),
        "Edit".into(),
    ];
    am
}

#[test]
fn l2_app_menu_cells_kept_when_not_drawn_on_the_last_bar() {
    let mut fx = Fx::two_bars();
    fx.m.bars[0].frame.height = 25.0;
    let am = app_menu(&mut fx);
    fx.it(am).associated_display = 1 << 1;
    let lay = fx.run();
    assert_eq!(lay.bars[0].items.len(), 1);
    assert!(lay.bars[1].items.is_empty());
    let cells = fx.m.item(am).unwrap().app_menu.title_bounds.clone();
    let xs: Vec<f32> = cells.iter().map(|c| c.x).collect();
    assert_eq!(xs, vec![0.0, 64.0, 112.0]);
    // Centre 12, cell height 11.2 + 2.8 + 6 = 20 → y = 2.
    assert!(cells.iter().all(|c| c.y == 2.0 && c.height == 20.0));
    // Hit testing sees the laid-out cells.
    let item = fx.m.item(am).unwrap();
    assert_eq!(
        layout::app_menu_title_at(item, Point::new(70.0, 12.0)),
        Some(2)
    );
    // The scene draws the titles side by side.
    let s = layout::bar_scene(&fx.m, &lay.bars[0]);
    let xs: Vec<f32> = texts(&s).iter().map(|p| p.x).collect();
    assert_eq!(xs, vec![27.0, 91.0, 139.0]);
}

// ----------------------------------------------------------------------------------------
// PERF-9: app_menu titles are measured once, not on every pass
// ----------------------------------------------------------------------------------------

/// `HeadlessResources` counting `text_metrics` calls.
struct Counting {
    inner: HeadlessResources,
    calls: Vec<String>,
}

impl Resources for Counting {
    fn text_metrics(&mut self, font: &FontSpec, text: &str) -> TextMetrics {
        self.calls.push(text.to_string());
        self.inner.text_metrics(font, text)
    }
    fn load_image(&mut self, source: &ImageSource) -> Result<ImageInfo, ImageError> {
        self.inner.load_image(source)
    }
    fn displays(&self) -> &[DisplayInfo] {
        self.inner.displays()
    }
    fn spaces(&self) -> &[SpaceInfo] {
        self.inner.spaces()
    }
    fn active_display(&self) -> u32 {
        self.inner.active_display()
    }
    fn menu_bar_visible(&self) -> bool {
        self.inner.menu_bar_visible()
    }
    fn now(&self) -> Instant {
        self.inner.now()
    }
    fn menu_extras(&mut self) -> Option<Vec<MenuExtra>> {
        self.inner.menu_extras()
    }
    fn query_system(&mut self, q: SystemQuery) -> Option<SystemValue> {
        self.inner.query_system(q)
    }
}

#[test]
fn perf9_app_menu_titles_measured_only_when_they_change() {
    let mut fx = Fx::two_bars();
    let am = app_menu(&mut fx);
    let mut res = Counting {
        inner: HeadlessResources::default(),
        calls: Vec::new(),
    };
    let first = layout::layout(&mut fx.m, &mut res);
    // One measurement per drawn title on the first pass (two bars).
    assert_eq!(res.calls, vec!["Finder", "File", "Edit"]);
    res.calls.clear();
    let again = layout::layout(&mut fx.m, &mut res);
    assert!(res.calls.is_empty(), "re-measured: {:?}", res.calls);
    assert_eq!(first, again);
    assert_eq!(again.bars[0].menu_lines.len(), 1);
    assert_eq!(again.bars[0].menu_lines[0].1.len(), 3);

    // New titles, spacing or fonts invalidate the cache.
    fx.it(am).app_menu.titles.push("View".into());
    layout::layout(&mut fx.m, &mut res);
    assert_eq!(res.calls.len(), 4);
    res.calls.clear();
    fx.it(am).app_menu.spacing = 10;
    layout::layout(&mut fx.m, &mut res);
    assert_eq!(res.calls.len(), 4);
    let xs: Vec<f32> =
        fx.m.item(am)
            .unwrap()
            .app_menu
            .title_bounds
            .iter()
            .map(|c| c.x)
            .collect();
    assert_eq!(xs, vec![0.0, 60.0, 104.0, 148.0]);
    res.calls.clear();
    fx.it(am).label.font.size = 20.0;
    layout::layout(&mut fx.m, &mut res);
    assert!(res.calls.iter().filter(|t| t.as_str() != "").count() >= 4);
}

// ----------------------------------------------------------------------------------------
// L3 (documented as D21): item windows are clipped to their host window
// ----------------------------------------------------------------------------------------

#[test]
fn l3_d21_member_wider_than_popup_is_clipped_to_the_popup_window() {
    let mut fx = Fx::new();
    let host = fx.item("host", "left", "ab");
    let m = fx.item("m", "left", "abcdefgh");
    fx.add_popup_member(host, m);
    fx.it(m).set_width(30);
    fx.it(host).popup.drawing = true;
    let lay = fx.run();
    let pl = &lay.popups[0];
    // Popup width pl + pr + slot + bw = 30; the member window keeps get_length(true) = 68.
    assert_eq!(pl.frame, r(20.0, 25.0, 30.0, 25.0));
    assert_eq!(pl.items[0].frame, r(20.0, 25.0, 68.0, 25.0));
    let s = layout::popup_scene(&fx.m, pl);
    assert_eq!(s.size.width, 30.0);
    assert!(s.primitives.iter().any(|p| matches!(
        p,
        Primitive::PushClip { rect, .. } if *rect == r(0.0, 0.0, 68.0, 25.0)
    )));
    // Hit testing uses the full item window (D21).
    assert_eq!(
        layout::window_at(&fx.m, &lay, Point::new(80.0, 30.0)),
        layout::WindowHit::Item(m)
    );
}

// ----------------------------------------------------------------------------------------
// L4: slider knob offset uses C's single-precision quotient
// ----------------------------------------------------------------------------------------

#[test]
fn l4_knob_offset_matches_float_quotient() {
    let mut fx = Fx::new();
    let s = fx.item("s", "left", "");
    {
        let res = &mut fx.res;
        let it = fx.m.item_mut(s).unwrap();
        it.set_type(ItemType::Slider, "/h");
        it.label.drawing = false;
        it.slider.setup(100);
        it.slider.track.set_height(10);
        it.slider.knob.set_string("ab", true, res);
    }
    let mut mismatches = Vec::new();
    for pct in 0..=100u32 {
        fx.it(s).slider.percentage = pct;
        fx.run();
        let it = fx.m.item(s).unwrap();
        let kw = it.slider.knob.bounds.width as f64;
        assert_eq!(kw, 18.0);
        // C: int32 raw = ((float)pct)/100.f * W - kw/2.; max(min(raw, W - (kw + 1)), 0).
        let raw = (((pct as f32) / 100.0f32) as f64 * 100.0 - kw / 2.0) as i32;
        let off = (raw as f64).min(100.0 - (kw + 1.0)).max(0.0) as u32;
        if it.slider.knob.bounds.x != off as f32 {
            mismatches.push((pct, off, it.slider.knob.bounds.x));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:?}");
    // pct 11: 0.11f·100 − 9 = 1.99999994 → 1 (the double product would give 2).
    fx.it(s).slider.percentage = 11;
    fx.run();
    assert_eq!(fx.m.item(s).unwrap().slider.knob.bounds.x, 1.0);
}

// ----------------------------------------------------------------------------------------
// L5: popup background image y is truncated like image_calculate_bounds' uint32_t y
// ----------------------------------------------------------------------------------------

#[test]
fn l5_popup_background_image_y_truncated() {
    let mut fx = Fx::new();
    let host = fx.item("host", "left", "ab");
    let p1 = fx.item("p1", "left", "abc");
    fx.add_popup_member(host, p1);
    fx.it(host).popup.drawing = true;
    {
        let bg = &mut fx.it(host).popup.background;
        bg.enabled = true;
        bg.border_width = 0;
        bg.image.enabled = true;
        bg.image.bounds = r(0.0, 0.0, 100.0, 15.0);
    }
    fx.run();
    // (u32)(0 + 7.5) = 7 → 7 − 7.5 = −0.5.
    let img = fx.m.item(host).unwrap().popup.background.image.bounds;
    assert_eq!(img, r(0.0, -0.5, 100.0, 15.0));
}

// ----------------------------------------------------------------------------------------
// L6: bracket first/last member with C's int-truncated min/max
// ----------------------------------------------------------------------------------------

#[test]
fn l6_bracket_last_member_uses_truncated_max() {
    let mut fx = Fx::new();
    let host = fx.item("host", "left", "ab"); // window 20,0,18,25
    let a = fx.item("a", "left", "abc");
    let b = fx.item("b", "left", "abc");
    fx.it(a).background.padding_right = 11;
    fx.add_popup_member(host, a);
    fx.add_popup_member(host, b);
    let br = fx.item("br", "left", "");
    fx.it(br).set_type(ItemType::Bracket, "/h");
    fx.it(br).bracket_members = vec![a, b];
    fx.add_popup_member(host, br);
    fx.it(host).popup.align = b'c';
    fx.it(host).popup.drawing = true;
    fx.run();
    let fa = fx.m.item(a).unwrap().frame(1).unwrap();
    let fb = fx.m.item(b).unwrap().frame(1).unwrap();
    // PB.w = 26 + 11 = 37 → anchor x = 20 + (18 − 37)/2 = 10.5.
    assert_eq!((fa.x, fa.width), (10.5, 26.0));
    assert_eq!((fb.x, fb.width), (10.5, 26.0));
    // a sets max = (int)36.5 = 36; b's 36.5 > 36 makes b the last member (pr 0):
    // len = 36.5 + 0 + 0 − 10.5 = 26 (not 37 with a's padding_right).
    let fbr = fx.m.item(br).unwrap().frame(1).unwrap();
    assert_eq!((fbr.x, fbr.width), (10.5, 26.0));
}

#[test]
fn l6_bracket_first_member_negative_fraction() {
    // Members at x = -27.5: a sets min = (int)-27.5 = -27, b's -27.5 < -27 makes b first.
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "abc");
    let b = fx.item("b", "left", "abc");
    let br = fx.item("br", "left", "");
    fx.it(br).set_type(ItemType::Bracket, "/h");
    fx.it(br).bracket_members = vec![a, b];
    fx.it(a).background.padding_left = 7;
    fx.it(a).set_frame(1, r(-27.5, 0.0, 26.0, 25.0));
    fx.it(b).set_frame(1, r(-27.5, 0.0, 26.0, 25.0));
    let bar = fx.m.bars[0].clone();
    let f = layout::bracket_bounds(&mut fx.m, br, &bar, 12);
    // first = b (pl 0): x = -27.5 (not a: -34.5, 33 wide); last = a (max = (int)-1.5 = -1,
    // b's -1.5 > -1 is false), pr 0 → len = -1.5 - (-27.5) = 26.
    assert_eq!((f.x, f.width), (-27.5, 26.0));
}

// ----------------------------------------------------------------------------------------
// R4: vertical bar with y_offset = i32::MIN
// ----------------------------------------------------------------------------------------

#[test]
fn r4_vertical_y_offset_min_does_not_overflow() {
    for pos in ["left", "right"] {
        let mut h = H::new();
        h.msg(&["--bar", &format!("position={pos}")]);
        h.msg(&["--add", "item", "a", "left"]);
        h.msg(&["--set", "a", "y_offset=-2147483648"]);
        let id = h.rt.model.find("a").unwrap();
        let f = h.rt.model.item(id).unwrap().frame(1).unwrap();
        // y = bar.y + cur − 2147483648 (in f64), height = ih + 2147483648.
        assert!(f.y < -2.0e9 && f.height > 2.0e9, "{f:?}");
    }
}

// ----------------------------------------------------------------------------------------
// PERF-1: placed items are resolved without per-item linear scans
// ----------------------------------------------------------------------------------------

#[test]
fn perf1_popup_members_out_of_global_order_resolve() {
    let mut fx = Fx::new();
    let host = fx.item("host", "left", "ab");
    let p1 = fx.item("p1", "left", "abc");
    let p2 = fx.item("p2", "left", "a");
    // Popup order p2, p1 (reverse of the global order).
    fx.add_popup_member(host, p2);
    fx.add_popup_member(host, p1);
    fx.it(host).popup.drawing = true;
    let lay = fx.run();
    let pl = &lay.popups[0];
    assert_eq!(
        pl.items.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![p2, p1]
    );
    let s = layout::popup_scene(&fx.m, pl);
    let clips: Vec<Rect> = s
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::PushClip { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(
        clips,
        vec![r(0.0, 0.0, 9.0, 25.0), r(0.0, 25.0, 26.0, 25.0)]
    );
    assert_eq!(texts(&s).len(), 2);
    // Both members are hit-testable through window_at.
    let f1 = fx.m.item(p1).unwrap().frame(1).unwrap();
    assert_eq!(
        layout::window_at(&fx.m, &lay, Point::new(f1.x + 1.0, f1.y + 1.0)),
        layout::WindowHit::Item(p1)
    );
}

#[test]
fn perf1_bar_scene_with_many_items_matches_per_item_lookup() {
    let mut fx = Fx::new();
    let mut ids = Vec::new();
    for k in 0..300 {
        let pos = if k % 2 == 0 { "left" } else { "right" };
        let id = fx.item(&format!("i{k}"), pos, "ab");
        fx.it(id).background.enabled = true;
        fx.it(id).background.color = mbar_core::color::Color::from_hex(0xff112233);
        ids.push(id);
    }
    let lay = fx.run();
    assert_eq!(lay.bars[0].items.len(), 300);
    let s = layout::bar_scene(&fx.m, &lay.bars[0]);
    // Every item window painted once, in global order.
    let clips: Vec<Rect> = s
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::PushClip { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    let frames: Vec<Rect> = ids
        .iter()
        .map(|id| fx.m.item(*id).unwrap().frame(1).unwrap())
        .collect();
    assert_eq!(clips, frames);
}
