//! Image loading and GPU textures (`image.c:image_load`, spec §6.2).
//!
//! Sources:
//! * files via ImageIO (`CGImageSource`), cached by path and modification time;
//! * application icons `app.<bundle id>` / `app.<localized app name>` via NSWorkspace,
//!   rendered at `32·s` pixels (s = largest backing scale of all screens) and reported as
//!   32×32 pt (spec Q3);
//! * arbitrary `CGImage`s handed over by the system layer (alias captures, space
//!   captures, media artwork) with a caller-provided logical size.
//!
//! Every image is converted once to premultiplied BGRA8 and uploaded into its own
//! texture. Replacing an image creates a new texture (in-flight frames keep the old one
//! alive), so updates never race with the GPU. Main thread only.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::Hasher;
use std::time::SystemTime;

use objc2::runtime::ProtocolObject;
use objc2::MainThreadMarker;
use objc2::Message;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize, CFURL};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_image_io::CGImageSource;
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};

use super::gpu::{self, Device, Texture};
use super::scene::ImageId;

/// Logical size of application icons (points).
pub const APP_ICON_POINTS: f32 = 32.0;

/// Size and identity of a stored image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageInfo {
    pub id: ImageId,
    /// Logical size in points (before any `scale` property).
    pub width: f32,
    pub height: f32,
    /// Texture size in pixels.
    pub pixel_width: u32,
    pub pixel_height: u32,
}

struct Entry {
    texture: Texture,
    width: f32,
    height: f32,
    px: (u32, u32),
    hash: u64,
}

/// Owner of all image textures.
pub struct ImageStore {
    device: Device,
    mtm: MainThreadMarker,
    images: HashMap<ImageId, Entry>,
    next: u32,
    files: HashMap<String, (ImageId, Option<SystemTime>)>,
    apps: HashMap<String, ImageId>,
    scratch: Vec<u8>,
}

/// `~` → `$HOME` (`resolve_path`).
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}{rest}");
        }
    }
    path.to_string()
}

impl ImageStore {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, mtm: MainThreadMarker) -> Self {
        ImageStore {
            device: device.retain(),
            mtm,
            images: HashMap::new(),
            next: 1,
            files: HashMap::new(),
            apps: HashMap::new(),
            scratch: Vec::new(),
        }
    }

    /// Size/identity of `id`.
    pub fn info(&self, id: ImageId) -> Option<ImageInfo> {
        self.images.get(&id).map(|e| ImageInfo {
            id,
            width: e.width,
            height: e.height,
            pixel_width: e.px.0,
            pixel_height: e.px.1,
        })
    }

    pub(crate) fn texture(&self, id: ImageId) -> Option<&ProtocolObject<dyn MTLTexture>> {
        self.images.get(&id).map(|e| &*e.texture)
    }

    /// Load an image property value: `app.<bundle id or app name>` or a file path
    /// (`~` expanded). `space.<n>` and `media.artwork` are produced by the system layer
    /// and must be inserted with [`insert_cgimage`](Self::insert_cgimage).
    pub fn load(&mut self, value: &str) -> Option<ImageInfo> {
        match value.split_once('.') {
            Some(("app", name)) => self.load_app_icon(name),
            _ => self.load_file(value),
        }
    }

    /// Load a file through ImageIO (any format ImageIO decodes; first frame). Cached by
    /// path; reloaded when the file's modification time changes.
    pub fn load_file(&mut self, path: &str) -> Option<ImageInfo> {
        let resolved = expand_tilde(path);
        let meta = std::fs::metadata(&resolved).ok()?;
        if meta.is_dir() {
            return None;
        }
        let mtime = meta.modified().ok();
        if let Some(&(id, cached_mtime)) = self.files.get(&resolved) {
            if cached_mtime == mtime && self.images.contains_key(&id) {
                return self.info(id);
            }
        }
        let url = CFURL::from_file_path(&resolved)?;
        // SAFETY: valid file URL, no options.
        let source = unsafe { CGImageSource::with_url(&url, None) }?;
        // SAFETY: valid image source; index 0 exists if decoding succeeds (else None).
        let image = unsafe { source.image_at_index(0, None) }?;
        let (w, h) = (CGImage::width(Some(&image)), CGImage::height(Some(&image)));
        let existing = self.files.get(&resolved).map(|&(id, _)| id);
        let info = match existing {
            Some(id) if self.images.contains_key(&id) => {
                self.replace_cgimage(id, &image, w as f32, h as f32, w as u32, h as u32)?;
                self.info(id)?
            }
            _ => self.insert_scaled(&image, w as f32, h as f32, w as u32, h as u32)?,
        };
        self.files.insert(resolved, (info.id, mtime));
        Some(info)
    }

    /// Application icon for a bundle identifier or the localized name of a running app
    /// (`workspace.m:workspace_icon_for_app`). Cached by name.
    pub fn load_app_icon(&mut self, name: &str) -> Option<ImageInfo> {
        if let Some(&id) = self.apps.get(name) {
            if let Some(info) = self.info(id) {
                return Some(info);
            }
        }
        let ws = NSWorkspace::sharedWorkspace();
        let ns_name = NSString::from_str(name);
        let mut url = ws.URLForApplicationWithBundleIdentifier(&ns_name);
        if url.is_none() {
            let apps = ws.runningApplications();
            for app in apps.iter() {
                let matches = app.localizedName().is_some_and(|n| n.to_string() == name);
                if matches {
                    if let Some(bid) = app.bundleIdentifier() {
                        url = ws.URLForApplicationWithBundleIdentifier(&bid);
                    }
                    break;
                }
            }
        }
        let path = url?.path()?;
        let icon = ws.iconForFile(&path);
        let s = self.max_backing_scale();
        let side = (APP_ICON_POINTS as f64 * s).round().max(1.0);
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(side, side));
        // SAFETY: `rect` is a valid, writable NSRect; nil context and hints are allowed.
        let cg = unsafe { icon.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let px = side as u32;
        let info = self.insert_scaled(&cg, APP_ICON_POINTS, APP_ICON_POINTS, px, px)?;
        self.apps.insert(name.to_string(), info.id);
        Some(info)
    }

    /// Largest `backingScaleFactor` over all screens (≥ 1).
    pub fn max_backing_scale(&self) -> f64 {
        let screens = NSScreen::screens(self.mtm);
        screens
            .iter()
            .map(|s| s.backingScaleFactor())
            .fold(1.0f64, f64::max)
    }

    /// Store a `CGImage` with a logical size in points (`None` → 1 px = 1 pt). The texture
    /// keeps the image's pixel resolution.
    pub fn insert_cgimage(
        &mut self,
        image: &CGImage,
        size_points: Option<(f32, f32)>,
    ) -> Option<ImageInfo> {
        let (w, h) = (
            CGImage::width(Some(image)) as u32,
            CGImage::height(Some(image)) as u32,
        );
        let (lw, lh) = size_points.unwrap_or((w as f32, h as f32));
        self.insert_scaled(image, lw, lh, w, h)
    }

    /// Replace the pixels/size of `id` (alias refresh, artwork change). Returns
    /// `Some(true)` if pixels or size changed (`image_set_image` change detection),
    /// `Some(false)` if identical, `None` if `id` is unknown or conversion failed.
    pub fn update_cgimage(
        &mut self,
        id: ImageId,
        image: &CGImage,
        size_points: Option<(f32, f32)>,
    ) -> Option<bool> {
        let (w, h) = (
            CGImage::width(Some(image)) as u32,
            CGImage::height(Some(image)) as u32,
        );
        let (lw, lh) = size_points.unwrap_or((w as f32, h as f32));
        self.replace_cgimage(id, image, lw, lh, w, h)
    }

    /// Forget an image (its texture is released once no frame uses it).
    pub fn remove(&mut self, id: ImageId) {
        self.images.remove(&id);
        self.files.retain(|_, (i, _)| *i != id);
        self.apps.retain(|_, i| *i != id);
    }

    /// Drop the cached app icons (e.g. after a display scale change).
    pub fn clear_app_icons(&mut self) {
        let ids: Vec<ImageId> = self.apps.drain().map(|(_, id)| id).collect();
        for id in ids {
            self.images.remove(&id);
        }
    }

    fn insert_scaled(
        &mut self,
        image: &CGImage,
        lw: f32,
        lh: f32,
        pw: u32,
        ph: u32,
    ) -> Option<ImageInfo> {
        let (texture, hash) = self.convert(image, pw, ph)?;
        let id = ImageId(self.next);
        self.next = self.next.wrapping_add(1).max(1);
        self.images.insert(
            id,
            Entry {
                texture,
                width: lw,
                height: lh,
                px: (pw, ph),
                hash,
            },
        );
        self.info(id)
    }

    fn replace_cgimage(
        &mut self,
        id: ImageId,
        image: &CGImage,
        lw: f32,
        lh: f32,
        pw: u32,
        ph: u32,
    ) -> Option<bool> {
        if !self.images.contains_key(&id) {
            return None;
        }
        let hash = self.render_to_scratch(image, pw, ph)?;
        let e = self.images.get_mut(&id)?;
        if e.hash == hash && e.px == (pw, ph) && e.width == lw && e.height == lh {
            return Some(false);
        }
        let texture = gpu::new_texture(&self.device, MTLPixelFormat::BGRA8Unorm, pw, ph)?;
        if !gpu::upload(&texture, 0, 0, pw, ph, &self.scratch, pw as usize * 4, 4) {
            return None;
        }
        *e = Entry {
            texture,
            width: lw,
            height: lh,
            px: (pw, ph),
            hash,
        };
        Some(true)
    }

    fn convert(&mut self, image: &CGImage, pw: u32, ph: u32) -> Option<(Texture, u64)> {
        let hash = self.render_to_scratch(image, pw, ph)?;
        let texture = gpu::new_texture(&self.device, MTLPixelFormat::BGRA8Unorm, pw, ph)?;
        if !gpu::upload(&texture, 0, 0, pw, ph, &self.scratch, pw as usize * 4, 4) {
            return None;
        }
        Some((texture, hash))
    }

    /// Draw `image` stretched into a `pw × ph` premultiplied BGRA bitmap in `scratch`;
    /// returns a hash of the pixels.
    fn render_to_scratch(&mut self, image: &CGImage, pw: u32, ph: u32) -> Option<u64> {
        if pw == 0 || ph == 0 || pw > 16384 || ph > 16384 {
            return None;
        }
        let row = pw as usize * 4;
        self.scratch.clear();
        self.scratch.resize(row * ph as usize, 0);
        let space: CFRetained<CGColorSpace> = CGColorSpace::new_device_rgb()?;
        // SAFETY: `scratch` holds `row * ph` writable bytes and outlives the context,
        // which is dropped before this function returns; 8 bpc premultiplied BGRA in
        // DeviceRGB is a supported bitmap format.
        let ctx = unsafe {
            CGBitmapContextCreate(
                self.scratch.as_mut_ptr() as *mut c_void,
                pw as usize,
                ph as usize,
                8,
                row,
                Some(&space),
                CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0,
            )
        }?;
        let same_size =
            CGImage::width(Some(image)) as u32 == pw && CGImage::height(Some(image)) as u32 == ph;
        CGContext::set_interpolation_quality(
            Some(&ctx),
            if same_size {
                CGInterpolationQuality::None
            } else {
                CGInterpolationQuality::High
            },
        );
        let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(pw as f64, ph as f64));
        CGContext::draw_image(Some(&ctx), rect, Some(image));
        CGContext::flush(Some(&ctx));
        drop(ctx);
        let mut h = DefaultHasher::new();
        h.write(&self.scratch);
        Some(h.finish())
    }
}
