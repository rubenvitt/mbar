//! Border window geometry and the drawing plan (`docs/spec/borders.md` §5.2, §7.2–§7.5,
//! JankyBorders `src/border.c`, `src/misc/drawing.h`, `src/misc/window.h`).
//!
//! Pure math, so it is tested on every host: the macOS layer
//! (`crates/mbar-macos/src/sys/borders/`) only turns a [`BorderGeometry`] into SkyLight
//! calls and a [`DrawPlan`] into CoreGraphics calls. Numbers follow the C code including
//! its `float`/`double` mix: where JankyBorders computes in `float` the value is rounded
//! to `f32` here too (`BORDER_TSMN`, the border offset of `border_calculate_bounds`, the
//! square inset `-w / 2.f`, color components).

use super::{BorderColor, BorderOrder, BorderSettings, BorderStyle, GradientDirection};

/// `BORDER_PADDING`: extra outset beyond the border width (room for the glow).
pub const BORDER_PADDING: f64 = 8.0;
/// `BORDER_TSMN`: inset of the "truly square" mode (a C `float`).
pub const BORDER_TSMN: f32 = 3.27;
/// Corner radius used when SkyLight reports none (`windows.c:75`).
pub const DEFAULT_CORNER_RADIUS: f64 = 9.0;
/// Fixed corner radius of `style=uniform` (`border.c:118`).
pub const UNIFORM_CORNER_RADIUS: f64 = 9.0;
/// Blur of the `glow(…)` shadow (`drawing.h:37`).
pub const GLOW_BLUR: f64 = 10.0;

/// `WINDOW_TAG_DOCUMENT`.
pub const WINDOW_TAG_DOCUMENT: u64 = 1 << 0;
/// `WINDOW_TAG_FLOATING` (also set on border windows).
pub const WINDOW_TAG_FLOATING: u64 = 1 << 1;
/// `WINDOW_TAG_ATTACHED`.
pub const WINDOW_TAG_ATTACHED: u64 = 1 << 7;
/// Unnamed tag set on every border window (`border.c:253`).
pub const WINDOW_TAG_BORDER_9: u64 = 1 << 9;
/// `WINDOW_TAG_STICKY`.
pub const WINDOW_TAG_STICKY: u64 = 1 << 11;
/// `WINDOW_TAG_IGNORES_CYCLE`.
pub const WINDOW_TAG_IGNORES_CYCLE: u64 = 1 << 18;
/// `WINDOW_TAG_MODAL`.
pub const WINDOW_TAG_MODAL: u64 = 1 << 31;
/// Unnamed tag cleared on sticky border windows (`border.c:251`).
pub const WINDOW_TAG_BORDER_45: u64 = 1 << 45;
/// Unnamed tag accepted instead of attribute `0x2` (`window.h:19`).
pub const WINDOW_TAG_58: u64 = 1 << 58;

/// `BORDER_TSMW`: the minimum border width of the truly-square mode. JankyBorders picks it
/// from the **build SDK** (BQ23); mbar decides at runtime (spec §14 open question 5).
pub fn border_tsmw(macos26: bool) -> f32 {
    if macos26 {
        52.0
    } else {
        8.0
    }
}

/// A point in CoreGraphics coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BorderPoint {
    pub x: f64,
    pub y: f64,
}

impl BorderPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A `CGRect` (global coordinates: origin at the top-left of the main display, y down;
/// border-local rects have their origin at 0,0).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BorderRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BorderRect {
    /// `CGRectNull`.
    pub const NULL: BorderRect = BorderRect {
        x: f64::INFINITY,
        y: f64::INFINITY,
        width: 0.0,
        height: 0.0,
    };

    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether this is [`BorderRect::NULL`] (`CGRectIsNull`).
    pub fn is_null(&self) -> bool {
        self.x.is_infinite() || self.y.is_infinite()
    }

    /// `CGRectInset`: standardizes, moves every edge inwards by `dx`/`dy` (outwards when
    /// negative) and returns [`BorderRect::NULL`] when the result has a negative size.
    pub fn inset(self, dx: f64, dy: f64) -> BorderRect {
        if self.is_null() {
            return self;
        }
        let r = self.standardized();
        let out = BorderRect::new(r.x + dx, r.y + dy, r.width - 2.0 * dx, r.height - 2.0 * dy);
        if out.width < 0.0 || out.height < 0.0 {
            BorderRect::NULL
        } else {
            out
        }
    }

    /// `CGRectStandardize`: non-negative width and height.
    pub fn standardized(self) -> BorderRect {
        let mut r = self;
        if r.width < 0.0 {
            r.x += r.width;
            r.width = -r.width;
        }
        if r.height < 0.0 {
            r.y += r.height;
            r.height = -r.height;
        }
        r
    }
}

/// `SLSSetWindowResolution` argument: the border window's backing scale. hidpi only
/// changes the bitmap resolution, never the geometry in points (BR-DRW-05).
pub fn backing_resolution(hidpi: bool) -> f64 {
    if hidpi {
        2.0
    } else {
        1.0
    }
}

/// `BORDER_ORDER_ABOVE` / `BORDER_ORDER_BELOW`: the `SLSTransactionOrderWindow` order.
pub fn order_value(order: BorderOrder) -> i32 {
    match order {
        BorderOrder::Above => 1,
        BorderOrder::Below => -1,
    }
}

/// The corner radius of a tracked window (`windows.c:67-78`): element 0 of
/// `SLSWindowIteratorGetCornerRadii` (macOS 26+), or [`DEFAULT_CORNER_RADIUS`] when that is
/// missing or not positive.
pub fn corner_radius(reported: Option<i32>) -> f64 {
    match reported {
        Some(r) if r > 0 => r as f64,
        _ => DEFAULT_CORNER_RADIUS,
    }
}

/// `inner_radius = radius + 1` (`windows.c:78`).
pub fn inner_radius(radius: f64) -> f64 {
    radius + 1.0
}

/// `window_suitable` (BR-WIN-01): the windows that get a border.
pub fn window_suitable(parent_wid: u32, attributes: u64, tags: u64) -> bool {
    parent_wid == 0
        && ((attributes & 0x2) != 0 || (tags & WINDOW_TAG_58) != 0)
        && (tags & WINDOW_TAG_ATTACHED) == 0
        && (tags & WINDOW_TAG_IGNORES_CYCLE) == 0
        && ((tags & WINDOW_TAG_DOCUMENT) != 0
            || ((tags & WINDOW_TAG_FLOATING) != 0 && (tags & WINDOW_TAG_MODAL) != 0))
}

/// `(set, clear)` tags of a border window (BR-DRW-04 step 10): floating + bit 9, plus
/// sticky (and clearing bit 45) when the target window is sticky.
pub fn border_window_tags(sticky: bool) -> (u64, u64) {
    let set = WINDOW_TAG_FLOATING | WINDOW_TAG_BORDER_9;
    if sticky {
        (set | WINDOW_TAG_STICKY, WINDOW_TAG_BORDER_45)
    } else {
        (set, 0)
    }
}

/// `too_small` (`border_check_too_small`): the window cannot hold its own rounded corners
/// (`CGRectInset(R, 1, 1)` narrower or lower than `2 · inner_radius`).
pub fn too_small(window: BorderRect, inner_radius: f64) -> bool {
    let smallest = window.inset(1.0, 1.0);
    smallest.width < 2.0 * inner_radius || smallest.height < 2.0 * inner_radius
}

/// The border offset `w + BORDER_PADDING` as `border_calculate_bounds` computes it: in
/// `double`, stored to a `float` (BR-DRW-04 step 2.4).
pub fn border_offset(width: f32) -> f64 {
    ((width as f64) + BORDER_PADDING) as f32 as f64
}

/// The border window of one target window (`border_calculate_bounds`, spec §7.2, §7.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderGeometry {
    /// The target window's frame (`target_bounds`).
    pub target: BorderRect,
    /// The border is hidden instead of drawn ([`too_small`]); the other fields are then
    /// unset.
    pub too_small: bool,
    /// Global origin of the border window: `(R.x − o, R.y − o)`.
    pub origin: BorderPoint,
    /// The border window's own frame: `(0, 0, R.w + 2o, R.h + 2o)`.
    pub frame: BorderRect,
    /// The target window in border-local coordinates: `(o, o, R.w, R.h)`.
    pub drawing_bounds: BorderRect,
}

/// `border_calculate_bounds`: the border window frame for `window` with the effective
/// `settings` and the window's `inner_radius`. The backing scale (hidpi) does not enter:
/// the frame is in points, [`backing_resolution`] only sets the bitmap density.
pub fn border_geometry(
    window: BorderRect,
    settings: &BorderSettings,
    inner_radius: f64,
) -> BorderGeometry {
    if too_small(window, inner_radius) {
        return BorderGeometry {
            target: window,
            too_small: true,
            origin: BorderPoint::default(),
            frame: BorderRect::default(),
            drawing_bounds: BorderRect::default(),
        };
    }
    let o = border_offset(settings.width);
    let outer = window.inset(-o, -o);
    BorderGeometry {
        target: window,
        too_small: false,
        origin: BorderPoint::new(outer.x, outer.y),
        frame: BorderRect::new(0.0, 0.0, outer.width, outer.height),
        drawing_bounds: BorderRect::new(o, o, window.width, window.height),
    }
}

/// `border_move` (BR-DRW-09): the new origin after a pure move, computed in `double`
/// (it may differ from [`BorderGeometry::origin`] in the last float bit, as in C).
pub fn move_origin(window: BorderRect, width: f32) -> BorderPoint {
    BorderPoint::new(
        window.x - width as f64 - BORDER_PADDING,
        window.y - width as f64 - BORDER_PADDING,
    )
}

/// An RGBA color with components in 0..=1 (`colors_from_hex`, computed in `float`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    /// Opaque black: the fill color of a fresh CoreGraphics context.
    pub const BLACK: Rgba = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };

    /// `colors_from_hex(0xAARRGGBB)`.
    pub fn from_argb(color: u32) -> Rgba {
        let c = |shift: u32| (((color >> shift) & 0xff) as f32 / 255.0) as f64;
        Rgba {
            r: c(16),
            g: c(8),
            b: c(0),
            a: c(24),
        }
    }
}

/// `drawing_create_gradient`: the start and end point of a gradient over a border window
/// of `frame_width × frame_height`. The context is unflipped (y up), so `(0, FH)` is the
/// top-left corner.
pub fn gradient_endpoints(
    direction: GradientDirection,
    frame_width: f64,
    frame_height: f64,
) -> (BorderPoint, BorderPoint) {
    match direction {
        GradientDirection::TopLeftToBottomRight => (
            BorderPoint::new(0.0, frame_height),
            BorderPoint::new(frame_width, 0.0),
        ),
        GradientDirection::TopRightToBottomLeft => (
            BorderPoint::new(frame_width, frame_height),
            BorderPoint::new(0.0, 0.0),
        ),
    }
}

/// A rounded rect whose radius never exceeds half its width or height, so
/// `CGPathCreateWithRoundedRect` / `CGPathAddRoundedRect` cannot assert (BQ32). Where
/// JankyBorders' own guard holds this is the identity.
pub fn clamp_radius(rect: BorderRect, radius: f64) -> f64 {
    radius.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0)
}

/// A path in border-local coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathShape {
    Rect(BorderRect),
    /// Radius already clamped with [`clamp_radius`].
    RoundedRect {
        rect: BorderRect,
        radius: f64,
    },
}

/// How the border color is applied (BR-DRW-06 step 2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    /// Fill and stroke color.
    Solid(Rgba),
    /// Fill and stroke color plus a shadow of `shadow` (the color at alpha 1) blurred by
    /// `blur`.
    Glow {
        color: Rgba,
        shadow: Rgba,
        blur: f64,
    },
    /// A linear gradient from `color1` at `start` to `color2` at `end` (sRGB, evenly
    /// spaced).
    Gradient {
        color1: Rgba,
        color2: Rgba,
        start: BorderPoint,
        end: BorderPoint,
    },
}

/// The visible band (BR-DRW-06 steps 6, 7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Band {
    /// Square: the rect is filled (solid/glow) or clipped to and painted with the gradient.
    Square(BorderRect),
    /// Round / uniform: the rounded rect is stroked with the line width (solid/glow), or
    /// replaced by its stroked outline, clipped to and painted with the gradient.
    Rounded { rect: BorderRect, radius: f64 },
}

/// Everything `border_draw` does, in order (BR-DRW-06).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawPlan {
    /// Cleared first; also the outer part of the even-odd clip.
    pub frame: BorderRect,
    /// `CGContextSetLineWidth`.
    pub line_width: f64,
    pub paint: Paint,
    /// The inner clip path `C`: painting happens only between `frame` and `C`. `None` when
    /// the truly-square inset collapses the window to `CGRectNull` (then nothing is
    /// painted).
    pub inner_clip: Option<PathShape>,
    /// Uniform style only: this rounded rect is filled first, with the paint's fill color
    /// (opaque black for gradients, BQ20, reproduced).
    pub corner_fill: Option<(PathShape, Rgba)>,
    /// The border band; `None` together with `inner_clip`.
    pub band: Option<Band>,
    /// Background (only with `order=below` and a visible solid/glow background): `C`
    /// filled with this color after the clip and the shadow were dropped.
    pub background: Option<(PathShape, Rgba)>,
}

/// Whether the truly-square mode applies (BR-DRW-06 step 4).
pub fn truly_square(settings: &BorderSettings, macos26: bool) -> bool {
    settings.style == BorderStyle::Square
        && settings.order == BorderOrder::Above
        && settings.width >= border_tsmw(macos26)
}

/// `border_draw` as data: what to paint for a border with `geometry`, the window's corner
/// `radius` (`inner_radius = radius + 1`), the effective `settings` and focus state.
/// `geometry` must not be [`BorderGeometry::too_small`].
pub fn draw_plan(
    geometry: &BorderGeometry,
    radius: f64,
    settings: &BorderSettings,
    focused: bool,
    macos26: bool,
) -> DrawPlan {
    let frame = geometry.frame;
    let inner = inner_radius(radius);
    let color = if focused {
        settings.active
    } else {
        settings.inactive
    };
    let paint = match color {
        BorderColor::Solid(c) => Paint::Solid(Rgba::from_argb(c)),
        BorderColor::Glow(c) => {
            let color = Rgba::from_argb(c);
            Paint::Glow {
                color,
                shadow: Rgba { a: 1.0, ..color },
                blur: GLOW_BLUR,
            }
        }
        BorderColor::Gradient {
            direction,
            color1,
            color2,
        } => {
            let (start, end) = gradient_endpoints(direction, frame.width, frame.height);
            Paint::Gradient {
                color1: Rgba::from_argb(color1),
                color2: Rgba::from_argb(color2),
                start,
                end,
            }
        }
    };
    let fill_color = match paint {
        Paint::Solid(c) | Paint::Glow { color: c, .. } => c,
        Paint::Gradient { .. } => Rgba::BLACK,
    };

    let d = geometry.drawing_bounds;
    let (path_rect, inner_clip) = if truly_square(settings, macos26) {
        let tsmn = BORDER_TSMN as f64;
        let r = d.inset(tsmn, tsmn);
        (r, (!r.is_null()).then_some(PathShape::Rect(r)))
    } else {
        let r = d.inset(1.0, 1.0);
        let clip = (!r.is_null()).then(|| PathShape::RoundedRect {
            rect: r,
            radius: clamp_radius(r, inner),
        });
        (d, clip)
    };

    let mut corner_fill = None;
    let band = if inner_clip.is_none() {
        None
    } else if settings.style == BorderStyle::Square {
        let inset = (-settings.width / 2.0) as f64;
        Some(Band::Square(path_rect.inset(inset, inset)))
    } else {
        let corner = if settings.style == BorderStyle::Uniform {
            UNIFORM_CORNER_RADIUS
        } else {
            radius
        };
        let corner = clamp_radius(path_rect, corner);
        if settings.style == BorderStyle::Uniform {
            corner_fill = Some((
                PathShape::RoundedRect {
                    rect: path_rect,
                    radius: corner,
                },
                fill_color,
            ));
        }
        Some(Band::Rounded {
            rect: path_rect,
            radius: corner,
        })
    };

    let background = match (inner_clip, settings.background) {
        (Some(clip), BorderColor::Solid(c) | BorderColor::Glow(c))
            if settings.show_background && settings.order != BorderOrder::Above =>
        {
            Some((clip, Rgba::from_argb(c)))
        }
        _ => None,
    };

    DrawPlan {
        frame,
        line_width: settings.width as f64,
        paint,
        inner_clip,
        corner_fill,
        band,
        background,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> BorderSettings {
        BorderSettings::default()
    }

    #[test]
    fn geometry_matches_border_calculate_bounds() {
        // R = (100, 50, 800, 600), w = 4 → o = 12.
        let r = BorderRect::new(100.0, 50.0, 800.0, 600.0);
        let g = border_geometry(r, &settings(), 10.0);
        assert!(!g.too_small);
        assert_eq!(g.target, r);
        assert_eq!(g.origin, BorderPoint::new(88.0, 38.0));
        assert_eq!(g.frame, BorderRect::new(0.0, 0.0, 824.0, 624.0));
        assert_eq!(g.drawing_bounds, BorderRect::new(12.0, 12.0, 800.0, 600.0));
        assert_eq!(move_origin(r, 4.0), g.origin);
    }

    #[test]
    fn offset_is_rounded_to_float() {
        // `float border_offset = -w - 8.0`: 4.3f + 8.0 (double) stored as float.
        let w = 4.3f32;
        let expected = ((w as f64) + 8.0) as f32 as f64;
        assert_eq!(border_offset(w), expected);
        let r = BorderRect::new(0.0, 0.0, 100.0, 100.0);
        let g = border_geometry(
            r,
            &BorderSettings {
                width: w,
                ..settings()
            },
            10.0,
        );
        assert_eq!(g.origin, BorderPoint::new(-expected, -expected));
        assert_eq!(g.frame.width, 100.0 + 2.0 * expected);
        // border_move computes the same offset in double.
        assert_eq!(move_origin(r, w).x, -(w as f64) - 8.0);
    }

    #[test]
    fn too_small_guard() {
        // inner radius 10: R.w − 2 must be ≥ 20.
        assert!(!too_small(BorderRect::new(0.0, 0.0, 22.0, 22.0), 10.0));
        assert!(too_small(BorderRect::new(0.0, 0.0, 21.9, 100.0), 10.0));
        assert!(too_small(BorderRect::new(0.0, 0.0, 100.0, 21.0), 10.0));
        // CGRectInset of a 1-pt window is CGRectNull (size 0).
        assert!(too_small(BorderRect::new(0.0, 0.0, 1.0, 1.0), 0.5));
        let g = border_geometry(BorderRect::new(5.0, 5.0, 10.0, 10.0), &settings(), 10.0);
        assert!(g.too_small);
    }

    #[test]
    fn rect_inset_semantics() {
        let r = BorderRect::new(10.0, 10.0, 4.0, 4.0);
        assert_eq!(r.inset(1.0, 1.0), BorderRect::new(11.0, 11.0, 2.0, 2.0));
        assert!(r.inset(3.0, 3.0).is_null());
        assert_eq!(r.inset(-2.0, -2.0), BorderRect::new(8.0, 8.0, 8.0, 8.0));
        assert_eq!(
            BorderRect::new(10.0, 10.0, -4.0, 4.0).standardized(),
            BorderRect::new(6.0, 10.0, 4.0, 4.0)
        );
    }

    #[test]
    fn radii_and_constants() {
        assert_eq!(corner_radius(None), 9.0);
        assert_eq!(corner_radius(Some(0)), 9.0);
        assert_eq!(corner_radius(Some(-3)), 9.0);
        assert_eq!(corner_radius(Some(16)), 16.0);
        assert_eq!(inner_radius(16.0), 17.0);
        assert_eq!(border_tsmw(true), 52.0);
        assert_eq!(border_tsmw(false), 8.0);
        assert_eq!(backing_resolution(true), 2.0);
        assert_eq!(backing_resolution(false), 1.0);
        assert_eq!(order_value(BorderOrder::Above), 1);
        assert_eq!(order_value(BorderOrder::Below), -1);
        assert_eq!(BORDER_TSMN as f64, 3.2699999809265137);
    }

    #[test]
    fn suitability_rules() {
        assert!(window_suitable(0, 0x2, WINDOW_TAG_DOCUMENT));
        assert!(window_suitable(0, 0, WINDOW_TAG_58 | WINDOW_TAG_DOCUMENT));
        assert!(window_suitable(
            0,
            0x2,
            WINDOW_TAG_FLOATING | WINDOW_TAG_MODAL
        ));
        assert!(!window_suitable(7, 0x2, WINDOW_TAG_DOCUMENT));
        assert!(!window_suitable(0, 0, WINDOW_TAG_DOCUMENT));
        assert!(!window_suitable(0, 0x2, WINDOW_TAG_FLOATING));
        assert!(!window_suitable(
            0,
            0x2,
            WINDOW_TAG_DOCUMENT | WINDOW_TAG_ATTACHED
        ));
        assert!(!window_suitable(
            0,
            0x2,
            WINDOW_TAG_DOCUMENT | WINDOW_TAG_IGNORES_CYCLE
        ));
        // Border windows themselves are never suitable.
        let (tags, _) = border_window_tags(false);
        assert!(!window_suitable(0, 0x2, tags));
        let (tags, _) = border_window_tags(true);
        assert!(!window_suitable(0, 0x2, tags));
    }

    #[test]
    fn border_tags() {
        assert_eq!(border_window_tags(false), ((1 << 1) | (1 << 9), 0));
        assert_eq!(
            border_window_tags(true),
            ((1 << 1) | (1 << 9) | (1 << 11), 1 << 45)
        );
    }

    #[test]
    fn color_components_use_float_division() {
        let c = Rgba::from_argb(0x80ff_4000);
        assert_eq!(c.a, (128.0f32 / 255.0) as f64);
        assert_eq!(c.r, 1.0);
        assert_eq!(c.g, (64.0f32 / 255.0) as f64);
        assert_eq!(c.b, 0.0);
    }

    #[test]
    fn gradient_directions() {
        assert_eq!(
            gradient_endpoints(GradientDirection::TopLeftToBottomRight, 824.0, 624.0),
            (BorderPoint::new(0.0, 624.0), BorderPoint::new(824.0, 0.0))
        );
        assert_eq!(
            gradient_endpoints(GradientDirection::TopRightToBottomLeft, 824.0, 624.0),
            (BorderPoint::new(824.0, 624.0), BorderPoint::new(0.0, 0.0))
        );
    }

    fn geometry() -> BorderGeometry {
        border_geometry(
            BorderRect::new(100.0, 50.0, 800.0, 600.0),
            &settings(),
            10.0,
        )
    }

    #[test]
    fn round_solid_plan() {
        let g = geometry();
        let p = draw_plan(&g, 9.0, &settings(), true, false);
        assert_eq!(p.frame, BorderRect::new(0.0, 0.0, 824.0, 624.0));
        assert_eq!(p.line_width, 4.0);
        assert_eq!(p.paint, Paint::Solid(Rgba::from_argb(0xffe1e3e4)));
        assert_eq!(
            p.inner_clip,
            Some(PathShape::RoundedRect {
                rect: BorderRect::new(13.0, 13.0, 798.0, 598.0),
                radius: 10.0
            })
        );
        assert_eq!(p.corner_fill, None);
        assert_eq!(
            p.band,
            Some(Band::Rounded {
                rect: BorderRect::new(12.0, 12.0, 800.0, 600.0),
                radius: 9.0
            })
        );
        assert_eq!(p.background, None);
        // Unfocused uses the inactive color.
        let p = draw_plan(&g, 9.0, &settings(), false, false);
        assert_eq!(p.paint, Paint::Solid(Rgba::from_argb(0)));
    }

    #[test]
    fn glow_plan_forces_shadow_alpha() {
        let s = BorderSettings {
            active: BorderColor::Glow(0x40102030),
            ..settings()
        };
        let p = draw_plan(&geometry(), 9.0, &s, true, false);
        let color = Rgba::from_argb(0x40102030);
        assert_eq!(
            p.paint,
            Paint::Glow {
                color,
                shadow: Rgba { a: 1.0, ..color },
                blur: 10.0
            }
        );
    }

    #[test]
    fn square_plans() {
        let s = BorderSettings {
            style: BorderStyle::Square,
            width: 10.0,
            ..settings()
        };
        let g = border_geometry(BorderRect::new(0.0, 0.0, 800.0, 600.0), &s, 10.0);
        // o = 18; below → not truly square: band = D outset by w/2.
        let p = draw_plan(&g, 9.0, &s, true, false);
        assert_eq!(
            p.band,
            Some(Band::Square(BorderRect::new(13.0, 13.0, 810.0, 610.0)))
        );
        assert!(matches!(p.inner_clip, Some(PathShape::RoundedRect { .. })));
        // Above with w ≥ 8 (pre-26): truly square, D inset by 3.27f.
        let s = BorderSettings {
            order: BorderOrder::Above,
            ..s
        };
        let p = draw_plan(&g, 9.0, &s, true, false);
        let t = BORDER_TSMN as f64;
        let path = BorderRect::new(18.0 + t, 18.0 + t, 800.0 - 2.0 * t, 600.0 - 2.0 * t);
        assert_eq!(p.inner_clip, Some(PathShape::Rect(path)));
        assert_eq!(p.band, Some(Band::Square(path.inset(-5.0, -5.0))));
        // On macOS 26 the threshold is 52.
        let p = draw_plan(&g, 9.0, &s, true, true);
        assert!(matches!(p.inner_clip, Some(PathShape::RoundedRect { .. })));
        assert!(!truly_square(&s, true));
        assert!(truly_square(&BorderSettings { width: 52.0, ..s }, true));
    }

    #[test]
    fn uniform_plan_fills_corners() {
        let s = BorderSettings {
            style: BorderStyle::Uniform,
            ..settings()
        };
        let g = border_geometry(BorderRect::new(0.0, 0.0, 800.0, 600.0), &s, 17.0);
        let p = draw_plan(&g, 16.0, &s, true, false);
        let d = BorderRect::new(12.0, 12.0, 800.0, 600.0);
        let shape = PathShape::RoundedRect {
            rect: d,
            radius: 9.0,
        };
        assert_eq!(p.corner_fill, Some((shape, Rgba::from_argb(0xffe1e3e4))));
        assert_eq!(
            p.band,
            Some(Band::Rounded {
                rect: d,
                radius: 9.0
            })
        );
        // Gradient: the fill color was never set → opaque black (BQ20).
        let s = BorderSettings {
            active: BorderColor::Gradient {
                direction: GradientDirection::TopLeftToBottomRight,
                color1: 0xffff0000,
                color2: 0xff0000ff,
            },
            ..s
        };
        let p = draw_plan(&g, 16.0, &s, true, false);
        assert_eq!(p.corner_fill, Some((shape, Rgba::BLACK)));
        assert_eq!(
            p.paint,
            Paint::Gradient {
                color1: Rgba::from_argb(0xffff0000),
                color2: Rgba::from_argb(0xff0000ff),
                start: BorderPoint::new(0.0, 624.0),
                end: BorderPoint::new(824.0, 0.0),
            }
        );
    }

    #[test]
    fn uniform_radius_is_clamped_for_tiny_windows() {
        // BQ32: reported radius 2 → inner 3, a 10-pt window passes too_small (8 ≥ 6) but
        // cannot hold radius 9.
        let s = BorderSettings {
            style: BorderStyle::Uniform,
            ..settings()
        };
        let g = border_geometry(BorderRect::new(0.0, 0.0, 10.0, 30.0), &s, 3.0);
        assert!(!g.too_small);
        let p = draw_plan(&g, 2.0, &s, true, false);
        assert_eq!(
            p.band,
            Some(Band::Rounded {
                rect: BorderRect::new(12.0, 12.0, 10.0, 30.0),
                radius: 5.0
            })
        );
        assert_eq!(
            clamp_radius(BorderRect::new(0.0, 0.0, 100.0, 100.0), 9.0),
            9.0
        );
        assert_eq!(
            clamp_radius(BorderRect::new(0.0, 0.0, 100.0, 4.0), 9.0),
            2.0
        );
    }

    #[test]
    fn background_only_below() {
        let s = BorderSettings {
            background: BorderColor::Solid(0x80000000),
            show_background: true,
            ..settings()
        };
        let p = draw_plan(&geometry(), 9.0, &s, false, false);
        assert_eq!(
            p.background,
            Some((p.inner_clip.unwrap(), Rgba::from_argb(0x80000000)))
        );
        let above = BorderSettings {
            order: BorderOrder::Above,
            ..s.clone()
        };
        assert_eq!(
            draw_plan(&geometry(), 9.0, &above, false, false).background,
            None
        );
        let hidden = BorderSettings {
            show_background: false,
            ..s
        };
        assert_eq!(
            draw_plan(&geometry(), 9.0, &hidden, false, false).background,
            None
        );
    }

    #[test]
    fn collapsed_truly_square_draws_nothing() {
        // A 6-pt wide window with a 60-pt border above: D inset by 3.27 is CGRectNull.
        let s = BorderSettings {
            style: BorderStyle::Square,
            order: BorderOrder::Above,
            width: 60.0,
            ..settings()
        };
        let g = border_geometry(BorderRect::new(0.0, 0.0, 6.0, 100.0), &s, 1.0);
        assert!(!g.too_small);
        let p = draw_plan(&g, 0.0, &s, true, true);
        assert_eq!(p.inner_clip, None);
        assert_eq!(p.band, None);
        assert_eq!(p.background, None);
    }
}
