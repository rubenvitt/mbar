//! CoreGraphics drawing of one border window (`border_draw`, `docs/spec/borders.md` §7.4,
//! JankyBorders `src/border.c:58-159` and `src/misc/drawing.h`).
//!
//! What to paint is decided by the pure [`mbar_core::borders::draw_plan`]; this module only
//! replays it on the window's `SLWindowContextCreate` context, in JankyBorders' order:
//! save, paint setup (fill+stroke color, glow shadow or gradient), line width, clear,
//! even-odd clip between the frame and the inner path, band, background (after dropping
//! the clip and the shadow), flush, restore, `SLSFlushWindowContentRegion`,
//! `SLSWindowThaw`.

use super::ffi;
use mbar_core::borders::{Band, BorderPoint, BorderRect, DrawPlan, Paint, PathShape, Rgba};
use objc2_core_foundation::{CFArray, CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGColor, CGContext, CGGradient, CGGradientDrawingOptions, CGMutablePath, CGPath,
};

fn cg_rect(r: BorderRect) -> CGRect {
    CGRect::new(CGPoint::new(r.x, r.y), CGSize::new(r.width, r.height))
}

fn cg_point(p: BorderPoint) -> CGPoint {
    CGPoint::new(p.x, p.y)
}

fn set_fill(ctx: &CGContext, c: Rgba) {
    CGContext::set_rgb_fill_color(Some(ctx), c.r, c.g, c.b, c.a);
}

fn set_stroke(ctx: &CGContext, c: Rgba) {
    CGContext::set_rgb_stroke_color(Some(ctx), c.r, c.g, c.b, c.a);
}

/// A new immutable path for `shape` (`CGPathCreateWithRect` /
/// `CGPathCreateWithRoundedRect`; the radius is already clamped by the plan).
fn shape_path(shape: PathShape) -> CFRetained<CGPath> {
    match shape {
        // SAFETY: a NULL transform is allowed.
        PathShape::Rect(r) => unsafe { CGPath::with_rect(cg_rect(r), std::ptr::null()) },
        PathShape::RoundedRect { rect, radius } => {
            // SAFETY: as above; `2·radius ≤ min(width, height)` holds (clamp_radius).
            unsafe { CGPath::with_rounded_rect(cg_rect(rect), radius, radius, std::ptr::null()) }
        }
    }
}

/// `drawing_create_gradient`: two sRGB stops, evenly spaced, default color space.
fn gradient(color1: Rgba, color2: Rgba) -> Option<CFRetained<CGGradient>> {
    let c1 = CGColor::new_srgb(color1.r, color1.g, color1.b, color1.a);
    let c2 = CGColor::new_srgb(color2.r, color2.g, color2.b, color2.a);
    let colors = CFArray::from_retained_objects(&[c1, c2]);
    // SAFETY: a CFArray of CGColors; NULL locations = evenly spaced.
    unsafe { CGGradient::with_colors(None, Some(colors.as_opaque()), std::ptr::null()) }
}

/// Paints `plan` into `ctx` and flushes the window (`cid`, `wid`) to the WindowServer.
pub(super) fn draw(ctx: &CGContext, cid: i32, wid: u32, plan: &DrawPlan) {
    let c = Some(ctx);
    CGContext::save_g_state(c);

    // Paint setup.
    let mut grad = None;
    match plan.paint {
        Paint::Solid(color) => {
            set_fill(ctx, color);
            set_stroke(ctx, color);
        }
        Paint::Glow {
            color,
            shadow,
            blur,
        } => {
            set_fill(ctx, color);
            set_stroke(ctx, color);
            let shadow = CGColor::new_generic_rgb(shadow.r, shadow.g, shadow.b, shadow.a);
            CGContext::set_shadow_with_color(c, CGSize::new(0.0, 0.0), blur, Some(&shadow));
        }
        Paint::Gradient {
            color1,
            color2,
            start,
            end,
        } => grad = gradient(color1, color2).map(|g| (g, cg_point(start), cg_point(end))),
    }

    CGContext::set_line_width(c, plan.line_width);
    CGContext::clear_rect(c, cg_rect(plan.frame));

    let inner = plan.inner_clip.map(shape_path);
    if let Some(inner) = &inner {
        // drawing_clip_between_rect_and_path: frame + C, even-odd.
        let clip = CGMutablePath::new();
        // SAFETY: NULL transforms are allowed; both paths are live.
        unsafe {
            CGMutablePath::add_rect(Some(&clip), std::ptr::null(), cg_rect(plan.frame));
            CGMutablePath::add_path(Some(&clip), std::ptr::null(), Some(inner));
        }
        CGContext::add_path(c, Some(&clip));
        CGContext::eo_clip(c);
    }

    if let Some((shape, color)) = plan.corner_fill {
        // Uniform style: fill the corner gaps with the current fill color first.
        set_fill(ctx, color);
        CGContext::add_path(c, Some(&shape_path(shape)));
        CGContext::fill_path(c);
    }

    match (plan.band, &grad) {
        (Some(Band::Square(rect)), None) => {
            CGContext::add_path(c, Some(&shape_path(PathShape::Rect(rect))));
            CGContext::fill_path(c);
        }
        (Some(Band::Square(rect)), Some((g, start, end))) => {
            CGContext::add_path(c, Some(&shape_path(PathShape::Rect(rect))));
            CGContext::clip(c);
            CGContext::draw_linear_gradient(c, Some(g), *start, *end, CGGradientDrawingOptions(0));
        }
        (Some(Band::Rounded { rect, radius }), None) => {
            CGContext::add_path(
                c,
                Some(&shape_path(PathShape::RoundedRect { rect, radius })),
            );
            CGContext::stroke_path(c);
        }
        (Some(Band::Rounded { rect, radius }), Some((g, start, end))) => {
            CGContext::add_path(
                c,
                Some(&shape_path(PathShape::RoundedRect { rect, radius })),
            );
            CGContext::replace_path_with_stroked_path(c);
            CGContext::clip(c);
            CGContext::draw_linear_gradient(c, Some(g), *start, *end, CGGradientDrawingOptions(0));
        }
        (None, _) => {}
    }
    drop(grad);

    if let Some((shape, color)) = plan.background {
        // Drop the clip and the shadow, then fill C behind the window.
        CGContext::restore_g_state(c);
        CGContext::save_g_state(c);
        set_fill(ctx, color);
        set_stroke(ctx, Rgba::from_argb(0));
        CGContext::add_path(c, Some(&shape_path(shape)));
        CGContext::fill_path(c);
    }

    CGContext::flush(c);
    CGContext::restore_g_state(c);
    ffi::flush_window(cid, wid);
    ffi::thaw(cid, wid);
}
