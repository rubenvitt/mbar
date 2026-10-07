//! Pure helpers of the graphics layer (no Apple types): coordinate conversion,
//! SketchyBar drawing math, font-feature parsing, pixel bounds and graph tessellation.
//!
//! Everything here is deterministic and unit tested. Because `lib.rs` is gated on
//! `target_os = "macos"`, these tests only run on macOS CI (they can also be run on any
//! host by including this file via `#[path]` in a scratch crate).

use super::scene::{Point, Rect};

// ---------------------------------------------------------------------------------------
// Coordinates
// ---------------------------------------------------------------------------------------

/// Global top-left screen rect (CG display coordinates, y down from the top of the
/// primary display) → AppKit global bottom-left rect `(x, y, w, h)`.
///
/// `primary_height` is the height of the *primary* screen (`NSScreen.screens[0]`, the one
/// whose AppKit origin is `(0, 0)`), not `NSScreen.mainScreen` (the key-window screen).
pub fn top_left_to_appkit(rect: Rect, primary_height: f64) -> (f64, f64, f64, f64) {
    let h = rect.height as f64;
    (
        rect.x as f64,
        primary_height - (rect.y as f64 + h),
        rect.width as f64,
        h,
    )
}

/// Inverse of [`top_left_to_appkit`].
pub fn appkit_to_top_left(x: f64, y: f64, w: f64, h: f64, primary_height: f64) -> Rect {
    Rect::new(
        x as f32,
        (primary_height - (y + h)) as f32,
        w as f32,
        h as f32,
    )
}

/// A point in an unflipped view (origin bottom-left) → top-left view coordinates.
pub fn flip_y(p: Point, view_height: f32) -> Point {
    Point::new(p.x, view_height - p.y)
}

/// Drawable size in pixels for a view of `size` points at `scale` (never 0).
pub fn drawable_size(width: f32, height: f32, scale: f32) -> (u32, u32) {
    let px = |v: f32| -> u32 {
        let v = (v.max(0.0) * scale).ceil();
        if v.is_finite() {
            (v as u32).clamp(1, 16384)
        } else {
            1
        }
    };
    (px(width), px(height))
}

/// Snap a coordinate (points) to the device pixel grid.
pub fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

// ---------------------------------------------------------------------------------------
// SketchyBar drawing math (docs/spec/components.md)
// ---------------------------------------------------------------------------------------

/// `background.c:draw_rect` radius clamp: if the radius exceeds half the inset width or
/// height, it becomes `(u32)(min(w, h) / 2)` (truncated).
pub fn clamp_corner_radius(inset_w: f32, inset_h: f32, radius: f32) -> f32 {
    let radius = radius.max(0.0);
    if radius > inset_h / 2.0 || radius > inset_w / 2.0 {
        let half = if inset_h > inset_w {
            inset_w / 2.0
        } else {
            inset_h / 2.0
        };
        half.max(0.0).trunc()
    } else {
        radius
    }
}

/// Geometry of a `draw_rect` call: `(inset rect, clamped radius)` where the inset rect is
/// `CGRectInset(region, lw/2, lw/2)`; the fill covers the inset rounded rect and the
/// stroke (width `lw`) is centred on its edge.
pub fn rounded_rect_geometry(region: Rect, radius: f32, border_width: f32) -> (Rect, f32) {
    let lw = border_width.max(0.0);
    let inset = region.inset(lw / 2.0, lw / 2.0);
    let r = clamp_corner_radius(inset.width, inset.height, radius);
    (inset, r)
}

/// `image.c:image_draw`: an image is clipped/bordered only when `h > 2r && w > 2r`.
pub fn image_is_rounded(width: f32, height: f32, radius: f32) -> bool {
    height > 2.0 * radius && width > 2.0 * radius
}

/// `(u32)(v + 1.5)` used for ink widths/heights (`text.c:text_prepare_line`); non-finite
/// or negative inputs (e.g. `CGRectNull` of an empty line) count as 0 → 1.
pub fn ink_extent(v: f64) -> u32 {
    let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
    (v + 1.5) as u32
}

/// `(u32)(v + 0.5)` used for the typographic width.
pub fn round_half_up_u32(v: f64) -> u32 {
    let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
    (v + 0.5) as u32
}

/// `(i32)(v + 0.5)` (C truncation toward zero) used for ink bounds origins.
pub fn trunc_half_i32(v: f64) -> i32 {
    if v.is_finite() {
        (v + 0.5) as i32
    } else {
        0
    }
}

/// Byte prefix of `s` containing its first `max_chars` code points (`max_chars == 0` →
/// the whole string), see `text.c` truncation.
pub fn char_prefix(s: &str, max_chars: u32) -> &str {
    if max_chars == 0 {
        return s;
    }
    match s.char_indices().nth(max_chars as usize) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

// ---------------------------------------------------------------------------------------
// Font features (`font.c:font_create_ctfont`)
// ---------------------------------------------------------------------------------------

/// One parsed font feature setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFeature {
    /// `n:m` → AAT `{kCTFontFeatureTypeIdentifierKey: n, kCTFontFeatureSelectorIdentifierKey: m}`.
    Aat { kind: i32, selector: i32 },
    /// `[+|-]tag` → `{kCTFontOpenTypeFeatureTag: tag, kCTFontOpenTypeFeatureValue: value}`.
    OpenType { tag: [u8; 4], value: i32 },
}

/// `sscanf("%d")` prefix: optional leading whitespace, optional sign, ≥ 1 digit. Returns
/// the value (wrapping like a 32-bit store) and the rest of the string.
fn scan_c_int(s: &str) -> Option<(i32, &str)> {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let b = t.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut v: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add((b[i] - b'0') as i64);
        i += 1;
    }
    if i == start {
        return None;
    }
    let v = if neg { v.wrapping_neg() } else { v };
    Some((v as i32, &t[i..]))
}

/// Parse one feature entry exactly like SketchyBar: `"%d:%d"` (trailing garbage ignored)
/// → AAT; else optional `+`/`-` then exactly 4 bytes → OpenType; otherwise `None`.
pub fn parse_feature(entry: &str) -> Option<FontFeature> {
    if let Some((kind, rest)) = scan_c_int(entry) {
        if let Some(rest) = rest.strip_prefix(':') {
            if let Some((selector, _)) = scan_c_int(rest) {
                return Some(FontFeature::Aat { kind, selector });
            }
        }
    }
    let (value, tag) = if let Some(t) = entry.strip_prefix('+') {
        (1, t)
    } else if let Some(t) = entry.strip_prefix('-') {
        (0, t)
    } else {
        (1, entry)
    };
    let bytes = tag.as_bytes();
    if bytes.len() != 4 {
        return None;
    }
    Some(FontFeature::OpenType {
        tag: [bytes[0], bytes[1], bytes[2], bytes[3]],
        value,
    })
}

/// Split a feature list on `,` (empty entries skipped, like `strtok`) and parse each.
pub fn parse_features(list: &str) -> impl Iterator<Item = FontFeature> + '_ {
    list.split(',')
        .filter(|e| !e.is_empty())
        .filter_map(parse_feature)
}

// ---------------------------------------------------------------------------------------
// Text rasterization bounds
// ---------------------------------------------------------------------------------------

/// Integer pixel bounds of a rasterized run, relative to the pen origin, **y up**
/// (CoreGraphics). `x0/y0` inclusive, `x1/y1` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBounds {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl PixelBounds {
    pub fn width(&self) -> u32 {
        (self.x1 - self.x0).max(0) as u32
    }
    pub fn height(&self) -> u32 {
        (self.y1 - self.y0).max(0) as u32
    }
}

/// Largest bitmap side we rasterize a single run into.
pub const MAX_RUN_PIXELS: u32 = 8192;

/// Pixel box covering the union of the ink box (`ink_*`, y up, relative to the pen) and
/// the typographic box (`0..advance` × `-descent..ascent`), padded by `pad` points, at
/// `scale`. `None` when empty or absurdly large.
#[allow(clippy::too_many_arguments)]
pub fn raster_bounds(
    ink_x: f64,
    ink_y: f64,
    ink_w: f64,
    ink_h: f64,
    advance: f64,
    ascent: f64,
    descent: f64,
    pad: f64,
    scale: f64,
) -> Option<PixelBounds> {
    let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
    let (mut lx, mut ly, mut hx, mut hy) =
        (0.0f64, -finite(descent), finite(advance), finite(ascent));
    let ink_valid = ink_x.is_finite() && ink_y.is_finite() && ink_w > 0.0 && ink_h > 0.0;
    if ink_valid {
        lx = lx.min(ink_x);
        ly = ly.min(ink_y);
        hx = hx.max(ink_x + ink_w);
        hy = hy.max(ink_y + ink_h);
    }
    if !(hx > lx && hy > ly) {
        return None;
    }
    let b = PixelBounds {
        x0: ((lx - pad) * scale).floor() as i32,
        y0: ((ly - pad) * scale).floor() as i32,
        x1: ((hx + pad) * scale).ceil() as i32,
        y1: ((hy + pad) * scale).ceil() as i32,
    };
    if b.width() == 0
        || b.height() == 0
        || b.width() > MAX_RUN_PIXELS
        || b.height() > MAX_RUN_PIXELS
    {
        return None;
    }
    Some(b)
}

/// Quantize a premultiplied colour to a `u32` cache key (RGBA8).
pub fn rgba_key(c: [f32; 4]) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(c[0]) << 24) | (q(c[1]) << 16) | (q(c[2]) << 8) | q(c[3])
}

/// Undo premultiplication (for APIs that take straight colour). Alpha 0 → zeros.
pub fn unpremultiply(c: [f32; 4]) -> [f32; 4] {
    if c[3] <= 0.0 {
        return [0.0; 4];
    }
    [
        (c[0] / c[3]).min(1.0),
        (c[1] / c[3]).min(1.0),
        (c[2] / c[3]).min(1.0),
        c[3],
    ]
}

// ---------------------------------------------------------------------------------------
// Graph tessellation
// ---------------------------------------------------------------------------------------

/// One tessellated vertex of a graph path. `dist` is the signed distance from the centre
/// line (points) for stroke vertices; `half_width` the stroke half width (a huge value
/// marks fill vertices, which are always fully covered).
/// `#[repr(C)]`: uploaded verbatim as the shader's `PathVertex` (32 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PathVertex {
    pub pos: [f32; 2],
    pub dist: f32,
    pub half_width: f32,
    pub color: [f32; 4],
}

/// `half_width` value used for fill vertices.
pub const FILL_HALF_WIDTH: f32 = 1.0e6;

/// CoreGraphics' default miter limit.
pub const MITER_LIMIT: f32 = 10.0;

fn normal(a: Point, b: Point) -> Option<[f32; 2]> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len <= 1e-6 || !len.is_finite() {
        return None;
    }
    Some([-dy / len, dx / len])
}

/// Tessellate the stroke of a polyline into a triangle list (miter joins up to
/// [`MITER_LIMIT`], beyond that the segments are emitted unjoined; butt caps).
/// `aa` (points) widens the geometry so the fragment shader can antialias the edges.
/// Consecutive duplicate points are ignored. Appends to `out`.
pub fn tessellate_stroke(
    points: &[Point],
    line_width: f32,
    aa: f32,
    color: [f32; 4],
    out: &mut Vec<PathVertex>,
) {
    let hw = line_width.max(0.0) / 2.0;
    if hw <= 0.0 || !hw.is_finite() || color[3] <= 0.0 {
        return;
    }
    let w = hw + aa.max(0.0);
    // Walk the non-degenerate segments; each segment is emitted once the offset at its
    // end (the join with the following segment) is known.
    let mut prev: Option<(Point, [f32; 2], [f32; 2])> = None; // (start, normal, start offset)
    let mut i = 0;
    while i + 1 < points.len() {
        let a = points[i];
        // Find the next point that differs from `a`.
        let mut j = i + 1;
        let mut n = normal(a, points[j]);
        while n.is_none() && j + 1 < points.len() {
            j += 1;
            n = normal(a, points[j]);
        }
        let Some(n) = n else { break };
        let own = [n[0] * w, n[1] * w];
        let start = match prev {
            None => own,
            Some((pa, pn, p_start)) => match miter_offset(pn, n, w) {
                Some(m) => {
                    emit_quad(pa, a, p_start, m, w, hw, color, out);
                    m
                }
                None => {
                    emit_quad(pa, a, p_start, [pn[0] * w, pn[1] * w], w, hw, color, out);
                    own
                }
            },
        };
        prev = Some((a, n, start));
        i = j;
    }
    if let Some((pa, pn, p_start)) = prev {
        emit_quad(
            pa,
            points[i],
            p_start,
            [pn[0] * w, pn[1] * w],
            w,
            hw,
            color,
            out,
        );
    }
}

/// Miter offset at a joint between segments with normals `n0` → `n1`, or `None` when the
/// miter would exceed the limit (then both segments use their own normals).
fn miter_offset(n0: [f32; 2], n1: [f32; 2], w: f32) -> Option<[f32; 2]> {
    let mx = n0[0] + n1[0];
    let my = n0[1] + n1[1];
    let len = (mx * mx + my * my).sqrt();
    if len <= 1e-6 {
        return None;
    }
    let m = [mx / len, my / len];
    let cos = m[0] * n1[0] + m[1] * n1[1];
    if cos <= 1e-6 {
        return None;
    }
    let k = 1.0 / cos;
    // CG: miter length / line width = 1/sin(θ/2) = 1/cos(φ) with φ the half turn angle.
    if k > MITER_LIMIT {
        return None;
    }
    Some([m[0] * w * k, m[1] * w * k])
}

#[allow(clippy::too_many_arguments)]
fn emit_quad(
    a: Point,
    b: Point,
    off_a: [f32; 2],
    off_b: [f32; 2],
    w: f32,
    hw: f32,
    color: [f32; 4],
    out: &mut Vec<PathVertex>,
) {
    let v = |p: Point, o: [f32; 2], sign: f32| PathVertex {
        pos: [p.x + o[0] * sign, p.y + o[1] * sign],
        dist: w * sign,
        half_width: hw,
        color,
    };
    let al = v(a, off_a, 1.0);
    let ar = v(a, off_a, -1.0);
    let bl = v(b, off_b, 1.0);
    let br = v(b, off_b, -1.0);
    out.extend_from_slice(&[al, ar, bl, bl, ar, br]);
}

/// Triangulate the area between an x-monotone polyline and the horizontal line
/// `y = baseline` (trapezoid strips). Appends a triangle list to `out`.
pub fn tessellate_fill(
    points: &[Point],
    baseline: f32,
    color: [f32; 4],
    out: &mut Vec<PathVertex>,
) {
    if color[3] <= 0.0 {
        return;
    }
    let v = |x: f32, y: f32| PathVertex {
        pos: [x, y],
        dist: 0.0,
        half_width: FILL_HALF_WIDTH,
        color,
    };
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if (a.x - b.x).abs() <= 1e-6 {
            continue;
        }
        let a0 = v(a.x, a.y);
        let b0 = v(b.x, b.y);
        let a1 = v(a.x, baseline);
        let b1 = v(b.x, baseline);
        out.extend_from_slice(&[a0, a1, b0, b0, a1, b1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appkit_conversion_roundtrip() {
        let r = Rect::new(10.0, 0.0, 200.0, 32.0);
        let (x, y, w, h) = top_left_to_appkit(r, 1080.0);
        assert_eq!((x, y, w, h), (10.0, 1048.0, 200.0, 32.0));
        assert_eq!(appkit_to_top_left(x, y, w, h, 1080.0), r);
        // A display above the primary one has negative top-left y.
        let (_, y2, _, _) = top_left_to_appkit(Rect::new(0.0, -1440.0, 100.0, 40.0), 1080.0);
        assert_eq!(y2, 2480.0);
        assert_eq!(flip_y(Point::new(3.0, 5.0), 32.0), Point::new(3.0, 27.0));
    }

    #[test]
    fn drawable_sizes() {
        assert_eq!(drawable_size(100.5, 32.0, 2.0), (201, 64));
        assert_eq!(drawable_size(0.0, 0.0, 2.0), (1, 1));
        assert_eq!(drawable_size(f32::INFINITY, 1.0, 1.0), (1, 1));
        assert_eq!(snap(1.3, 2.0), 1.5);
    }

    #[test]
    fn corner_radius_clamp() {
        assert_eq!(clamp_corner_radius(100.0, 20.0, 5.0), 5.0);
        assert_eq!(clamp_corner_radius(100.0, 20.0, 11.0), 10.0);
        assert_eq!(clamp_corner_radius(15.0, 20.0, 9.0), 7.0); // (u32)(7.5)
        assert_eq!(clamp_corner_radius(100.0, 21.0, 10.5), 10.5);
        assert_eq!(clamp_corner_radius(100.0, 21.0, 11.0), 10.0);
        let (inset, r) = rounded_rect_geometry(Rect::new(0.0, 0.0, 40.0, 20.0), 50.0, 2.0);
        assert_eq!(inset, Rect::new(1.0, 1.0, 38.0, 18.0));
        assert_eq!(r, 9.0);
    }

    #[test]
    fn image_rounding_rule() {
        assert!(image_is_rounded(32.0, 32.0, 0.0));
        assert!(image_is_rounded(32.0, 32.0, 15.9));
        assert!(!image_is_rounded(32.0, 32.0, 16.0));
        assert!(!image_is_rounded(0.0, 32.0, 0.0));
    }

    #[test]
    fn ink_rounding() {
        assert_eq!(ink_extent(0.0), 1);
        assert_eq!(ink_extent(f64::NAN), 1);
        assert_eq!(ink_extent(10.4), 11);
        assert_eq!(ink_extent(10.6), 12);
        assert_eq!(round_half_up_u32(10.49), 10);
        assert_eq!(round_half_up_u32(10.5), 11);
        assert_eq!(trunc_half_i32(-0.7), 0);
        assert_eq!(trunc_half_i32(-1.7), -1);
        assert_eq!(trunc_half_i32(1.5), 2);
    }

    #[test]
    fn prefixes() {
        assert_eq!(char_prefix("hello", 0), "hello");
        assert_eq!(char_prefix("hello", 3), "hel");
        assert_eq!(char_prefix("hé€x", 3), "hé€");
        assert_eq!(char_prefix("ab", 5), "ab");
    }

    #[test]
    fn features() {
        let f: Vec<_> = parse_features("+tnum,-liga,1:0,,smcp,toolong,+ab,3:x, 4:5abc").collect();
        assert_eq!(
            f,
            vec![
                FontFeature::OpenType {
                    tag: *b"tnum",
                    value: 1
                },
                FontFeature::OpenType {
                    tag: *b"liga",
                    value: 0
                },
                FontFeature::Aat {
                    kind: 1,
                    selector: 0
                },
                FontFeature::OpenType {
                    tag: *b"smcp",
                    value: 1
                },
                // "3:x" fails %d:%d and is 3 bytes → skipped; " 4:5abc" matches %d:%d.
                FontFeature::Aat {
                    kind: 4,
                    selector: 5
                },
            ]
        );
        assert_eq!(
            parse_feature("-3:-2"),
            Some(FontFeature::Aat {
                kind: -3,
                selector: -2
            })
        );
        assert_eq!(parse_feature("1:"), None);
        assert_eq!(
            parse_feature("1:ab"),
            Some(FontFeature::OpenType {
                tag: *b"1:ab",
                value: 1
            })
        );
        assert_eq!(parse_feature(""), None);
    }

    #[test]
    fn raster_bounds_union() {
        // Ink inside the typographic box.
        let b = raster_bounds(1.0, 0.0, 8.0, 10.0, 10.0, 12.0, 3.0, 1.0, 2.0).unwrap();
        assert_eq!(
            b,
            PixelBounds {
                x0: -2,
                y0: -8,
                x1: 22,
                y1: 26
            }
        );
        // Emoji-like: no ink, typographic box only.
        let b = raster_bounds(
            f64::INFINITY,
            f64::INFINITY,
            0.0,
            0.0,
            10.0,
            10.0,
            2.0,
            0.0,
            1.0,
        )
        .unwrap();
        assert_eq!((b.width(), b.height()), (10, 12));
        assert!(raster_bounds(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0).is_none());
        assert!(raster_bounds(0.0, 0.0, 1.0e6, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0).is_none());
    }

    #[test]
    fn color_helpers() {
        assert_eq!(rgba_key([1.0, 0.0, 0.0, 1.0]), 0xff0000ff);
        assert_eq!(unpremultiply([0.25, 0.0, 0.5, 0.5]), [0.5, 0.0, 1.0, 0.5]);
        assert_eq!(unpremultiply([0.25, 0.0, 0.5, 0.0]), [0.0; 4]);
    }

    #[test]
    fn stroke_straight_line() {
        let mut out = Vec::new();
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 0.0),
        ];
        tessellate_stroke(&pts, 2.0, 0.5, [1.0; 4], &mut out);
        assert_eq!(out.len(), 6);
        // Normal of +x direction is (0, 1); width incl. AA is 1.5.
        assert_eq!(out[0].pos, [0.0, 1.5]);
        assert_eq!(out[1].pos, [0.0, -1.5]);
        assert_eq!(out[5].pos, [10.0, -1.5]);
        assert_eq!(out[0].dist, 1.5);
        assert_eq!(out[0].half_width, 1.0);
    }

    #[test]
    fn stroke_miter_join() {
        let mut out = Vec::new();
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
        ];
        tessellate_stroke(&pts, 2.0, 0.0, [1.0; 4], &mut out);
        assert_eq!(out.len(), 12);
        // End of segment 0 and start of segment 1 share the miter vertex.
        let end0 = out[2].pos;
        let start1 = out[6].pos;
        assert!((end0[0] - start1[0]).abs() < 1e-5 && (end0[1] - start1[1]).abs() < 1e-5);
        // 90° miter: offset (−1, 1) from the corner on the left side.
        assert!((end0[0] - 9.0).abs() < 1e-5 && (end0[1] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn stroke_spike_falls_back_unjoined() {
        let mut out = Vec::new();
        // Nearly reversing direction: miter far beyond the limit.
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 0.1),
        ];
        tessellate_stroke(&pts, 2.0, 0.0, [1.0; 4], &mut out);
        assert_eq!(out.len(), 12);
        for v in &out {
            assert!(v.pos[0].abs() < 20.0 && v.pos[1].abs() < 5.0);
        }
    }

    #[test]
    fn stroke_degenerate() {
        let mut out = Vec::new();
        tessellate_stroke(&[Point::new(1.0, 1.0)], 1.0, 0.5, [1.0; 4], &mut out);
        tessellate_stroke(
            &[Point::new(1.0, 1.0), Point::new(1.0, 1.0)],
            1.0,
            0.5,
            [1.0; 4],
            &mut out,
        );
        tessellate_stroke(
            &[Point::new(0.0, 0.0), Point::new(1.0, 1.0)],
            0.0,
            0.5,
            [1.0; 4],
            &mut out,
        );
        tessellate_stroke(
            &[Point::new(0.0, 0.0), Point::new(1.0, 1.0)],
            1.0,
            0.5,
            [0.0; 4],
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn fill_trapezoids() {
        let mut out = Vec::new();
        let pts = [
            Point::new(0.0, 5.0),
            Point::new(0.0, 2.0), // vertical first segment (graph LTR) contributes nothing
            Point::new(1.0, 3.0),
            Point::new(2.0, 4.0),
        ];
        tessellate_fill(&pts, 10.0, [0.2; 4], &mut out);
        assert_eq!(out.len(), 12);
        assert!(out.iter().all(|v| v.half_width == FILL_HALF_WIDTH));
        // Area check: sum of triangle areas = ∫ (10 - y) dx = (8+7)/2 + (7+6)/2 = 14.
        let mut area = 0.0;
        for t in out.chunks(3) {
            let (a, b, c) = (t[0].pos, t[1].pos, t[2].pos);
            area += ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0;
        }
        assert!((area - 14.0).abs() < 1e-4);
    }
}
