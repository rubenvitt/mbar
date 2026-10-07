//! WP-A integration tests: bar frames, horizontal/vertical layout, brackets, popups,
//! component bounds, scenes and hit testing (`layout.rs`).
//!
//! All expected numbers are derived by hand from the spec formulas with the
//! `HeadlessResources` metrics: default font size 14 → glyph advance 8.4, ascent 11.2,
//! descent 2.8, ink box `(0, -2.8, 8.4·n, 14)`. After `text_prepare_line`:
//! `width = (u32)(8.4·n + 1.5)` ("a" 9, "ab" 18, "abc" 26), `bounds.h = (u32)(14 + 1.5) = 15`,
//! empty string → width 1, height 1. Baseline for centre `y`:
//! `(u32)(y - (11.2 - 2.8)/2) = (u32)(y - 4.2)`.

use mbar_core::bar::BarState;
use mbar_core::geometry::{Point, Rect, Size};
use mbar_core::item::{ItemId, ItemType, Position};
use mbar_core::layout::{self, WindowHit, NIRVANA};
use mbar_core::platform::{level, HeadlessResources, ImageInfo, ImageKey};
use mbar_core::scene::Primitive;
use mbar_core::Model;

// ----------------------------------------------------------------------------------------
// Fixtures
// ----------------------------------------------------------------------------------------

struct Fx {
    m: Model,
    res: HeadlessResources,
}

impl Fx {
    /// One 1920×1080 display, top bar of height 25 directly at the top of the screen
    /// (frame `0,0,1920,25`), bar padding 20/20, active display 1.
    fn new() -> Fx {
        let res = HeadlessResources::default();
        let mut m = Model::new();
        m.active_adid = 1;
        let mut bar = BarState::new(1, 1);
        bar.sid = 1;
        bar.frame = Rect::new(0.0, 0.0, 1920.0, 25.0);
        m.bars.push(bar);
        Fx { m, res }
    }

    /// An item with only a label (icon `drawing=off`).
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

    fn it(&mut self, id: ItemId) -> &mut mbar_core::item::BarItem {
        self.m.item_mut(id).unwrap()
    }

    fn frame(&self, id: ItemId) -> Rect {
        self.m.item(id).unwrap().frame(1).unwrap()
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

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(x, y, w, h)
}

// ----------------------------------------------------------------------------------------
// bar_frame (bar.md §4.1)
// ----------------------------------------------------------------------------------------

#[test]
fn bar_frame_top_bottom_vertical_notch() {
    let res = HeadlessResources::default();
    let mut d = res.displays[0].clone();
    let mut bar = Model::new().bar;

    // Top, menu bar visible (24 pt), not topmost.
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(0.0, 24.0, 1920.0, 25.0)
    );
    assert_eq!(
        layout::bar_frame(&bar, &d, false),
        r(0.0, 0.0, 1920.0, 25.0)
    );
    bar.margin = 10;
    bar.background.y_offset = 5;
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(10.0, 29.0, 1900.0, 25.0)
    );
    // topmost disables the menu-bar avoidance.
    bar.topmost = true;
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(10.0, 5.0, 1900.0, 25.0)
    );
    bar.topmost = false;

    // Bottom: y = maxY - H - 2*Y (quirk) - notch_offset.
    bar.position = b'b';
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(10.0, 1045.0, 1900.0, 25.0)
    );

    // Built-in display: notch offset/height apply.
    d.builtin = true;
    bar.notch_offset = 3;
    bar.notch_display_height = 32;
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(10.0, 1042.0, 1900.0, 32.0)
    );
    bar.position = b't';
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(10.0, 32.0, 1900.0, 32.0)
    );
    // Any other position char behaves like top.
    bar.position = b'x';
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(10.0, 32.0, 1900.0, 32.0)
    );

    // Vertical: notch ignored, width = thickness, height minus 2*Y and the menu bar.
    bar.position = b'l';
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(10.0, 29.0, 25.0, 1046.0)
    );
    bar.position = b'r';
    assert_eq!(
        layout::bar_frame(&bar, &d, true),
        r(1885.0, 29.0, 25.0, 1046.0)
    );
    assert_eq!(
        layout::bar_frame(&bar, &d, false),
        r(1885.0, 5.0, 25.0, 1070.0)
    );
}

// ----------------------------------------------------------------------------------------
// Horizontal layout (bar.md §4.4, item.md §4.5)
// ----------------------------------------------------------------------------------------

#[test]
fn horizontal_left_right_center_cursors() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab"); // 18
    let b = fx.item("b", "left", "abc"); // 26
    fx.it(b).background.padding_left = 5;
    fx.it(b).background.padding_right = 3;
    let c = fx.item("c", "right", "a"); // 9
    fx.it(c).background.padding_left = 2;
    fx.it(c).background.padding_right = 4;
    let d = fx.item("d", "right", "ab"); // 18
    let e = fx.item("e", "center", "ab"); // 18
    fx.it(e).background.padding_left = 1;
    fx.it(e).background.padding_right = 1;
    let f = fx.item("f", "center", "abc"); // 26

    let bar = fx.m.bars[0].clone();
    assert_eq!(
        layout::side_length(&fx.m, &bar, Position::Center, false),
        46
    );
    let lay = fx.run();

    // left: cursor 20, +18 → 38; b: 38+5 = 43, then 43+26+3 = 72.
    assert_eq!(fx.frame(a), r(20.0, 0.0, 18.0, 25.0));
    assert_eq!(fx.frame(b), r(43.0, 0.0, 26.0, 25.0));
    // right: cursor 1900; c: 1900-9-4 = 1887, advance -pl → 1885; d: 1885-18 = 1867.
    assert_eq!(fx.frame(c), r(1887.0, 0.0, 9.0, 25.0));
    assert_eq!(fx.frame(d), r(1867.0, 0.0, 18.0, 25.0));
    // centre: (1920-46)/2 = 937; e: 938; 938+18+1 = 957; f: 957.
    assert_eq!(fx.frame(e), r(938.0, 0.0, 18.0, 25.0));
    assert_eq!(fx.frame(f), r(957.0, 0.0, 26.0, 25.0));

    // Placed items in global order; graph rtl flags.
    let ids: Vec<ItemId> = lay.bars[0].items.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![a, b, c, d, e, f]);
    assert!(fx.m.item(c).unwrap().graph.rtl);
    assert!(!fx.m.item(a).unwrap().graph.rtl);

    // Label bounds: x = item-local content start (0), baseline (u32)(12 - 4.2) = 7.
    let lb = fx.m.item(a).unwrap().label.bounds;
    assert_eq!((lb.x, lb.y, lb.width, lb.height), (0.0, 7.0, 18.0, 15.0));
}

#[test]
fn horizontal_default_icon_and_label_occupy_one_point() {
    // A fresh item inherits an empty laid-out icon/label: width 1 each (Q7).
    let mut fx = Fx::new();
    let id = fx.m.create_item(&mut fx.res);
    fx.it(id).set_name("x");
    fx.run();
    assert_eq!(fx.frame(id), r(20.0, 0.0, 2.0, 25.0));
    assert_eq!(fx.m.item(id).unwrap().label.bounds.x, 1.0);
}

#[test]
fn horizontal_const_width_and_alignment() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab");
    fx.it(a).set_width(60);
    fx.it(a).background.padding_left = 10;
    fx.it(a).background.padding_right = 7; // ignored for const LTR items
    let b = fx.item("b", "left", "a");
    fx.it(b).align = b'c';
    fx.it(b).set_width(30);
    let c = fx.item("c", "left", "a");
    fx.it(c).align = b'r';
    fx.it(c).set_width(30);
    fx.run();
    // a: cursor 20+10 = 30, width 60; advance custom_width - pl → 30 + 50 = 80 (net +60).
    assert_eq!(fx.frame(a), r(30.0, 0.0, 60.0, 25.0));
    // b: 80, centred content (30-9)/2 = 10.
    assert_eq!(fx.frame(b), r(80.0, 0.0, 30.0, 25.0));
    assert_eq!(fx.m.item(b).unwrap().label.bounds.x, 10.0);
    // c: 110, right-aligned content 30-9 = 21.
    assert_eq!(fx.frame(c), r(110.0, 0.0, 30.0, 25.0));
    assert_eq!(fx.m.item(c).unwrap().label.bounds.x, 21.0);

    // A const width smaller than the content: display length = content (window never
    // shrinks), slot = custom_width.
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "abc"); // 26
    fx.it(a).set_width(10);
    let b = fx.item("b", "left", "a");
    fx.run();
    assert_eq!(fx.frame(a), r(20.0, 0.0, 26.0, 25.0));
    assert_eq!(fx.frame(b), r(30.0, 0.0, 9.0, 25.0));
}

#[test]
fn horizontal_const_width_rtl_advance() {
    let mut fx = Fx::new();
    let a = fx.item("a", "right", "ab"); // disp 40 (const)
    fx.it(a).set_width(40);
    fx.it(a).background.padding_right = 5;
    fx.it(a).background.padding_left = 3;
    let b = fx.item("b", "right", "a");
    fx.run();
    // a: 1900-40-5 = 1855; advance disp + pr - custom = 40+5-40 → 1860 (net: left by 40).
    assert_eq!(fx.frame(a), r(1855.0, 0.0, 40.0, 25.0));
    // b: 1860 - 9 = 1851.
    assert_eq!(fx.frame(b), r(1851.0, 0.0, 9.0, 25.0));
}

#[test]
fn horizontal_rtl_unsigned_wrap_and_clamps() {
    let mut fx = Fx::new();
    fx.m.bars[0].frame = r(0.0, 0.0, 100.0, 25.0);
    let a = fx.item("a", "right", "abc"); // 26
    fx.it(a).set_width(90);
    let b = fx.item("b", "right", "a"); // 9
    fx.run();
    // a: cand = 80-90 wraps → huge → W - disp = 10.
    assert_eq!(fx.frame(a).x, 10.0);
    // advance: 10 + 90 - 90 = 10; b: cand = 10 - 9 = 1.
    assert_eq!(fx.frame(b).x, 1.0);

    // W - disp < 0: D10 clamps the conversion at 0.
    let mut fx = Fx::new();
    fx.m.bars[0].frame = r(0.0, 0.0, 100.0, 25.0);
    let a = fx.item("a", "right", "a");
    fx.it(a).set_width(120);
    fx.run();
    assert_eq!(fx.frame(a), r(0.0, 0.0, 120.0, 25.0));

    // Negative padding_right cannot push an item past the right edge.
    let mut fx = Fx::new();
    let a = fx.item("a", "right", "a");
    fx.it(a).background.padding_right = -50;
    fx.run();
    assert_eq!(fx.frame(a).x, 1911.0);

    // Left cursor clamp: negative padding never starts before x = 0.
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "a");
    fx.it(a).background.padding_left = -50;
    fx.run();
    assert_eq!(fx.frame(a).x, 0.0);
}

#[test]
fn horizontal_notch_q_and_e() {
    let mut fx = Fx::new();
    fx.res.displays[0].builtin = true; // notch_width 200
    let q1 = fx.item("q1", "q", "ab");
    let q2 = fx.item("q2", "q", "a");
    let e1 = fx.item("e1", "e", "ab");
    let e2 = fx.item("e2", "e", "a");
    fx.run();
    // q: (1920-200)/2 = 860 → 842, then 842-9 = 833; e: (1920+200)/2 = 1060 → 1078.
    assert_eq!(fx.frame(q1).x, 842.0);
    assert_eq!(fx.frame(q2).x, 833.0);
    assert_eq!(fx.frame(e1).x, 1060.0);
    assert_eq!(fx.frame(e2).x, 1078.0);
    assert!(fx.m.item(q1).unwrap().graph.rtl);
    assert!(!fx.m.item(e1).unwrap().graph.rtl);

    // Non built-in display: both start at W/2.
    fx.res.displays[0].builtin = false;
    fx.run();
    assert_eq!(fx.frame(q1).x, 942.0);
    assert_eq!(fx.frame(e1).x, 960.0);
}

#[test]
fn horizontal_shadow_extents_widen_window() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab");
    // Default shadow: angle 30, distance 5 → offset.x 4.33 → right extent 4.
    fx.it(a).label.shadow.enabled = true;
    let b = fx.item("b", "left", "ab");
    fx.it(b).label.shadow.set_angle(180); // offset.x = -5 → left extent 5
    fx.it(b).label.shadow.enabled = true;
    fx.run();
    assert_eq!(fx.m.item(a).unwrap().shadow_extents(), (0, 4));
    assert_eq!(fx.frame(a), r(20.0, 0.0, 22.0, 25.0));
    // b: cursor 38; window shifted left by 5 and widened; content starts at local x 5.
    assert_eq!(fx.frame(b), r(33.0, 0.0, 23.0, 25.0));
    assert_eq!(fx.m.item(b).unwrap().label.bounds.x, 5.0);
}

#[test]
fn horizontal_skips_undrawn_and_parks_at_nirvana() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab");
    let b = fx.item("b", "left", "a");
    fx.run();
    assert_eq!(fx.frame(b).x, 38.0);
    fx.it(a).drawing = false;
    let lay = fx.run();
    assert_eq!(fx.frame(b).x, 20.0);
    // a keeps its size at nirvana.
    assert_eq!(fx.frame(a), r(NIRVANA.x, NIRVANA.y, 18.0, 25.0));
    assert_eq!(lay.bars[0].items.len(), 1);

    // Space association on another space.
    fx.it(a).drawing = true;
    fx.it(a).associated_space = 1 << 2;
    fx.run();
    assert_eq!(fx.frame(a).origin(), NIRVANA);

    // Hidden bar: frame and all items at nirvana.
    fx.m.bars[0].hidden = true;
    let lay = fx.run();
    assert_eq!(lay.bars[0].frame, r(NIRVANA.x, NIRVANA.y, 1920.0, 25.0));
    assert!(lay.bars[0].items.is_empty());
    assert_eq!(fx.frame(b).origin(), NIRVANA);

    // Bars with sid < 1 are skipped entirely.
    fx.m.bars[0].hidden = false;
    fx.m.bars[0].sid = 0;
    assert!(fx.run().bars.is_empty());
}

#[test]
fn horizontal_frames_on_second_display() {
    let mut fx = Fx::new();
    let mut b2 = BarState::new(2, 2);
    b2.sid = 1;
    b2.frame = r(1920.0, 0.0, 1280.0, 30.0);
    fx.m.bars.push(b2);
    let a = fx.item("a", "left", "ab");
    let b = fx.item("b", "right", "a");
    fx.it(b).associated_display = 1 << 2;
    fx.run();
    let ai = fx.m.item(a).unwrap();
    assert_eq!(ai.frame(1), Some(r(20.0, 0.0, 18.0, 25.0)));
    assert_eq!(ai.frame(2), Some(r(1940.0, 0.0, 18.0, 30.0)));
    let bi = fx.m.item(b).unwrap();
    // Only on display 2: 1920 + (1280 - 20 - 9).
    assert_eq!(bi.frame(1).unwrap().origin(), NIRVANA);
    assert_eq!(bi.frame(2), Some(r(3171.0, 0.0, 9.0, 30.0)));
}

// ----------------------------------------------------------------------------------------
// Item internals (item.md §4.3, components.md §4.8, §5.3, §7.3, §8.4)
// ----------------------------------------------------------------------------------------

#[test]
fn item_background_auto_height_double_subtraction() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab");
    fx.it(a).background.enabled = true;
    fx.m.bar.background.border_width = 1;
    fx.run();
    // bar_height = 25 - 2 = 23; background h = 23 - 2 = 21; y = 12 - 10 = 2.
    let bg = &fx.m.item(a).unwrap().background;
    assert_eq!(bg.bounds, r(0.0, 2.0, 18.0, 21.0));
    assert_eq!(bg.height, 21);
    assert!(!bg.overrides_height);

    // Explicit height and y_offset (content y = 12 + 3 = 15).
    fx.it(a).background.set_height(10);
    fx.it(a).y_offset = 3;
    fx.run();
    let it = fx.m.item(a).unwrap();
    assert_eq!(it.background.bounds, r(0.0, 10.0, 18.0, 10.0));
    assert_eq!(it.label.bounds.y, 10.0); // (u32)(15 - 4.2)

    // D10: y_offset below 0 and a background taller than 2*y clamp at 0 (no wrap).
    fx.it(a).y_offset = -20;
    fx.it(a).background.set_height(30);
    fx.run();
    let it = fx.m.item(a).unwrap();
    assert_eq!(it.background.bounds, r(0.0, 0.0, 18.0, 30.0));
    assert_eq!(it.label.bounds.y, 0.0);
}

#[test]
fn item_text_background_and_order_icon_label() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "abc");
    {
        let res = &mut fx.res;
        let it = fx.m.item_mut(a).unwrap();
        it.icon.drawing = true;
        it.icon.set_string("ab", true, res);
        it.icon.padding_left = 2;
        it.icon.padding_right = 3;
        it.icon.background.enabled = true;
        it.label.background.enabled = true;
        it.label.background.set_height(8);
    }
    fx.run();
    let it = fx.m.item(a).unwrap();
    // icon length 2+18+3 = 23 → label at 23; window 23 + 26 = 49 wide.
    assert_eq!(fx.frame(a).width, 49.0);
    assert_eq!(it.icon.bounds.x, 0.0);
    assert_eq!(it.label.bounds.x, 23.0);
    // icon background: auto height = text bounds.h 15, at y 12 - 7 = 5, width 23.
    assert_eq!(it.icon.background.bounds, r(0.0, 5.0, 23.0, 15.0));
    // label background: explicit 8 → y 8.
    assert_eq!(it.label.background.bounds, r(23.0, 8.0, 26.0, 8.0));
}

#[test]
fn text_alignment_inside_const_width() {
    let mut res = HeadlessResources::default();
    let mut t = mbar_core::components::Text::default();
    t.set_string("a", true, &mut res); // 9
    t.set_width(30);
    t.align = b'c';
    layout::text_calculate_bounds(&mut t, 4, 12);
    assert_eq!(t.bounds.x, 4.0 + 10.0); // (30 - 9)/2 = 10
    t.align = b'r';
    layout::text_calculate_bounds(&mut t, 4, 12);
    assert_eq!(t.bounds.x, 25.0);
    t.align = b'l';
    layout::text_calculate_bounds(&mut t, 4, 12);
    assert_eq!(t.bounds.x, 4.0);
    // Natural length larger than custom width: negative offset, int division toward 0.
    t.set_string("abcd", true, &mut res); // 35
    t.align = b'c';
    layout::text_calculate_bounds(&mut t, 4, 12);
    assert_eq!(t.bounds.x, 4.0 - 2.0); // (30-35)/2 = -2
}

#[test]
fn background_bounds_writes_height_and_lays_out_image() {
    let mut bg = mbar_core::components::Background::default();
    bg.image.enabled = true;
    bg.image.bounds = r(0.0, 0.0, 16.0, 10.0);
    bg.image.padding_left = 2;
    bg.image.y_offset = 1;
    layout::background_calculate_bounds(&mut bg, 3, 12, 40, 21);
    assert_eq!(bg.bounds, r(3.0, 2.0, 40.0, 21.0)); // 12 - 21/2 (=10)
    assert_eq!(bg.height, 21);
    // image: x + padding_left, y - h/2 + y_offset.
    assert_eq!(bg.image.bounds, r(5.0, 8.0, 16.0, 10.0));
    // D10 clamp.
    layout::background_calculate_bounds(&mut bg, 0, 3, 5, 20);
    assert_eq!(bg.bounds.y, 0.0);
}

#[test]
fn media_artwork_normalised_to_32pt() {
    let mut img = mbar_core::components::Image {
        link: true,
        enabled: true,
        scale: 0.5,
        ..Default::default()
    };
    layout::image_calculate_bounds(&mut img, 0.0, 20.0, Some(Size::new(300.0, 150.0)));
    // k = 32/150 → 64 × 32, × scale 0.5 → 32 × 16; y = 20 - 8.
    assert_eq!(img.bounds, r(0.0, 12.0, 32.0, 16.0));
}

#[test]
fn slider_track_fill_knob() {
    let mut fx = Fx::new();
    let s = fx.item("s", "left", "");
    {
        let res = &mut fx.res;
        let it = fx.m.item_mut(s).unwrap();
        it.set_type(ItemType::Slider, "/h");
        it.label.drawing = false;
        it.slider.setup(100);
        it.slider.track.set_height(10);
        it.slider.knob.set_string("o", true, res); // knob ink width 9
        it.slider.percentage = 50;
    }
    fx.run();
    let it = fx.m.item(s).unwrap();
    assert_eq!(fx.frame(s), r(20.0, 0.0, 100.0, 25.0));
    assert_eq!(it.slider.track.bounds, r(0.0, 7.0, 100.0, 10.0));
    assert_eq!(it.slider.fill.bounds, r(0.0, 7.0, 50.0, 10.0));
    // knob: (i32)(50 - 4.5) = 45.
    assert_eq!(it.slider.knob.bounds.x, 45.0);
    fx.it(s).slider.percentage = 100;
    fx.run();
    // min(95, 100 - 10) = 90.
    assert_eq!(fx.m.item(s).unwrap().slider.knob.bounds.x, 90.0);
    fx.it(s).slider.percentage = 0;
    fx.run();
    assert_eq!(fx.m.item(s).unwrap().slider.knob.bounds.x, 0.0);
}

#[test]
fn graph_bounds_and_d11_height() {
    let mut fx = Fx::new();
    let g = fx.item("g", "left", "");
    {
        let it = fx.m.item_mut(g).unwrap();
        it.set_type(ItemType::Graph, "/h");
        it.label.drawing = false;
        it.graph.setup(4);
    }
    fx.run();
    // No background: H = (25 - 1) - 1 = 23; y = 12 - 11.5 + 0.5 = 1.
    let it = fx.m.item(g).unwrap();
    assert_eq!(it.graph.bounds, r(0.0, 1.0, 4.0, 23.0));
    assert_eq!(fx.frame(g).width, 4.0);

    // Background with auto height: D11 uses this pass's 23 → 23 - 0 - 1 = 22, even on the
    // very first pass (C would read the previous height 0 → 0 - 1 → clamp).
    fx.it(g).background.enabled = true;
    fx.it(g).background.border_width = 2;
    fx.run();
    let it = fx.m.item(g).unwrap();
    assert_eq!(it.graph.bounds.height, 20.0);
    assert_eq!(it.graph.bounds.y, 12.0 - 10.0 + 0.5);
}

// ----------------------------------------------------------------------------------------
// Vertical bars (bar.md §4.5, item.md §4.6)
// ----------------------------------------------------------------------------------------

#[test]
fn vertical_layout() {
    let mut fx = Fx::new();
    fx.m.bar.position = b'l';
    fx.m.bars[0].frame = r(0.0, 24.0, 25.0, 1056.0);
    let a = fx.item("a", "left", "ab"); // h 15, len 18
    let b = fx.item("b", "left", "a");
    fx.it(b).y_offset = -4;
    fx.it(b).background.padding_left = 2;
    let c = fx.item("c", "right", "a");
    fx.it(c).background.padding_right = 1;
    let d = fx.item("d", "center", "abc");
    fx.it(d).y_offset = 3;
    fx.run();

    // a: cur 20; x = (25 - 18)/2 = 3; window (0, 24+20, 25, 15).
    assert_eq!(fx.frame(a), r(0.0, 44.0, 25.0, 15.0));
    let ai = fx.m.item(a).unwrap();
    assert_eq!(ai.label.bounds.x, 3.0);
    // y = (u32)(15/2) = 7 → baseline (u32)(7 - 4.2) = 2.
    assert_eq!(ai.label.bounds.y, 2.0);
    // b: cur 35 + 2 = 37; y_offset -4 shifts the window up by 4 and grows it.
    assert_eq!(fx.frame(b), r(0.0, 24.0 + 37.0 - 4.0, 25.0, 19.0));
    assert_eq!(fx.m.item(b).unwrap().label.bounds.x, 8.0); // (25-9)/2 = 8
                                                           // c: right from 1056 - 20 = 1036; 1036 - 15 - 1 = 1020.
    assert_eq!(fx.frame(c), r(0.0, 24.0 + 1020.0, 25.0, 15.0));
    // d: centre (1056 - 0 - 15)/2 - 1 = 519.5 → 519; window grows by |y_offset|.
    assert_eq!(fx.frame(d), r(0.0, 24.0 + 519.0, 25.0, 18.0));
}

#[test]
fn vertical_center_subtracts_margin_and_shadow_shift() {
    let mut fx = Fx::new();
    fx.m.bar.position = b'r';
    fx.m.bar.margin = 10;
    fx.m.bars[0].frame = r(1885.0, 24.0, 25.0, 1056.0);
    let d = fx.item("d", "center", "a");
    fx.it(d).label.shadow.set_angle(180);
    fx.it(d).label.shadow.enabled = true; // left extent 5
    fx.run();
    // (1056 - 20 - 15)/2 - 1 = 509.5 → 509; window x = bar x - 5, width = thickness.
    assert_eq!(fx.frame(d), r(1880.0, 24.0 + 509.0, 25.0, 15.0));
    // content x = (25 - 9)/2 + 5 = 13.
    assert_eq!(fx.m.item(d).unwrap().label.bounds.x, 13.0);
}

#[test]
fn vertical_brackets_are_not_laid_out() {
    let mut fx = Fx::new();
    fx.m.bar.position = b'l';
    fx.m.bars[0].frame = r(0.0, 24.0, 25.0, 1056.0);
    let a = fx.item("a", "left", "ab");
    let br = fx.item("br", "left", "");
    fx.it(br).set_type(ItemType::Bracket, "/h");
    fx.it(br).bracket_members = vec![a];
    fx.run();
    // Drawn bracket without a previous frame: a 1×1 window at nirvana.
    assert_eq!(fx.frame(br), r(NIRVANA.x, NIRVANA.y, 1.0, 1.0));
}

// ----------------------------------------------------------------------------------------
// Brackets (item.md §5.2, bar.md §4.6)
// ----------------------------------------------------------------------------------------

fn bracket_fixture() -> (Fx, ItemId, ItemId, ItemId) {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab"); // 20..38
    let b = fx.item("b", "left", "abc");
    fx.it(b).background.padding_left = 5;
    fx.it(b).background.padding_right = 3; // 43..69
    let br = fx.item("br", "left", "");
    fx.it(br).set_type(ItemType::Bracket, "/h");
    fx.it(br).bracket_members = vec![b, a];
    (fx, a, b, br)
}

#[test]
fn bracket_spans_members() {
    let (mut fx, _a, b, br) = bracket_fixture();
    fx.it(br).background.set_height(20);
    fx.it(br).background.enabled = true;
    let lay = fx.run();
    // first = a (x 20, pl 0), last = b (max 69, pr 3): len = 69 + 3 + 0 - 20 = 52.
    assert_eq!(fx.frame(br), r(20.0, 0.0, 52.0, 25.0));
    assert_eq!(
        fx.m.item(br).unwrap().background.bounds,
        r(0.0, 2.0, 52.0, 20.0)
    );
    assert!(lay.bars[0].items.iter().any(|p| p.id == br));
    // Brackets do not take part in the cursor flow.
    assert_eq!(fx.frame(b).x, 43.0);

    // first member with padding_left: the bracket starts at first.x - pl.
    let (mut fx, a, _b, br) = bracket_fixture();
    fx.it(a).background.padding_left = 4;
    fx.run();
    // a: 24..42; b: 47..73 → len = 73 + 3 + 4 - 24 = 56, x = 20.
    assert_eq!(fx.frame(br), r(20.0, 0.0, 56.0, 25.0));
    // Without an explicit height the bracket background is 0 high.
    assert_eq!(fx.m.item(br).unwrap().background.bounds.height, 0.0);
}

#[test]
fn bracket_shadow_extents_and_no_members() {
    let (mut fx, a, b, br) = bracket_fixture();
    fx.it(br).background.enabled = true;
    fx.it(br).background.x_offset = -3; // left extent 3 for the bracket
    fx.run();
    // Window not shifted; widened by the extents; background starts L inside.
    assert_eq!(fx.frame(br), r(20.0, 0.0, 55.0, 25.0));
    assert_eq!(fx.m.item(br).unwrap().background.bounds.x, 3.0);

    // No member drawn → nirvana, size unchanged.
    fx.it(a).drawing = false;
    fx.it(b).drawing = false;
    fx.run();
    assert_eq!(fx.frame(br), r(NIRVANA.x, NIRVANA.y, 55.0, 25.0));
}

// ----------------------------------------------------------------------------------------
// Popups (item.md §6.3–6.4, bar.md §4.7)
// ----------------------------------------------------------------------------------------

fn popup_fixture() -> (Fx, ItemId, ItemId, ItemId) {
    let mut fx = Fx::new();
    let host = fx.item("host", "left", "ab"); // window 20,0,18,25
    let p1 = fx.item("p1", "left", "abc");
    let p2 = fx.item("p2", "left", "a");
    fx.add_popup_member(host, p1);
    fx.add_popup_member(host, p2);
    fx.it(host).popup.drawing = true;
    (fx, host, p1, p2)
}

#[test]
fn popup_vertical_stack() {
    let (mut fx, host, p1, p2) = popup_fixture();
    let lay = fx.run();
    let h = fx.m.item(host).unwrap();
    // cell = host window height 25; anchor (20, 0 + 25), align left minus host pl 0.
    assert_eq!(h.popup.cell_size, 25);
    assert_eq!(h.popup.adid, 1);
    assert_eq!(h.popup.anchor, Point::new(20.0, 25.0));
    assert_eq!(h.popup.frame, Some(r(20.0, 25.0, 26.0, 50.0)));
    assert_eq!(fx.frame(p1), r(20.0, 25.0, 26.0, 25.0));
    assert_eq!(fx.frame(p2), r(20.0, 50.0, 9.0, 25.0));
    // Member content centred in its cell: y = 12 → baseline 7.
    assert_eq!(fx.m.item(p1).unwrap().label.bounds.y, 7.0);

    assert_eq!(lay.popups.len(), 1);
    let pl = &lay.popups[0];
    assert_eq!(pl.host, host);
    assert_eq!(pl.frame, r(20.0, 25.0, 26.0, 50.0));
    assert_eq!(pl.level, level::POPUP_MENU);
    let ids: Vec<ItemId> = pl.items.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![p1, p2]);
    // The members are drawn on the active bar, too (associated_bar), but not painted there.
    assert!(lay.bars[0].items.iter().any(|p| p.id == p1));
}

#[test]
fn popup_border_quirk_paddings_and_y_offset() {
    let (mut fx, host, p1, p2) = popup_fixture();
    fx.it(host).popup.background.border_width = 2;
    fx.it(host).popup.y_offset = 4;
    fx.it(p2).background.padding_left = 6;
    fx.it(p2).background.padding_right = 1;
    fx.run();
    let h = fx.m.item(host).unwrap();
    // y starts at bw; width gets bw once (right), height twice: 2 + 25 + 25 + 2.
    // widths: p1 26, p2 6 + 9 + 1 = 16 → max 26, + 2 = 28.
    assert_eq!(h.popup.frame, Some(r(20.0, 29.0, 28.0, 54.0)));
    assert_eq!(fx.frame(p1), r(20.0, 31.0, 26.0, 25.0));
    assert_eq!(fx.frame(p2), r(26.0, 56.0, 9.0, 25.0));
    assert_eq!(h.popup.background.bounds, r(0.0, 0.0, 28.0, 54.0));
}

#[test]
fn popup_horizontal_row() {
    let (mut fx, host, p1, p2) = popup_fixture();
    fx.it(host).popup.horizontal = true;
    fx.it(host).popup.background.border_width = 1;
    fx.it(p1).background.padding_right = 2;
    fx.run();
    let h = fx.m.item(host).unwrap();
    // total = 26 + 2 + 9 = 37; row_h = 25; width = x + bw = 38; height 1 + 25 + 1.
    assert_eq!(h.popup.frame, Some(r(20.0, 25.0, 38.0, 27.0)));
    assert_eq!(fx.frame(p1), r(20.0, 26.0, 26.0, 25.0));
    assert_eq!(fx.frame(p2), r(48.0, 26.0, 9.0, 25.0));
}

#[test]
fn popup_alignment_cell_size_and_bottom_bar() {
    let (mut fx, host, p1, _p2) = popup_fixture();
    fx.it(host).popup.align = b'c';
    fx.run();
    // (18 - 26)/2 = -4.
    assert_eq!(
        fx.m.item(host).unwrap().popup.anchor,
        Point::new(16.0, 25.0)
    );
    fx.it(host).popup.align = b'r';
    fx.run();
    assert_eq!(
        fx.m.item(host).unwrap().popup.anchor,
        Point::new(12.0, 25.0)
    );
    // align left subtracts the host padding_left.
    fx.it(host).popup.align = b'l';
    fx.it(host).background.padding_left = 7;
    fx.run();
    assert_eq!(
        fx.m.item(host).unwrap().popup.anchor,
        Point::new(20.0, 25.0)
    );

    // Explicit cell size.
    fx.it(host).popup.set_cell_size(40);
    fx.run();
    assert_eq!(fx.m.item(host).unwrap().popup.frame.unwrap().height, 80.0);
    assert_eq!(fx.frame(p1).height, 40.0);

    // Bottom bar: the popup opens above the bar window.
    fx.m.bar.position = b'b';
    fx.m.bars[0].frame = r(0.0, 1055.0, 1920.0, 25.0);
    fx.run();
    assert_eq!(
        fx.m.item(host).unwrap().popup.anchor,
        Point::new(20.0, 1055.0 - 80.0)
    );
}

#[test]
fn popup_on_vertical_bar() {
    let (mut fx, host, p1, _p2) = popup_fixture();
    fx.m.bar.position = b'l';
    fx.m.bars[0].frame = r(0.0, 24.0, 25.0, 1056.0);
    fx.run();
    // host window (0, 44, 25, 15): cell = window width 25; popup to the right.
    let h = fx.m.item(host).unwrap();
    assert_eq!(h.popup.cell_size, 25);
    assert_eq!(h.popup.anchor, Point::new(25.0, 44.0));
    assert_eq!(fx.frame(p1).origin(), Point::new(25.0, 44.0));
    fx.m.bar.position = b'r';
    fx.m.bars[0].frame = r(1895.0, 24.0, 25.0, 1056.0);
    fx.run();
    assert_eq!(
        fx.m.item(host).unwrap().popup.anchor,
        Point::new(1895.0 - 26.0, 44.0)
    );
}

#[test]
fn popup_needs_active_display_and_drawn_host_d3() {
    let (mut fx, host, p1, _p2) = popup_fixture();
    fx.m.active_adid = 2;
    let lay = fx.run();
    assert!(lay.popups.is_empty());
    assert_eq!(fx.m.item(host).unwrap().popup.adid, 0);
    assert_eq!(fx.frame(p1).origin(), NIRVANA);

    fx.m.active_adid = 1;
    let lay = fx.run();
    assert_eq!(lay.popups.len(), 1);

    // D3: host not drawn → the popup and its members disappear.
    fx.it(host).drawing = false;
    let lay = fx.run();
    assert!(lay.popups.is_empty());
    assert_eq!(fx.m.item(host).unwrap().popup.frame, None);
    assert_eq!(fx.frame(p1).origin(), NIRVANA);
    assert_eq!(layout::popup_at_point(&fx.m, Point::new(25.0, 30.0)), None);

    // Empty popup: anchored but no window.
    let mut fx = Fx::new();
    let host = fx.item("host", "left", "ab");
    fx.it(host).popup.drawing = true;
    let lay = fx.run();
    assert!(lay.popups.is_empty());
    assert_eq!(fx.m.item(host).unwrap().popup.adid, 1);
}

#[test]
fn popup_set_anchor_marks_members_and_ordering() {
    let (mut fx, host, p1, _p2) = popup_fixture();
    fx.it(p1).needs_update = false;
    fx.run();
    assert!(fx.m.item(p1).unwrap().needs_update);
    assert!(fx.m.item(host).unwrap().popup.needs_ordering);
    // Unchanged adid: no new marking.
    fx.it(p1).needs_update = false;
    fx.it(host).popup.needs_ordering = false;
    fx.run();
    assert!(!fx.m.item(p1).unwrap().needs_update);
    assert!(!fx.m.item(host).unwrap().popup.needs_ordering);
}

#[test]
fn nested_popup_flyout_same_pass_d4() {
    let (mut fx, host, p1, _p2) = popup_fixture();
    let q = fx.item("q", "left", "ab");
    fx.add_popup_member(p1, q);
    fx.it(p1).popup.drawing = true;
    let lay = fx.run();
    // Parent popup frame (20, 25, 26, 50); p1 window (20, 25, 26, 25).
    // Flyout (vertical parent, align left): x = 20 - 18 = 2; y = 25 - bw 0.
    let p = &fx.m.item(p1).unwrap().popup;
    assert_eq!(p.anchor, Point::new(2.0, 25.0));
    assert_eq!(p.frame, Some(r(2.0, 25.0, 18.0, 25.0)));
    // D4: the nested member is placed against the new anchor in the same pass.
    assert_eq!(fx.frame(q), r(2.0, 25.0, 18.0, 25.0));
    // Parents before nested popups.
    let hosts: Vec<ItemId> = lay.popups.iter().map(|p| p.host).collect();
    assert_eq!(hosts, vec![host, p1]);

    // align right: flyout to the right of the parent window; bw of the parent raises it.
    fx.it(p1).popup.align = b'r';
    fx.it(host).popup.background.border_width = 1;
    fx.run();
    // parent frame now (20, 25, 27, 52); p1 window y = 26.
    assert_eq!(
        fx.m.item(p1).unwrap().popup.anchor,
        Point::new(20.0 + 27.0, 26.0 - 1.0)
    );
    assert_eq!(fx.frame(q).origin(), Point::new(47.0, 25.0));

    // Horizontal parent: nested popups open below the member like bar popups.
    fx.it(host).popup.horizontal = true;
    fx.it(p1).popup.align = b'l';
    fx.run();
    let p1f = fx.frame(p1);
    assert_eq!(
        fx.m.item(p1).unwrap().popup.anchor,
        Point::new(p1f.x, p1f.y + p1f.height)
    );
}

#[test]
fn bracket_in_popup() {
    let (mut fx, host, p1, p2) = popup_fixture();
    let br = fx.item("br", "left", "");
    fx.it(br).set_type(ItemType::Bracket, "/h");
    fx.it(br).bracket_members = vec![p1, p2];
    fx.add_popup_member(host, br);
    fx.it(br).background.set_height(10);
    fx.run();
    // Members p1 (20,25,26,25) and p2 (20,50,9,25): first = p1 (min x, first wins ties),
    // last = p1 (max x+w). len = 46 - 20 = 26.
    assert_eq!(fx.frame(br), r(20.0, 25.0, 26.0, 25.0));
    // cell = max(height(p1) = 15, 25) = 25 → y = 12 for the background.
    assert_eq!(
        fx.m.item(br).unwrap().background.bounds,
        r(0.0, 7.0, 26.0, 10.0)
    );
}

#[test]
fn popup_background_image_sizes_width() {
    let (mut fx, host, p1, _p2) = popup_fixture();
    {
        let bg = &mut fx.it(host).popup.background;
        bg.enabled = true;
        bg.border_width = 1;
        bg.image.enabled = true;
        bg.image.bounds = r(0.0, 0.0, 100.0, 10.0);
    }
    fx.it(host).popup.horizontal = true;
    fx.run();
    let p = &fx.m.item(host).unwrap().popup;
    // width = image 100 + 2*bw; x = (102 - 35)/2 = 33 (u32 division).
    assert_eq!(p.frame.unwrap().width, 102.0);
    assert_eq!(fx.frame(p1).x, 20.0 + 33.0);
    // image at (bw, bw + h/2 - h/2).
    assert_eq!(p.background.image.bounds, r(1.0, 1.0, 100.0, 10.0));
}

// ----------------------------------------------------------------------------------------
// Scenes (bar.md §5.1, components.md §4.9, §5.4–5.6, §6.5, §8.5)
// ----------------------------------------------------------------------------------------

#[test]
fn bar_scene_background_and_items() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab");
    fx.it(a)
        .background
        .set_color(0xff00ff00, &mut Default::default());
    fx.it(a).background.corner_radius = 50;
    fx.it(a).background.border_width = 2;
    fx.m.bar.background.y_offset = 5;
    fx.m.bar.background.x_offset = 3;
    let lay = fx.run();
    let scene = layout::bar_scene(&fx.m, &lay.bars[0]);
    assert_eq!(scene.size, Size::new(1920.0, 25.0));
    // Bar background: y_offset cancelled, x_offset applied, no border.
    match &scene.primitives[0] {
        Primitive::Rect {
            rect,
            color,
            border_width,
            ..
        } => {
            assert_eq!(*rect, r(3.0, 0.0, 1920.0, 25.0));
            assert_eq!(color.hex, 0x44000000);
            assert_eq!(*border_width, 0.0);
        }
        p => panic!("unexpected {p:?}"),
    }
    // Item window clip, item background (radius clamped: inset 16×21 → 8).
    assert_eq!(
        scene.primitives[1],
        Primitive::PushClip {
            rect: r(20.0, 0.0, 18.0, 25.0),
            corner_radius: 0.0
        }
    );
    match &scene.primitives[2] {
        Primitive::Rect {
            rect,
            corner_radius,
            ..
        } => {
            // CG (0, 1, 18, 23) → top-left y = 25 - 24 = 1, shifted by the window x 20.
            assert_eq!(*rect, r(20.0, 1.0, 18.0, 23.0));
            assert_eq!(*corner_radius, 8.0);
        }
        p => panic!("unexpected {p:?}"),
    }
    // Label glyphs: pen (0 + 0 - scroll, baseline 7) → (20, 25 - 7 = 18).
    match &scene.primitives[3] {
        Primitive::Text {
            origin,
            color,
            clip,
            ..
        } => {
            assert_eq!(*origin, Point::new(20.0, 18.0));
            assert_eq!(color.hex, 0xffffffff);
            assert!(clip.is_none());
        }
        p => panic!("unexpected {p:?}"),
    }
    assert_eq!(scene.primitives[4], Primitive::PopClip);
}

#[test]
fn bar_scene_skips_invisible_bar_background_and_clip_holes() {
    let mut fx = Fx::new();
    fx.m.bar.background.color.set_hex(0);
    let a = fx.item("a", "left", "ab");
    fx.it(a)
        .background
        .set_color(0x80ffffff, &mut Default::default());
    fx.it(a).background.clip = 0.5;
    fx.it(a).background.corner_radius = 4;
    fx.m.bar.background.border_width = 2;
    fx.m.bar.background.border_color.set_hex(0x80ff0000);
    let lay = fx.run();
    let scene = layout::bar_scene(&fx.m, &lay.bars[0]);
    // The bar border is now visible (alpha 0x80, width 2): the bar background is drawn.
    assert!(matches!(scene.primitives[0], Primitive::Rect { .. }));
    // bar_height = 25 - 3 = 22, bg auto h = 19, y = 12 - 9 = 3 → top-left y = 25-22 = 3.
    match &scene.primitives[1] {
        Primitive::ClipHole {
            rect,
            corner_radius,
            alpha,
            stroke_width,
            stroke_alpha,
        } => {
            assert_eq!(*rect, r(20.0, 3.0, 18.0, 19.0));
            assert_eq!(*corner_radius, 4.0);
            assert_eq!(*alpha, 0.5);
            assert_eq!(*stroke_width, 2.0);
            assert!((stroke_alpha - 128.0 / 255.0).abs() < 1e-6);
        }
        p => panic!("unexpected {p:?}"),
    }

    // No visible content: no bar background primitive.
    fx.m.bar.background.border_width = 0;
    let scene = layout::bar_scene(&fx.m, &lay.bars[0]);
    assert!(matches!(scene.primitives[0], Primitive::ClipHole { .. }));
}

#[test]
fn bar_scene_brackets_below_items_and_popup_members_excluded() {
    let (mut fx, _a, _b, br) = bracket_fixture();
    fx.it(br)
        .background
        .set_color(0xff0000ff, &mut Default::default());
    fx.it(br).background.set_height(20);
    let lay = fx.run();
    let scene = layout::bar_scene(&fx.m, &lay.bars[0]);
    // bar bg, then the bracket window first (lowest), then a and b.
    let clips: Vec<Rect> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::PushClip { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(
        clips,
        vec![
            r(20.0, 0.0, 52.0, 25.0),
            r(20.0, 0.0, 18.0, 25.0),
            r(43.0, 0.0, 26.0, 25.0)
        ]
    );

    let (mut fx, _host, _p1, _p2) = popup_fixture();
    let lay = fx.run();
    let scene = layout::bar_scene(&fx.m, &lay.bars[0]);
    let n = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::PushClip { .. }))
        .count();
    assert_eq!(n, 1, "only the host is painted into the bar");
}

#[test]
fn popup_scene_background_without_shadow() {
    let (mut fx, host, _p1, _p2) = popup_fixture();
    {
        let bg = &mut fx.it(host).popup.background;
        bg.enabled = true;
        bg.shadow.enabled = true;
    }
    let lay = fx.run();
    let scene = layout::popup_scene(&fx.m, &lay.popups[0]);
    assert_eq!(scene.size, Size::new(26.0, 50.0));
    assert!(!scene
        .primitives
        .iter()
        .any(|p| matches!(p, Primitive::Shadow { .. })));
    match &scene.primitives[0] {
        Primitive::Rect { rect, color, .. } => {
            assert_eq!(*rect, r(0.0, 0.0, 26.0, 50.0));
            assert_eq!(color.hex, 0x44000000);
        }
        p => panic!("unexpected {p:?}"),
    }
    // Members at popup-local positions.
    assert_eq!(
        scene.primitives[1],
        Primitive::PushClip {
            rect: r(0.0, 0.0, 26.0, 25.0),
            corner_radius: 0.0
        }
    );
}

#[test]
fn item_scene_text_shadow_scroll_highlight_and_clip() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "abcd");
    {
        let it = fx.it(a);
        it.label.max_chars = 2;
        it.label.width = 18;
        it.label.scroll = 3.0;
        it.label.padding_left = 2;
        it.label.y_offset = 1;
        it.label.highlight = true;
        it.label.shadow.enabled = true;
    }
    fx.run();
    let mut scene = mbar_core::scene::Scene::default();
    let item = fx.m.item(a).unwrap().clone();
    layout::item_scene(&item, Point::new(100.0, 0.0), 25.0, None, &mut scene);
    let clip = Some(r(102.0, 25.0 - 9999.0, 18.0, 19998.0));
    let off = item.label.shadow.offset;
    // Shadow: no scroll; baseline 7 + offset.y + y_offset.
    assert_eq!(
        scene.primitives[0],
        Primitive::Text {
            origin: Point::new(100.0 + off.x + 2.0, 25.0 - (7.0 + off.y + 1.0)),
            key: item.label.line.unwrap(),
            color: item.label.shadow.color,
            clip,
        }
    );
    assert_eq!(
        scene.primitives[1],
        Primitive::Text {
            origin: Point::new(100.0 + 2.0 - 3.0, 25.0 - 8.0),
            key: item.label.line.unwrap(),
            color: item.label.highlight_color,
            clip,
        }
    );
}

#[test]
fn item_scene_graph_exact_path() {
    let mut fx = Fx::new();
    let g = fx.item("g", "left", "");
    {
        let it = fx.it(g);
        it.set_type(ItemType::Graph, "/h");
        it.label.drawing = false;
        it.graph.setup(4);
        for v in [0.25, 0.5, 0.75, 1.0] {
            it.graph.push(v);
        }
    }
    fx.run();
    let item = fx.m.item(g).unwrap().clone();
    let mut scene = mbar_core::scene::Scene::default();
    layout::item_scene(&item, Point::new(0.0, 0.0), 25.0, None, &mut scene);
    // x0 0, y 1, h 23; LTR: (0,Y0), (0,Y3), (1,Y2), (2,Y1); fill closes (2,1), (0,1).
    let p = |x: f32, v: f32| Point::new(x, 25.0 - (1.0 + v * 23.0));
    match &scene.primitives[0] {
        Primitive::Graph {
            line,
            fill,
            fill_color,
            line_width,
            ..
        } => {
            assert_eq!(
                *line,
                vec![p(0.0, 0.25), p(0.0, 1.0), p(1.0, 0.75), p(2.0, 0.5)]
            );
            assert_eq!(&fill[..4], &line[..]);
            assert_eq!(&fill[4..], &[p(2.0, 0.0), p(0.0, 0.0)]);
            assert_eq!(fill_color.hex, 0x33cccccc); // line colour, alpha * 0.2
            assert_eq!(*line_width, 0.5);
        }
        p => panic!("unexpected {p:?}"),
    }

    // RTL (right position): x starts at x0 + width.
    fx.it(g).set_position("right");
    fx.run();
    let item = fx.m.item(g).unwrap().clone();
    let mut scene = mbar_core::scene::Scene::default();
    layout::item_scene(&item, Point::new(0.0, 0.0), 25.0, None, &mut scene);
    match &scene.primitives[0] {
        Primitive::Graph { line, fill, .. } => {
            assert_eq!(
                *line,
                vec![p(4.0, 1.0), p(4.0, 1.0), p(3.0, 0.75), p(2.0, 0.5)]
            );
            assert_eq!(&fill[4..], &[p(2.0, 0.0), p(4.0, 0.0)]);
        }
        p => panic!("unexpected {p:?}"),
    }
}

#[test]
fn item_scene_slider_alias_image_and_blur() {
    let mut fx = Fx::new();
    let s = fx.item("s", "left", "");
    {
        let it = fx.it(s);
        it.set_type(ItemType::Slider, "/h");
        it.label.drawing = false;
        it.slider.setup(100);
        it.slider.track.set_height(10);
        it.slider.percentage = 30;
        it.blur_radius = 7;
    }
    fx.run();
    let item = fx.m.item(s).unwrap().clone();
    let mut scene = mbar_core::scene::Scene::default();
    layout::item_scene(&item, Point::new(0.0, 0.0), 25.0, None, &mut scene);
    assert_eq!(
        scene.primitives[0],
        Primitive::BlurRegion {
            rect: r(0.0, 0.0, 100.0, 25.0),
            corner_radius: 0.0,
            radius: 7
        }
    );
    let rects: Vec<Rect> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Rect { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    // Track then fill (30 % of 100), y_top = 25 - 17 = 8.
    assert_eq!(
        rects,
        vec![r(0.0, 8.0, 100.0, 10.0), r(0.0, 8.0, 30.0, 10.0)]
    );

    // Alias with tint mask and a rounded image.
    let mut fx = Fx::new();
    let al = fx.item("al", "left", "");
    {
        let it = fx.it(al);
        it.set_type(ItemType::Alias, "/h");
        it.label.drawing = false;
        it.alias.image.set_image(
            Some(ImageInfo {
                key: ImageKey(9),
                size: Size::new(20.0, 16.0),
                hash: 1,
            }),
            true,
        );
        it.alias.color_override = true;
    }
    fx.run();
    let item = fx.m.item(al).unwrap().clone();
    assert_eq!(item.alias.image.bounds, r(0.0, 4.0, 20.0, 16.0));
    let mut scene = mbar_core::scene::Scene::default();
    layout::item_scene(&item, Point::new(0.0, 0.0), 25.0, None, &mut scene);
    assert_eq!(
        scene.primitives,
        vec![
            Primitive::Image {
                rect: r(0.0, 5.0, 20.0, 16.0),
                key: ImageKey(9),
                corner_radius: 0.0,
                rounded: true,
                border_width: 0.0,
                border_color: item.alias.image.border_color,
            },
            Primitive::ImageMask {
                rect: r(0.0, 5.0, 20.0, 16.0),
                key: ImageKey(9),
                color: item.alias.color,
            }
        ]
    );
}

#[test]
fn item_scene_background_shadow_and_media_artwork() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab");
    {
        let it = fx.it(a);
        it.label.drawing = false;
        it.background.enabled = true;
        it.background.shadow.enabled = true; // offset (4.33, -2.5)
        it.background.image.link = true;
        it.background.image.enabled = true;
    }
    fx.m.current_artwork = Some(ImageInfo {
        key: ImageKey(77),
        size: Size::new(64.0, 64.0),
        hash: 3,
    });
    let lay = fx.run();
    let item = fx.m.item(a).unwrap();
    // 32 pt artwork: the item grows to the image width.
    assert_eq!(item.background.image.bounds.width, 32.0);
    let scene = layout::bar_scene(&fx.m, &lay.bars[0]);
    let shadow = scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::Shadow { rect, .. } => Some(*rect),
            _ => None,
        })
        .unwrap();
    let bg = item.background.bounds;
    let off = item.background.shadow.offset;
    let wx = fx.frame(a).x;
    assert_eq!(
        shadow,
        r(
            wx + bg.x + off.x,
            25.0 - (bg.y + off.y + bg.height),
            bg.width,
            bg.height
        )
    );
    assert!(scene.primitives.iter().any(|p| matches!(
        p,
        Primitive::Image {
            key: ImageKey(77),
            ..
        }
    )));
}

// ----------------------------------------------------------------------------------------
// Hit testing (item.md §9.2–9.3, D2)
// ----------------------------------------------------------------------------------------

#[test]
fn hit_testing_bar_items_half_open_and_inclusive() {
    let (mut fx, a, b, br) = bracket_fixture();
    let lay = fx.run();
    let at = |fx: &Fx, x: f32, y: f32| layout::window_at(&fx.m, &lay, Point::new(x, y));
    assert_eq!(at(&fx, 25.0, 10.0), WindowHit::Item(a));
    // Item windows are above the bracket window; between members the bracket wins.
    assert_eq!(at(&fx, 40.0, 10.0), WindowHit::Item(br));
    assert_eq!(at(&fx, 50.0, 10.0), WindowHit::Item(b));
    // Half-open: x = 38 is outside a (and inside the bracket).
    assert_eq!(at(&fx, 38.0, 10.0), WindowHit::Item(br));
    assert_eq!(at(&fx, 10.0, 10.0), WindowHit::Bar(1));
    assert_eq!(at(&fx, 10.0, 25.0), WindowHit::None);
    // get_item_by_point is inclusive and global-order first.
    assert_eq!(
        layout::item_at_point(&fx.m, Point::new(38.0, 25.0)),
        Some(a)
    );
    // drawing=off items are never hit.
    fx.it(a).drawing = false;
    assert_eq!(at(&fx, 25.0, 10.0), WindowHit::Item(br));

    assert_eq!(layout::bar_at_point(&fx.m, Point::new(0.0, 0.0)), Some(1));
    assert_eq!(
        layout::bar_at_point(&fx.m, Point::new(1919.9, 24.9)),
        Some(1)
    );
    assert_eq!(layout::bar_at_point(&fx.m, Point::new(1920.0, 10.0)), None);
    assert_eq!(layout::bar_at_point(&fx.m, Point::new(10.0, 25.0)), None);
    fx.m.bars[0].hidden = true;
    assert_eq!(layout::bar_at_point(&fx.m, Point::new(10.0, 10.0)), None);
}

#[test]
fn hit_testing_later_items_above_earlier() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "abc"); // 20..46
    let b = fx.item("b", "left", "a");
    fx.it(b).background.padding_left = -10; // 36..45
    let lay = fx.run();
    assert_eq!(fx.frame(b).x, 36.0);
    let p = Point::new(40.0, 5.0);
    assert_eq!(layout::window_at(&fx.m, &lay, p), WindowHit::Item(b));
    assert_eq!(layout::item_at_point(&fx.m, p), Some(a));
}

#[test]
fn hit_testing_popups_and_levels() {
    let (mut fx, host, p1, p2) = popup_fixture();
    fx.it(host).popup.y_offset = -10; // popup frame (20, 15, 26, 50) overlaps the bar
    let lay = fx.run();
    let at = |fx: &Fx, lay: &layout::Layout, x: f32, y: f32| {
        layout::window_at(&fx.m, lay, Point::new(x, y))
    };
    // Popup (level 101) above the bar item.
    assert_eq!(at(&fx, &lay, 30.0, 20.0), WindowHit::Item(p1));
    assert_eq!(at(&fx, &lay, 40.0, 50.0), WindowHit::Popup(host));
    assert_eq!(at(&fx, &lay, 25.0, 50.0), WindowHit::Item(p2));
    assert_eq!(
        layout::popup_at_point(&fx.m, Point::new(40.0, 50.0)),
        Some(host)
    );
    assert_eq!(layout::popup_at_point(&fx.m, Point::new(46.0, 50.0)), None);

    // popup.topmost=off: backstop+1 (-19) is still above the default bar level (-20)…
    fx.it(host).popup.topmost = false;
    let lay = fx.run();
    assert_eq!(lay.popups[0].level, level::BACKSTOP_MENU + 1);
    assert_eq!(at(&fx, &lay, 30.0, 20.0), WindowHit::Item(p1));
    // …but below a `topmost=on` bar (status level 25).
    fx.m.bar.window_level = level::STATUS;
    assert_eq!(at(&fx, &lay, 30.0, 20.0), WindowHit::Item(host));
}

#[test]
fn local_points_and_slider_track_d2() {
    let mut fx = Fx::new();
    let s = fx.item("s", "left", "");
    {
        let it = fx.it(s);
        it.set_type(ItemType::Slider, "/h");
        it.label.drawing = false;
        it.slider.setup(100);
        it.slider.track.set_height(10);
        it.y_offset = 4; // track (0, 11, 100, 10) in y-up space
    }
    fx.run();
    let item = fx.m.item(s).unwrap();
    assert_eq!(item.slider.track.bounds, r(0.0, 11.0, 100.0, 10.0));
    // Screen (70, 6): local y-up = 25 - 6 = 19 → inside (C's y-down test would miss).
    let local = layout::item_local_point(item, 1, Point::new(70.0, 6.0)).unwrap();
    assert_eq!(local, Point::new(50.0, 19.0));
    assert!(layout::slider_track_contains(item, local));
    // Half-open edges.
    assert!(layout::slider_track_contains(item, Point::new(0.0, 11.0)));
    assert!(!layout::slider_track_contains(
        item,
        Point::new(100.0, 15.0)
    ));
    assert!(!layout::slider_track_contains(item, Point::new(50.0, 21.0)));
    assert!(layout::item_local_point(item, 2, Point::new(0.0, 0.0)).is_none());
    assert_eq!(item.slider.percentage_for_point(local), 50);
}

// ----------------------------------------------------------------------------------------
// app_menu (extension)
// ----------------------------------------------------------------------------------------

#[test]
fn app_menu_titles_layout_draw_and_hit() {
    let mut fx = Fx::new();
    let am = fx.item("menu", "left", "");
    {
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
        it.app_menu.hovered = Some(2);
    }
    let lay = fx.run();
    let item = fx.m.item(am).unwrap();
    // Cells: typographic width + 0.5 truncated + spacing 14: Finder 50+14, File 34+14,
    // Edit 34+14 → 160 wide.
    let widths: Vec<f32> = item.app_menu.title_bounds.iter().map(|r| r.width).collect();
    assert_eq!(widths, vec![64.0, 48.0, 48.0]);
    assert_eq!(fx.frame(am), r(20.0, 0.0, 160.0, 25.0));
    let xs: Vec<f32> = item.app_menu.title_bounds.iter().map(|r| r.x).collect();
    assert_eq!(xs, vec![0.0, 64.0, 112.0]);

    // Clicks resolve to indices into `titles`.
    assert_eq!(
        layout::app_menu_title_at(item, Point::new(10.0, 12.0)),
        Some(1)
    );
    assert_eq!(
        layout::app_menu_title_at(item, Point::new(64.0, 12.0)),
        Some(2)
    );
    assert_eq!(
        layout::app_menu_title_at(item, Point::new(170.0, 12.0)),
        None
    );

    let scene = layout::bar_scene(&fx.m, &lay.bars[0]);
    let texts = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Text { .. }))
        .count();
    assert_eq!(texts, 3);
    // The hovered title (index 2) gets the highlight cell.
    assert!(scene.primitives.iter().any(|p| matches!(
        p,
        Primitive::Rect { rect, color, .. } if rect.x == 20.0 + 64.0 && color.hex == 0x33ffffff
    )));
}

// ----------------------------------------------------------------------------------------
// Robustness and the remaining public entry points
// ----------------------------------------------------------------------------------------

#[test]
fn popup_cycle_does_not_recurse_forever() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab");
    let b = fx.item("b", "left", "a");
    let c = fx.item("c", "left", "a");
    fx.add_popup_member(a, b);
    fx.add_popup_member(b, c);
    fx.add_popup_member(c, b); // b <-> c cycle
    fx.it(a).popup.drawing = true;
    fx.it(b).popup.drawing = true;
    fx.it(c).popup.drawing = true;
    let lay = fx.run();
    assert!(lay.popups.iter().any(|p| p.host == a));
}

#[test]
fn public_helpers_popup_bounds_bracket_bounds_side_length() {
    let (mut fx, host, p1, _p2) = popup_fixture();
    // Not anchored yet: computes the size only (default cell size 30).
    assert!(layout::popup_bounds(&mut fx.m, host, &mut fx.res).is_none());
    assert_eq!(
        fx.m.item(host).unwrap().popup.background.bounds,
        r(0.0, 0.0, 26.0, 60.0)
    );
    fx.run();
    let pl = layout::popup_bounds(&mut fx.m, host, &mut fx.res).unwrap();
    assert_eq!(pl.frame, r(20.0, 25.0, 26.0, 50.0));
    assert_eq!(pl.items[0].id, p1);

    // anchor_popup on a non-active bar does nothing.
    let mut other = fx.m.bars[0].clone();
    other.adid = 2;
    let before = fx.m.item(host).unwrap().popup.clone();
    layout::anchor_popup(&mut fx.m, host, &other, &mut fx.res);
    assert_eq!(fx.m.item(host).unwrap().popup, before);

    let (mut fx, _a, _b, br) = bracket_fixture();
    fx.run();
    let bar = fx.m.bars[0].clone();
    assert_eq!(
        layout::bracket_bounds(&mut fx.m, br, &bar, 12),
        r(20.0, 0.0, 52.0, 25.0)
    );
    // Vertical side lengths use heights: 15 + 15 + (5 + 3) for the left items.
    assert_eq!(layout::side_length(&fx.m, &bar, Position::Left, true), 38);
    assert_eq!(
        layout::side_length(&fx.m, &bar, Position::Left, false),
        18 + 26 + 8
    );
}

#[test]
fn item_calculate_bounds_returns_slot_length() {
    let mut fx = Fx::new();
    let a = fx.item("a", "left", "ab");
    fx.it(a).set_width(40);
    fx.it(a).background.enabled = true;
    let it = fx.m.item_mut(a).unwrap();
    assert_eq!(layout::item_calculate_bounds(it, 30, 2, 15, 0), 40);
    // Background spans the slot from the unaligned x; auto height 30 - 1.
    assert_eq!(it.background.bounds, r(2.0, 1.0, 40.0, 29.0));
}
