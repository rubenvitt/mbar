//! CoreText text system: SketchyBar-compatible font resolution, CTLine layout with an
//! LRU cache, and rasterization of runs into GPU atlases.
//!
//! * Fonts are resolved exactly like `font.c:font_create_ctfont` (descriptor with family,
//!   style and size attributes, optional feature settings, `CTFontCreateWithFontDescriptor`)
//!   so CoreText's descriptor matching and CTLine cascade substitute identically.
//! * [`TextSystem::measure`] lays out `(font, string)` once (`text.c:text_prepare_line`) and
//!   returns a [`TextLayout`] with the ink/typographic metrics and a [`TextRunId`].
//! * Runs are rasterized lazily by the renderer, once per backing scale, into an A8 mask
//!   atlas (tinted in the shader, so colour changes never re-rasterize) or — when the run
//!   contains colour glyphs (`kCTFontColorGlyphsTrait`, e.g. emoji) — into a BGRA atlas
//!   keyed additionally by the text colour.
//!
//! Run lifetime: a run stays valid while it is used. Runs untouched (by `measure` or by
//! rendering) for a long time are evicted once the cache exceeds its capacity. The
//! integration layer should therefore call `measure` for each text primitive while
//! building a [`DrawList`](super::scene::DrawList) — a cache hit is a hash lookup without
//! allocation — and always use the returned id. Unknown ids are skipped by the renderer.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use objc2::runtime::ProtocolObject;
use objc2::Message;
use objc2_core_foundation::{
    CFArray, CFAttributedString, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
    CFURL,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_core_text::{
    kCTFontAttributeName, kCTFontFamilyNameAttribute, kCTFontFeatureSelectorIdentifierKey,
    kCTFontFeatureSettingsAttribute, kCTFontFeatureTypeIdentifierKey, kCTFontOpenTypeFeatureTag,
    kCTFontOpenTypeFeatureValue, kCTFontSizeAttribute, kCTFontStyleNameAttribute,
    kCTForegroundColorFromContextAttributeName, CTFont, CTFontDescriptor,
    CTFontManagerRegisterFontsForURL, CTFontManagerScope, CTFontSymbolicTraits, CTLine,
    CTLineBoundsOptions, CTRun,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};

use super::atlas::{Atlas, InsertOutcome, Placement};
use super::gpu::{self, Device, Texture};
use super::scene::{Rgba, TextRunId};
use super::util::{self, FontFeature, PixelBounds};

/// Handle of a resolved font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontId(pub u32);

/// Metrics of a laid-out `(font, string)` (`text.c:text_prepare_line`, spec §4.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextLayout {
    pub run: TextRunId,
    /// `typographic ? typographic_width : ink_width` as requested in `measure`.
    pub width: f32,
    /// `(u32)(glyph_path_bounds.w + 1.5)`; empty string → 1.
    pub ink_width: f32,
    /// `(u32)(glyph_path_bounds.h + 1.5)`; empty string → 1.
    pub ink_height: f32,
    /// `(u32)(CTLineGetTypographicBounds + 0.5)`.
    pub typographic_width: f32,
    /// `(i32)(bounds.x + 0.5)` / `(i32)(bounds.y + 0.5)` of the ink box (y up, relative
    /// to the pen).
    pub ink_x: f32,
    pub ink_y: f32,
    /// Typographic ascent/descent (font-wide, descent positive).
    pub ascent: f32,
    pub descent: f32,
    /// The run contains colour glyphs and is rasterized as BGRA.
    pub has_color_glyphs: bool,
}

/// A rasterized run inside an atlas page, as the renderer needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GlyphQuad {
    pub color: bool,
    pub page: u32,
    /// Normalized texture coordinates `u0, v0, u1, v1`.
    pub uv: [f32; 4],
    /// Quad offset from the pen origin (points, y down) and size.
    pub dx: f32,
    pub dy: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    run: TextRunId,
    scale_bits: u32,
    /// Quantized text colour for colour runs, 0 for masks.
    color: u32,
}

struct FontEntry {
    family: Box<str>,
    style: Box<str>,
    size_bits: u32,
    features: Option<Box<str>>,
    font: CFRetained<CTFont>,
}

struct Run {
    font: FontId,
    text: Rc<str>,
    line: CFRetained<CTLine>,
    layout: TextLayout,
    ink: [f64; 4],
    advance: f64,
    ascent: f64,
    descent: f64,
    last_used: u64,
}

const MASK_PAGE: u32 = 1024;
const COLOR_PAGE: u32 = 512;
const MAX_PAGES: usize = 4;
/// Points of padding around a rasterized run (antialiasing fringe, glyph overhang).
const RASTER_PAD: f64 = 1.0;

/// Default number of cached runs before LRU eviction kicks in.
pub const DEFAULT_RUN_CAPACITY: usize = 2048;

/// CoreText layout cache plus glyph-run atlases. Main thread only.
pub struct TextSystem {
    device: Device,
    fonts: Vec<FontEntry>,
    font_index: HashMap<u64, Vec<u32>>,
    layouts: HashMap<FontId, HashMap<Rc<str>, TextRunId>>,
    runs: HashMap<TextRunId, Run>,
    next_run: u64,
    clock: u64,
    capacity: usize,
    mask_atlas: Atlas<GlyphKey>,
    color_atlas: Atlas<GlyphKey>,
    mask_pages: Vec<Option<Texture>>,
    color_pages: Vec<Option<Texture>>,
    scratch: Vec<u8>,
    stamps: Vec<u64>,
    font_smoothing: bool,
}

fn font_hash(family: &str, style: &str, size: f32, features: Option<&str>) -> u64 {
    let mut h = DefaultHasher::new();
    family.hash(&mut h);
    style.hash(&mut h);
    size.to_bits().hash(&mut h);
    features.hash(&mut h);
    h.finish()
}

/// `font.c:font_create_ctfont`.
fn create_ct_font(
    family: &str,
    style: &str,
    size: f32,
    features: Option<&str>,
) -> CFRetained<CTFont> {
    let family_s = CFString::from_str(family);
    let style_s = CFString::from_str(style);
    let size_n = CFNumber::new_f32(size);
    // SAFETY: the attribute keys are CoreText's exported constants (valid CFStrings for
    // the whole process lifetime).
    let (k_family, k_style, k_size) = unsafe {
        (
            kCTFontFamilyNameAttribute,
            kCTFontStyleNameAttribute,
            kCTFontSizeAttribute,
        )
    };
    let attrs = CFDictionary::<CFString, CFType>::from_slices(
        &[k_family, k_style, k_size],
        &[&family_s, &style_s, &size_n],
    );
    // SAFETY: `attrs` is a CFDictionary<CFString, CFType> of font descriptor attributes.
    let mut desc = unsafe { CTFontDescriptor::with_attributes(attrs.as_opaque()) };

    if let Some(list) = features {
        let mut settings: Vec<CFRetained<CFDictionary<CFString, CFType>>> = Vec::new();
        for f in util::parse_features(list) {
            let d = match f {
                FontFeature::Aat { kind, selector } => {
                    let n = CFNumber::new_i32(kind);
                    let m = CFNumber::new_i32(selector);
                    // SAFETY: CoreText constant keys (see above).
                    let (kt, ks) = unsafe {
                        (
                            kCTFontFeatureTypeIdentifierKey,
                            kCTFontFeatureSelectorIdentifierKey,
                        )
                    };
                    CFDictionary::<CFString, CFType>::from_slices(&[kt, ks], &[&n, &m])
                }
                FontFeature::OpenType { tag, value } => {
                    let tag = CFString::from_str(&String::from_utf8_lossy(&tag));
                    let v = CFNumber::new_i32(value);
                    // SAFETY: CoreText constant keys (see above).
                    let (kt, kv) =
                        unsafe { (kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue) };
                    CFDictionary::<CFString, CFType>::from_slices(&[kt, kv], &[&tag, &v])
                }
            };
            settings.push(d);
        }
        if !settings.is_empty() {
            let array = CFArray::<CFDictionary<CFString, CFType>>::from_retained_objects(&settings);
            // SAFETY: CoreText constant key (see above).
            let k = unsafe { kCTFontFeatureSettingsAttribute };
            let extra = CFDictionary::<CFString, CFType>::from_slices(&[k], &[&array]);
            // SAFETY: `extra` is a valid attribute dictionary whose value is an array of
            // feature setting dictionaries, as `kCTFontFeatureSettingsAttribute` expects.
            desc = unsafe { desc.copy_with_attributes(extra.as_opaque()) };
        }
    }
    // SAFETY: a null matrix is allowed (identity); size 0.0 keeps the descriptor's size.
    unsafe { CTFont::with_font_descriptor(&desc, 0.0, std::ptr::null()) }
}

/// `true` if any glyph run of `line` uses a font with colour glyphs.
fn line_has_color_glyphs(line: &CTLine) -> bool {
    // SAFETY: `line` is a valid CTLine; the returned array contains CTRun objects.
    let runs = unsafe { line.glyph_runs() };
    // SAFETY: CTLineGetGlyphRuns returns a CFArray of CTRunRef.
    let runs: &CFArray<CTRun> = unsafe { runs.cast_unchecked() };
    // SAFETY: CoreText constant key.
    let key = unsafe { kCTFontAttributeName };
    for run in runs.iter() {
        // SAFETY: `run` is a valid CTRun; its attributes are a CFString-keyed dictionary.
        let attrs = unsafe { run.attributes() };
        // SAFETY: run attribute dictionaries are keyed by CFString with CFType values.
        let attrs: &CFDictionary<CFString, CFType> = unsafe { attrs.cast_unchecked() };
        if let Some(font) = attrs.get(key).and_then(|v| v.downcast::<CTFont>().ok()) {
            // SAFETY: `font` is a valid CTFont.
            let traits = unsafe { font.symbolic_traits() };
            if traits.contains(CTFontSymbolicTraits::TraitColorGlyphs) {
                return true;
            }
        }
    }
    false
}

impl TextSystem {
    /// Create a text system uploading into textures of `device`
    /// (use [`Renderer::device`](super::renderer::Renderer::device)).
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Self {
        TextSystem {
            device: device.retain(),
            fonts: Vec::new(),
            font_index: HashMap::new(),
            layouts: HashMap::new(),
            runs: HashMap::new(),
            next_run: 1,
            clock: 0,
            capacity: DEFAULT_RUN_CAPACITY,
            mask_atlas: Atlas::new(MASK_PAGE, MASK_PAGE, MAX_PAGES),
            color_atlas: Atlas::new(COLOR_PAGE, COLOR_PAGE, MAX_PAGES),
            mask_pages: Vec::new(),
            color_pages: Vec::new(),
            scratch: Vec::new(),
            stamps: Vec::new(),
            font_smoothing: false,
        }
    }

    /// Maximum number of cached runs (minimum 16).
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(16);
    }

    /// `bar.font_smoothing` (`CGContextSetAllowsFontSmoothing`). Only affects runs
    /// rasterized afterwards (SketchyBar also only applies it to new contexts).
    pub fn set_font_smoothing(&mut self, on: bool) {
        self.font_smoothing = on;
    }

    /// Resolve a font from SketchyBar's `(family, style, size, features)`; cached.
    /// A cache hit does not allocate.
    pub fn font(&mut self, family: &str, style: &str, size: f32, features: Option<&str>) -> FontId {
        let hash = font_hash(family, style, size, features);
        if let Some(ids) = self.font_index.get(&hash) {
            for &id in ids {
                let f = &self.fonts[id as usize];
                if &*f.family == family
                    && &*f.style == style
                    && f.size_bits == size.to_bits()
                    && f.features.as_deref() == features
                {
                    return FontId(id);
                }
            }
        }
        let font = create_ct_font(family, style, size, features);
        let id = self.fonts.len() as u32;
        self.fonts.push(FontEntry {
            family: family.into(),
            style: style.into(),
            size_bits: size.to_bits(),
            features: features.map(Into::into),
            font,
        });
        self.font_index.entry(hash).or_default().push(id);
        FontId(id)
    }

    /// The CoreText font behind `id`.
    pub fn ct_font(&self, id: FontId) -> Option<&CTFont> {
        self.fonts.get(id.0 as usize).map(|f| &*f.font)
    }

    /// Lay out `text` with `font` (cached by `(font, text)`). `typographic_width` selects
    /// what `TextLayout::width` reports (`font.typographical_width`).
    pub fn measure(&mut self, font: FontId, text: &str, typographic_width: bool) -> TextLayout {
        self.clock += 1;
        let clock = self.clock;
        if let Some(id) = self.layouts.get(&font).and_then(|m| m.get(text)).copied() {
            if let Some(run) = self.runs.get_mut(&id) {
                run.last_used = clock;
                let mut l = run.layout;
                l.width = if typographic_width {
                    l.typographic_width
                } else {
                    l.ink_width
                };
                return l;
            }
        }
        let layout = self.create_run(font, text, clock);
        if self.runs.len() > self.capacity {
            self.evict();
        }
        let mut l = layout;
        l.width = if typographic_width {
            l.typographic_width
        } else {
            l.ink_width
        };
        l
    }

    /// Ink width of the first `max_chars` code points (`max_chars` truncation, §4.3 step
    /// 8): `(u32)(glyph_path_bounds(prefix).w + 1.5)`.
    pub fn truncated_width(&mut self, font: FontId, text: &str, max_chars: u32) -> f32 {
        let prefix = util::char_prefix(text, max_chars);
        self.measure(font, prefix, false).ink_width
    }

    /// Whether `run` is still cached.
    pub fn contains(&self, run: TextRunId) -> bool {
        self.runs.contains_key(&run)
    }

    /// Number of cached runs.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// `--load-font`: register a font file for this process (`CTFontManagerRegisterFontsForURL`,
    /// process scope). Accepts `file://` URLs and plain paths (`~` expanded).
    pub fn register_font(path: &str) -> bool {
        let url = if path.contains("://") {
            CFURL::from_string(None, &CFString::from_str(path), None)
        } else {
            let expanded = if let Some(rest) = path.strip_prefix('~') {
                match std::env::var("HOME") {
                    Ok(home) => format!("{home}{rest}"),
                    Err(_) => path.to_string(),
                }
            } else {
                path.to_string()
            };
            CFURL::from_file_path(expanded)
        };
        let Some(url) = url else { return false };
        // SAFETY: `url` is a valid CFURL; a null error out-pointer is allowed.
        unsafe {
            CTFontManagerRegisterFontsForURL(
                &url,
                CTFontManagerScope::Process,
                std::ptr::null_mut(),
            )
        }
    }

    fn create_run(&mut self, font: FontId, text: &str, clock: u64) -> TextLayout {
        let id = TextRunId(self.next_run);
        self.next_run += 1;
        let ct_font = match self.fonts.get(font.0 as usize) {
            Some(f) => f.font.clone(),
            // Unknown font id: fall back to a default-resolved font so callers still get
            // a valid run.
            None => create_ct_font("", "", 14.0, None),
        };
        let string = CFString::from_str(text);
        // SAFETY: CoreText constant keys.
        let (k_font, k_ctx) = unsafe {
            (
                kCTFontAttributeName,
                kCTForegroundColorFromContextAttributeName,
            )
        };
        let attrs = CFDictionary::<CFString, CFType>::from_slices(
            &[k_font, k_ctx],
            &[&ct_font, CFBoolean::new(true)],
        );
        // SAFETY: valid string and attribute dictionary; default allocator.
        let attributed =
            unsafe { CFAttributedString::new(None, Some(&string), Some(attrs.as_opaque())) };
        let line = match attributed {
            // SAFETY: valid attributed string.
            Some(a) => unsafe { CTLine::with_attributed_string(&a) },
            None => {
                // Allocation failure; build an empty line instead.
                let empty = CFString::from_str("");
                // SAFETY: as above.
                let a =
                    unsafe { CFAttributedString::new(None, Some(&empty), Some(attrs.as_opaque())) }
                        .expect("CFAttributedStringCreate failed");
                // SAFETY: valid attributed string.
                unsafe { CTLine::with_attributed_string(&a) }
            }
        };
        let mut ascent: f64 = 0.0;
        let mut descent: f64 = 0.0;
        // SAFETY: the out-pointers are valid for writes; leading may be null.
        let typo_w =
            unsafe { line.typographic_bounds(&mut ascent, &mut descent, std::ptr::null_mut()) };
        // SAFETY: valid line.
        let ink = unsafe { line.bounds_with_options(CTLineBoundsOptions::UseGlyphPathBounds) };
        let has_color = line_has_color_glyphs(&line);
        let ink_w = util::ink_extent(ink.size.width);
        let ink_h = util::ink_extent(ink.size.height);
        let layout = TextLayout {
            run: id,
            width: ink_w as f32,
            ink_width: ink_w as f32,
            ink_height: ink_h as f32,
            typographic_width: util::round_half_up_u32(typo_w) as f32,
            ink_x: util::trunc_half_i32(ink.origin.x) as f32,
            ink_y: util::trunc_half_i32(ink.origin.y) as f32,
            ascent: ascent as f32,
            descent: descent as f32,
            has_color_glyphs: has_color,
        };
        let text: Rc<str> = Rc::from(text);
        self.layouts
            .entry(font)
            .or_default()
            .insert(text.clone(), id);
        self.runs.insert(
            id,
            Run {
                font,
                text,
                line,
                layout,
                ink: [ink.origin.x, ink.origin.y, ink.size.width, ink.size.height],
                advance: typo_w,
                ascent,
                descent,
                last_used: clock,
            },
        );
        layout
    }

    /// Drop the least recently used quarter of the runs.
    fn evict(&mut self) {
        let target = self.capacity * 3 / 4;
        let remove = self.runs.len().saturating_sub(target);
        if remove == 0 {
            return;
        }
        self.stamps.clear();
        self.stamps.extend(self.runs.values().map(|r| r.last_used));
        let (_, cutoff, _) = self.stamps.select_nth_unstable(remove - 1);
        let cutoff = *cutoff;
        let layouts = &mut self.layouts;
        self.runs.retain(|_, r| {
            if r.last_used <= cutoff {
                if let Some(m) = layouts.get_mut(&r.font) {
                    m.remove(&*r.text);
                }
                false
            } else {
                true
            }
        });
        self.layouts.retain(|_, m| !m.is_empty());
    }

    /// Texture of an atlas page.
    pub(crate) fn page_texture(
        &self,
        color: bool,
        page: u32,
    ) -> Option<&ProtocolObject<dyn MTLTexture>> {
        let pages = if color {
            &self.color_pages
        } else {
            &self.mask_pages
        };
        pages.get(page as usize).and_then(|p| p.as_deref())
    }

    /// Find or rasterize `run` at `scale` for frame `serial` (`completed` = newest frame
    /// the GPU finished). `color` only matters for colour-glyph runs.
    pub(crate) fn glyph_quad(
        &mut self,
        run_id: TextRunId,
        scale: f32,
        color: Rgba,
        serial: u64,
        completed: u64,
    ) -> Option<GlyphQuad> {
        let run = self.runs.get_mut(&run_id)?;
        run.last_used = self.clock;
        let is_color = run.layout.has_color_glyphs;
        let s = scale as f64;
        let pb = util::raster_bounds(
            run.ink[0],
            run.ink[1],
            run.ink[2],
            run.ink[3],
            run.advance,
            run.ascent,
            run.descent,
            RASTER_PAD,
            s,
        )?;
        let key = GlyphKey {
            run: run_id,
            scale_bits: scale.to_bits(),
            color: if is_color { util::rgba_key(color) } else { 0 },
        };
        let atlas = if is_color {
            &mut self.color_atlas
        } else {
            &mut self.mask_atlas
        };
        if let Some(pl) = atlas.get(&key, serial) {
            return Some(self.make_quad(is_color, pl, pb, scale));
        }
        let line = run.line.clone();
        let (w, h) = (pb.width(), pb.height());
        let bpp = if is_color { 4 } else { 1 };
        let row = w as usize * bpp;
        self.scratch.clear();
        self.scratch.resize(row * h as usize, 0);
        if !rasterize_line(
            &line,
            &mut self.scratch,
            w,
            h,
            row,
            is_color,
            color,
            pb,
            s,
            self.font_smoothing,
        ) {
            return None;
        }
        let atlas = if is_color {
            &mut self.color_atlas
        } else {
            &mut self.mask_atlas
        };
        let (pl, outcome) = atlas.insert(key, w, h, serial, completed)?;
        self.apply_outcome(is_color, outcome);
        let tex = self.page_texture(is_color, pl.page)?;
        if !gpu::upload(tex, pl.x, pl.y, w, h, &self.scratch, row, bpp) {
            return None;
        }
        Some(self.make_quad(is_color, pl, pb, scale))
    }

    fn apply_outcome(&mut self, color: bool, outcome: InsertOutcome) {
        let format = if color {
            MTLPixelFormat::BGRA8Unorm
        } else {
            MTLPixelFormat::R8Unorm
        };
        let device = self.device.clone();
        let pages = if color {
            &mut self.color_pages
        } else {
            &mut self.mask_pages
        };
        if let Some(i) = outcome.removed {
            if let Some(slot) = pages.get_mut(i as usize) {
                *slot = None;
            }
        }
        if let Some((i, w, h)) = outcome.added {
            let i = i as usize;
            if pages.len() <= i {
                pages.resize(i + 1, None);
            }
            pages[i] = gpu::new_texture(&device, format, w, h);
        }
        // `cleared` pages keep their texture; stale texels are overwritten on reuse.
    }

    fn make_quad(&self, color: bool, pl: Placement, pb: PixelBounds, scale: f32) -> GlyphQuad {
        let (pw, ph) = self
            .page_texture(color, pl.page)
            .map(|t| (t.width() as f32, t.height() as f32))
            .unwrap_or((1.0, 1.0));
        GlyphQuad {
            color,
            page: pl.page,
            uv: [
                pl.x as f32 / pw,
                pl.y as f32 / ph,
                (pl.x + pl.w) as f32 / pw,
                (pl.y + pl.h) as f32 / ph,
            ],
            dx: pb.x0 as f32 / scale,
            dy: -(pb.y1 as f32) / scale,
            w: pl.w as f32 / scale,
            h: pl.h as f32 / scale,
        }
    }
}

/// Draw `line` into `buf` (top row first) so that the pen lands at pixel `(-x0, -y0)`
/// (y up) of a `w × h` bitmap. Mask runs: alpha-only; colour runs: BGRA premultiplied
/// with `color` as the context fill colour (used by non-colour glyphs).
#[allow(clippy::too_many_arguments)]
fn rasterize_line(
    line: &CTLine,
    buf: &mut [u8],
    w: u32,
    h: u32,
    row: usize,
    is_color: bool,
    color: Rgba,
    pb: PixelBounds,
    scale: f64,
    font_smoothing: bool,
) -> bool {
    let (space, info) = if is_color {
        (
            CGColorSpace::new_device_rgb(),
            CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0,
        )
    } else {
        (None, CGImageAlphaInfo::Only.0)
    };
    if is_color && space.is_none() {
        return false;
    }
    if buf.len() < row * h as usize {
        return false;
    }
    // SAFETY: `buf` is a writable buffer of `row * h` bytes that outlives the context
    // (the context is dropped at the end of this function); the format combination
    // (8 bpc alpha-only without colour space, or 8 bpc BGRA premultiplied in DeviceRGB)
    // is supported by CGBitmapContextCreate.
    let ctx = unsafe {
        CGBitmapContextCreate(
            buf.as_mut_ptr() as *mut c_void,
            w as usize,
            h as usize,
            8,
            row,
            space.as_deref(),
            info,
        )
    };
    let Some(ctx) = ctx else { return false };
    let c = Some(&*ctx);
    CGContext::set_allows_font_smoothing(c, font_smoothing);
    CGContext::set_should_antialias(c, true);
    if is_color {
        let [r, g, b, a] = util::unpremultiply(color);
        CGContext::set_rgb_fill_color(c, r as f64, g as f64, b as f64, a as f64);
    } else {
        CGContext::set_gray_fill_color(c, 1.0, 1.0);
    }
    CGContext::scale_ctm(c, scale, scale);
    CGContext::set_text_position(c, -(pb.x0 as f64) / scale, -(pb.y0 as f64) / scale);
    // SAFETY: valid line and bitmap context.
    unsafe { line.draw(&ctx) };
    CGContext::flush(c);
    true
}
