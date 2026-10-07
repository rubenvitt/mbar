//! Display lists consumed by renderers (`docs/DESIGN-CORE.md` "Scene").
//!
//! Coordinates are points with the origin at the **top-left of the window**; renderers
//! multiply by the backing scale. Primitives are painted in order (painter's algorithm).
//! `layout.rs` (WP-A) produces scenes; the macOS renderer and tests consume them.

use crate::color::Color;
use crate::geometry::{Point, Rect, Size};
use crate::platform::{ImageKey, TextKey};

/// One drawing operation.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    /// `draw_rect` (`components.md` §5.4): the stroke is centred on `rect` inset by
    /// `border_width/2` (so it lies inside `rect`); the fill covers that inset rect; the
    /// radius is already clamped by layout. `border_width == 0` draws no stroke (Q1).
    Rect {
        rect: Rect,
        color: Color,
        corner_radius: f32,
        border_width: f32,
        border_color: Color,
    },
    /// Solid silhouette of a shape translated by the shadow offset (no blur). Background
    /// shadows also stroke with the shadow colour (`border_width`).
    Shadow {
        rect: Rect,
        color: Color,
        corner_radius: f32,
        border_width: f32,
    },
    /// A text line; `origin` is the pen position on the baseline (already including
    /// paddings, `y_offset` and `-scroll`); glyph colour comes from `color`.
    Text {
        origin: Point,
        key: TextKey,
        color: Color,
        clip: Option<Rect>,
    },
    /// A picture stretched to `rect` (nearest-neighbour). When `rounded`, it is clipped to the
    /// rounded rect and a border of `border_width` is drawn inside (`image_draw`, §6.5).
    Image {
        rect: Rect,
        key: ImageKey,
        corner_radius: f32,
        rounded: bool,
        border_width: f32,
        border_color: Color,
    },
    /// Fill `color` through the picture's alpha (alias tint, `alias_draw`, Q4).
    ImageMask {
        rect: Rect,
        key: ImageKey,
        color: Color,
    },
    /// A graph (`graph_draw`, §8.5): `line` is the exact stroked polyline, `fill` the closed
    /// polygon painted after (on top of) the stroke.
    Graph {
        line: Vec<Point>,
        fill: Vec<Point>,
        line_color: Color,
        fill_color: Color,
        line_width: f32,
    },
    /// Destination-out punch of the bar background (`background.clip`, §5.6):
    /// `dst *= 1 - alpha` inside the rounded rect; `stroke_width`/`stroke_alpha` reproduce the
    /// inherited bar stroke (Q11).
    ClipHole {
        rect: Rect,
        corner_radius: f32,
        alpha: f32,
        stroke_width: f32,
        stroke_alpha: f32,
    },
    /// Intersect the clip with a (rounded) rect until the matching `PopClip`.
    PushClip {
        rect: Rect,
        corner_radius: f32,
    },
    PopClip,
    /// Region behind which the platform realises a background blur (`blur_radius` of items;
    /// a child window behind the bar window).
    BlurRegion {
        rect: Rect,
        corner_radius: f32,
        radius: u32,
    },
}

/// The content of one window.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scene {
    pub size: Size,
    pub primitives: Vec<Primitive>,
}

impl Scene {
    pub fn new(size: Size) -> Self {
        Scene {
            size,
            primitives: Vec::new(),
        }
    }

    pub fn push(&mut self, p: Primitive) {
        self.primitives.push(p);
    }
}
