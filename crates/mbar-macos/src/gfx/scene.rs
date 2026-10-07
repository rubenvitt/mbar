//! Renderer-facing display list.
//!
//! This is the crate-local drawing vocabulary of the Metal renderer. The integration
//! layer (`platform`) maps `mbar_core::scene::Primitive`s onto [`DrawCmd`]s. All
//! coordinates are **logical points** relative to the window's content, origin
//! **top-left**, y growing downward. Colours are **premultiplied** RGBA in `[0, 1]`
//! (see [`premultiply_argb`]).
//!
//! The list is designed to be reused across frames: call [`DrawList::clear`] and refill
//! it; the backing `Vec`s keep their capacity so steady-state frames do not allocate.
//!
//! This module is pure Rust (no Apple types).

use std::ops::Range;

/// A point in logical points (top-left origin).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }
}

/// An axis-aligned rectangle in logical points (top-left origin).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect::new(0.0, 0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    pub fn max_x(&self) -> f32 {
        self.x + self.width
    }

    pub fn max_y(&self) -> f32 {
        self.y + self.height
    }

    /// `true` when the rect has no area (or a NaN size).
    pub fn is_empty(&self) -> bool {
        // Written so that NaN sizes count as empty.
        !(self.width > 0.0 && self.height > 0.0)
    }

    /// Intersection; an empty intersection yields a zero-sized rect at the clamped origin.
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.max_x().min(other.max_x());
        let y1 = self.max_y().min(other.max_y());
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    /// Translate by `(dx, dy)`.
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// `CGRectInset`: shrink each side by `dx` / `dy` (negative grows).
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            self.width - 2.0 * dx,
            self.height - 2.0 * dy,
        )
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.max_x() && p.y >= self.y && p.y < self.max_y()
    }
}

/// Handle of a laid-out text run owned by [`crate::gfx::text::TextSystem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextRunId(pub u64);

/// Handle of an image owned by [`crate::gfx::image::ImageStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(pub u32);

/// Premultiplied RGBA colour, components in `[0, 1]`.
pub type Rgba = [f32; 4];

/// Fully transparent colour.
pub const TRANSPARENT: Rgba = [0.0; 4];

/// `0xAARRGGBB` → premultiplied RGBA (`color.c:color_set_hex` channel extraction).
pub fn premultiply_argb(argb: u32) -> Rgba {
    let a = ((argb >> 24) & 0xff) as f32 / 255.0;
    let r = ((argb >> 16) & 0xff) as f32 / 255.0;
    let g = ((argb >> 8) & 0xff) as f32 / 255.0;
    let b = (argb & 0xff) as f32 / 255.0;
    [r * a, g * a, b * a, a]
}

/// Straight (non-premultiplied) float channels → premultiplied RGBA. Channels are
/// clamped to `[0, 1]` like `color.c`'s float setters.
pub fn premultiply(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    let c = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    let a = c(a);
    [c(r) * a, c(g) * a, c(b) * a, a]
}

/// One drawing command. Commands are painted in list order (painter's algorithm).
#[derive(Debug, Clone, PartialEq)]
pub enum DrawCmd {
    /// `background.c:draw_rect` semantics: the region is inset by `border_width / 2`, the
    /// corner radius is clamped to half the inset size (truncated), the inset rounded rect
    /// is filled and then stroked with `border_width` centred on its edge (so the border
    /// lies entirely inside `rect`). `border_width == 0` draws no stroke. Hard-edged
    /// shadows are expressed as an extra `RoundedRect` with the shadow colour as both
    /// fill and border, translated by the shadow offset.
    RoundedRect {
        rect: Rect,
        fill: Rgba,
        corner_radius: f32,
        border_width: f32,
        border_color: Rgba,
    },
    /// A text run drawn with its pen (baseline-left) at `origin`. `color` tints mask runs;
    /// for runs containing colour glyphs (emoji) it is the colour of the non-emoji glyphs.
    /// `clip` is an additional axis-aligned clip (`max_chars` truncation).
    Text {
        origin: Point,
        run: TextRunId,
        color: Rgba,
        clip: Option<Rect>,
    },
    /// `image.c:image_draw` semantics: stretched to `rect`. When
    /// `rect.width > 2r && rect.height > 2r` the image is clipped to the rounded rect and
    /// a border of `border_width` is drawn inside the edge; otherwise neither clip nor
    /// border. `tint` (premultiplied, alpha 0 = none) paints a colour through the image
    /// alpha on top of the image (alias `color` override). `nearest` selects
    /// nearest-neighbour sampling (SketchyBar's `kCGInterpolationNone`).
    Image {
        rect: Rect,
        image: ImageId,
        corner_radius: f32,
        border_width: f32,
        border_color: Rgba,
        tint: Rgba,
        nearest: bool,
    },
    /// Graph polyline. `points` indexes [`DrawList::points`]; the polyline must be
    /// x-monotone (graph samples). The line is stroked with `line_width`, then (if `fill`)
    /// the area between the polyline and the horizontal line `y = baseline` is filled on
    /// top of it (`graph.c:graph_draw` order).
    Path {
        points: Range<u32>,
        baseline: f32,
        line_color: Rgba,
        fill_color: Rgba,
        line_width: f32,
        fill: bool,
    },
    /// Push a clip. Clips nest by intersection. `corner_radius > 0` clips to a rounded
    /// rect (the innermost rounded clip is applied exactly; outer rounded clips act as
    /// their bounding rects).
    PushClip { rect: Rect, corner_radius: f32 },
    /// Pop the most recent clip. Unbalanced pops are ignored.
    PopClip,
}

/// A window's display list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawList {
    pub items: Vec<DrawCmd>,
    /// Shared vertex pool for [`DrawCmd::Path`].
    pub points: Vec<Point>,
}

impl DrawList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty the list, keeping capacity.
    pub fn clear(&mut self) {
        self.items.clear();
        self.points.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn push(&mut self, cmd: DrawCmd) {
        self.items.push(cmd);
    }

    /// Append a graph path, copying `points` into the shared pool.
    #[allow(clippy::too_many_arguments)]
    pub fn push_path<I: IntoIterator<Item = Point>>(
        &mut self,
        points: I,
        baseline: f32,
        line_color: Rgba,
        fill_color: Rgba,
        line_width: f32,
        fill: bool,
    ) {
        let start = self.points.len() as u32;
        self.points.extend(points);
        let end = self.points.len() as u32;
        self.items.push(DrawCmd::Path {
            points: start..end,
            baseline,
            line_color,
            fill_color,
            line_width,
            fill,
        });
    }

    /// The points of a path command (empty for an out-of-range range).
    pub fn path_points(&self, range: &Range<u32>) -> &[Point] {
        self.points
            .get(range.start as usize..range.end as usize)
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply_hex() {
        assert_eq!(premultiply_argb(0xffffffff), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(premultiply_argb(0x00ff0000), [0.0, 0.0, 0.0, 0.0]);
        let c = premultiply_argb(0x80ff0000);
        let a = 128.0 / 255.0;
        assert!((c[0] - a).abs() < 1e-6 && c[1] == 0.0 && (c[3] - a).abs() < 1e-6);
    }

    #[test]
    fn premultiply_floats_clamp() {
        assert_eq!(premultiply(2.0, -1.0, 0.5, 0.5), [0.5, 0.0, 0.25, 0.5]);
        assert_eq!(premultiply(f32::NAN, 1.0, 1.0, 1.0)[0], 0.0);
    }

    #[test]
    fn rect_ops() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), Rect::new(5.0, 5.0, 5.0, 5.0));
        let c = Rect::new(20.0, 20.0, 1.0, 1.0);
        assert!(a.intersect(&c).is_empty());
        assert_eq!(a.inset(1.0, 2.0), Rect::new(1.0, 2.0, 8.0, 6.0));
        assert!(a.contains(Point::new(0.0, 9.9)));
        assert!(!a.contains(Point::new(10.0, 0.0)));
        assert!(Rect::new(0.0, 0.0, f32::NAN, 1.0).is_empty());
    }

    #[test]
    fn path_pool() {
        let mut l = DrawList::new();
        l.push_path(
            [Point::new(0.0, 0.0), Point::new(1.0, 1.0)],
            5.0,
            TRANSPARENT,
            TRANSPARENT,
            1.0,
            true,
        );
        l.push_path([Point::new(2.0, 2.0)], 5.0, TRANSPARENT, TRANSPARENT, 1.0, false);
        match &l.items[1] {
            DrawCmd::Path { points, .. } => {
                assert_eq!(l.path_points(points), &[Point::new(2.0, 2.0)]);
            }
            _ => unreachable!(),
        }
        let cap = l.points.capacity();
        l.clear();
        assert!(l.is_empty() && l.points.capacity() == cap);
    }
}
