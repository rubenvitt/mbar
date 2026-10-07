//! [`MacResources`]: `mbar_core::platform::Resources` backed by CoreText (gfx
//! [`TextSystem`]), the gfx [`ImageStore`], SkyLight/AppKit display and space readers and
//! the forced-event readers of the system layer.
//!
//! Keys handed to the core:
//! * [`TextKey`]: a hash of `(FontId, string)` owned by [`TextCache`], which remembers the
//!   pair so a run evicted from the gfx LRU cache is transparently re-measured when the
//!   scene is drawn again. The table is pruned only with the core's list of live keys
//!   ([`TextCache::prune_live`], fed by `Runtime::for_each_text_key`), so a key the core
//!   still draws always resolves, however long ago it was measured.
//! * [`ImageKey`]: the [`ImageId`] of the store (see [`convert::image_key`]). Pictures that
//!   are replaced in place (alias captures, `space.<n>`, media artwork) keep their id and
//!   bump a generation that feeds [`ImageInfo::hash`], so the core's change detection
//!   (`image_set_image`) sees new content.

use super::convert::{self, image_key, SceneLookup};
use crate::gfx::image::{ImageInfo as GImageInfo, ImageStore};
use crate::gfx::renderer::{Renderer, RendererError};
use crate::gfx::scene::{ImageId, TextRunId};
use crate::gfx::text::{FontId, TextSystem};
use crate::sys::{alias, displays, events, menus, spaces};
use mbar_core::components::{FontSpec, ImageSource};
use mbar_core::geometry::{Rect, Size};
use mbar_core::platform::{
    DisplayInfo, ImageError, ImageInfo, ImageKey, MenuExtra, Resources, SpaceInfo, SystemQuery,
    SystemValue, TextKey, TextMetrics,
};
use objc2::MainThreadMarker;
use objc2_core_foundation::{CFRetained, CGRect};
use objc2_core_graphics::CGImage;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Instant;

fn hash_of(h: impl Hash) -> u64 {
    let mut s = DefaultHasher::new();
    h.hash(&mut s);
    s.finish()
}

/// CG rect → core rect.
pub fn core_rect(r: &CGRect) -> Rect {
    Rect::new(
        r.origin.x as f32,
        r.origin.y as f32,
        r.size.width as f32,
        r.size.height as f32,
    )
}

// ---------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------

struct TextEntry {
    font: FontId,
    text: Box<str>,
    run: TextRunId,
    used: u64,
}

/// Default bound of remembered `(font, string)` pairs before pruning.
pub const DEFAULT_TEXT_ENTRIES: usize = 16 * 1024;

/// The gfx [`TextSystem`] plus the `TextKey → (font, string)` table the core's keys
/// resolve through.
///
/// Pruning is liveness based: once the table grows past its bound, [`needs_prune`]
/// turns true and the platform calls [`prune_live`] with the keys the core still
/// references. Live keys are always kept; among the others the least recently used go
/// first (they are re-measured on demand if the core asks for them again).
///
/// [`needs_prune`]: TextCache::needs_prune
/// [`prune_live`]: TextCache::prune_live
pub struct TextCache {
    pub system: TextSystem,
    entries: HashMap<TextKey, TextEntry>,
    clock: u64,
    max_entries: usize,
    stamps: Vec<u64>,
    live: HashSet<TextKey>,
}

impl TextCache {
    pub fn new(system: TextSystem) -> TextCache {
        TextCache {
            system,
            entries: HashMap::new(),
            clock: 0,
            max_entries: DEFAULT_TEXT_ENTRIES,
            stamps: Vec::new(),
            live: HashSet::new(),
        }
    }

    /// Bound of remembered lines (minimum 256).
    pub fn set_max_entries(&mut self, n: usize) {
        self.max_entries = n.max(256);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `Resources::text_metrics`.
    pub fn metrics(&mut self, font: &FontSpec, text: &str) -> TextMetrics {
        let fid = self.system.font(
            &font.family,
            &font.style,
            font.size,
            font.features.as_deref(),
        );
        let layout = self.system.measure(fid, text, false);
        self.clock += 1;
        let clock = self.clock;
        let mut k = hash_of((fid.0, text));
        loop {
            match self.entries.get_mut(&TextKey(k)) {
                Some(e) if e.font == fid && &*e.text == text => {
                    e.run = layout.run;
                    e.used = clock;
                    break;
                }
                // 64-bit collision: probe the next key.
                Some(_) => k = k.wrapping_add(1),
                None => {
                    self.entries.insert(
                        TextKey(k),
                        TextEntry {
                            font: fid,
                            text: text.into(),
                            run: layout.run,
                            used: clock,
                        },
                    );
                    break;
                }
            }
        }
        convert::text_metrics(TextKey(k), text.is_empty(), &layout)
    }

    /// The run of `key`, re-measured if the gfx cache evicted it.
    pub fn run(&mut self, key: TextKey) -> Option<TextRunId> {
        let e = self.entries.get_mut(&key)?;
        e.used = self.clock;
        if !self.system.contains(e.run) {
            e.run = self.system.measure(e.font, &e.text, false).run;
        }
        Some(e.run)
    }

    /// The table outgrew its bound: call [`prune_live`](Self::prune_live).
    pub fn needs_prune(&self) -> bool {
        self.entries.len() > self.max_entries
    }

    /// Shrinks the table to three quarters of its bound without ever dropping a key that
    /// `for_each_live` reports (`Runtime::for_each_text_key`, plus the scenes the window
    /// manager keeps for re-rendering). Unreferenced lines go least recently used first;
    /// if the live set alone exceeds the target, every unreferenced line is dropped and the
    /// bound grows to twice the live set.
    pub fn prune_live(&mut self, for_each_live: impl FnOnce(&mut dyn FnMut(TextKey))) {
        let target = self.max_entries * 3 / 4;
        if self.entries.len() <= target {
            return;
        }
        let mut live = std::mem::take(&mut self.live);
        live.clear();
        for_each_live(&mut |k| {
            live.insert(k);
        });
        self.stamps.clear();
        self.stamps.extend(
            self.entries
                .iter()
                .filter(|(k, _)| !live.contains(k))
                .map(|(_, e)| e.used),
        );
        let remove = self
            .entries
            .len()
            .saturating_sub(target)
            .min(self.stamps.len());
        if remove > 0 {
            let (_, cutoff, _) = self.stamps.select_nth_unstable(remove - 1);
            let cutoff = *cutoff;
            self.entries
                .retain(|k, e| e.used > cutoff || live.contains(k));
        }
        self.live = live;
        if self.entries.len() > target {
            // More live lines than the bound allows: raise it, so the next prune is not
            // due after every frame.
            self.max_entries = self.entries.len() * 2;
        }
    }
}

// ---------------------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------------------

/// The gfx [`ImageStore`] plus in-place slots (alias captures per item, `space.<n>` per
/// index, the media artwork).
pub struct ImageCache {
    pub store: ImageStore,
    generations: HashMap<ImageId, u64>,
    spaces: HashMap<u32, ImageId>,
    aliases: HashMap<u64, ImageId>,
    artwork: Option<ImageId>,
}

impl ImageCache {
    pub fn new(store: ImageStore) -> ImageCache {
        ImageCache {
            store,
            generations: HashMap::new(),
            spaces: HashMap::new(),
            aliases: HashMap::new(),
            artwork: None,
        }
    }

    fn info(&self, g: &GImageInfo, hash: u64) -> ImageInfo {
        ImageInfo {
            key: image_key(g.id),
            size: Size::new(g.width, g.height),
            hash,
        }
    }

    /// Stores `image` in `slot` (replacing in place when possible). Returns the info and
    /// whether the picture changed.
    fn put(
        &mut self,
        slot: Option<ImageId>,
        image: &CGImage,
        size: Option<(f32, f32)>,
    ) -> Option<(ImageInfo, bool)> {
        if let Some(id) = slot {
            if let Some(changed) = self.store.update_cgimage(id, image, size) {
                let gen = self.generations.entry(id).or_insert(0);
                if changed {
                    *gen += 1;
                }
                let gen = *gen;
                let g = self.store.info(id)?;
                return Some((self.info(&g, hash_of(("slot", id.0, gen))), changed));
            }
        }
        let g = self.store.insert_cgimage(image, size)?;
        self.generations.insert(g.id, 0);
        Some((self.info(&g, hash_of(("slot", g.id.0, 0u64))), true))
    }

    /// Now-playing artwork (`COVER_CHANGED`); `None` clears it.
    pub fn set_artwork(&mut self, image: Option<&CGImage>) -> Option<ImageInfo> {
        let image = image?;
        let (info, _) = self.put(self.artwork, image, None)?;
        self.artwork = Some(convert::image_id(info.key)?);
        Some(info)
    }

    /// Alias capture of item `id` with its logical size in points.
    pub fn set_alias(
        &mut self,
        id: u64,
        image: &CGImage,
        size: Option<(f32, f32)>,
    ) -> Option<(ImageInfo, bool)> {
        let slot = self.aliases.get(&id).copied();
        let (info, changed) = self.put(slot, image, size)?;
        if let Some(iid) = convert::image_id(info.key) {
            self.aliases.insert(id, iid);
        }
        Some((info, changed))
    }

    /// Frees the alias picture of item `id` (the item was removed).
    pub fn remove_alias(&mut self, id: u64) {
        if let Some(iid) = self.aliases.remove(&id) {
            self.generations.remove(&iid);
            self.store.remove(iid);
        }
    }

    /// `space.<n>` capture (1 px = 1 pt).
    pub fn set_space(&mut self, index: u32, image: &CGImage) -> Option<ImageInfo> {
        let slot = self.spaces.get(&index).copied();
        let (info, _) = self.put(slot, image, None)?;
        if let Some(iid) = convert::image_id(info.key) {
            self.spaces.insert(index, iid);
        }
        Some(info)
    }

    /// `Resources::load_image` for app icons and files.
    pub fn load(&mut self, source: &ImageSource) -> Result<ImageInfo, ImageError> {
        match source {
            ImageSource::App(name) => {
                let g = self.store.load_app_icon(name).ok_or(ImageError::NotFound)?;
                Ok(self.info(&g, hash_of(("app", g.id.0))))
            }
            ImageSource::File(path) => {
                let meta = std::fs::metadata(path).map_err(|_| ImageError::NotFound)?;
                if meta.is_dir() {
                    return Err(ImageError::NotFound);
                }
                // `CGDataProviderCreateWithFilename` fails on unreadable files.
                std::fs::File::open(path).map_err(|_| ImageError::InvalidFormat)?;
                let g = self.store.load_file(path).ok_or(ImageError::DecodeFailed)?;
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos());
                Ok(self.info(&g, hash_of((path, mtime, meta.len(), g.id.0))))
            }
            ImageSource::Space { .. } | ImageSource::MediaArtwork | ImageSource::Empty => {
                Err(ImageError::NotFound)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------------------

/// Everything the core reads synchronously, plus the GPU objects the window manager draws
/// with. Main thread only.
pub struct MacResources {
    mtm: MainThreadMarker,
    pub renderer: Renderer,
    pub text: TextCache,
    pub images: ImageCache,
    displays: Vec<DisplayInfo>,
    spaces: Vec<SpaceInfo>,
    menu_bar_visible: bool,
    /// Last SSID reported by the Wi-Fi source (forced `wifi_change` without blocking).
    pub(crate) last_ssid: Option<String>,
    /// `space_windows_change` payloads already delivered through a forced query; the
    /// matching events the system layer posts at the same time are dropped once.
    pub(crate) suppressed_space_windows: Vec<String>,
}

impl MacResources {
    /// Creates the Metal renderer, text system and image store and reads the displays.
    pub fn new(mtm: MainThreadMarker) -> Result<MacResources, RendererError> {
        let renderer = Renderer::new()?;
        let text = TextCache::new(TextSystem::new(renderer.device()));
        let images = ImageCache::new(ImageStore::new(renderer.device(), mtm));
        let mut res = MacResources {
            mtm,
            renderer,
            text,
            images,
            displays: Vec::new(),
            spaces: Vec::new(),
            menu_bar_visible: true,
            last_ssid: None,
            suppressed_space_windows: Vec::new(),
        };
        res.refresh_displays();
        Ok(res)
    }

    pub fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }

    /// Re-reads displays, spaces and the menu-bar auto-hide state (display/space/wake
    /// events, before they reach the core).
    pub fn refresh_displays(&mut self) {
        self.displays = displays::list(self.mtm)
            .into_iter()
            .filter(|d| d.adid != 0)
            .map(|d| DisplayInfo {
                id: d.id,
                adid: d.adid,
                uuid: d.uuid,
                frame: core_rect(&d.frame),
                builtin: d.builtin,
                menu_bar_height: d.menu_bar_height as f32,
                current_space: d.current_space,
            })
            .collect();
        self.spaces = spaces::all_spaces()
            .into_iter()
            .map(|s| SpaceInfo {
                id: s.id,
                display: s.adid,
                fullscreen: s.fullscreen,
            })
            .collect();
        self.menu_bar_visible = displays::menu_bar_visible();
    }

    /// Only the menu-bar visibility changed.
    pub fn refresh_menu_bar(&mut self) {
        self.menu_bar_visible = displays::menu_bar_visible();
        for d in &mut self.displays {
            d.menu_bar_height = displays::menu_bar_height(self.mtm, d.id) as f32;
        }
    }

    /// Lookup for [`convert::scene_to_drawlist`].
    pub fn lookup(&mut self) -> Lookup<'_> {
        Lookup {
            text: &mut self.text,
            images: &self.images.store,
        }
    }
}

/// [`SceneLookup`] over the text cache and image store.
pub struct Lookup<'a> {
    pub text: &'a mut TextCache,
    pub images: &'a ImageStore,
}

impl SceneLookup for Lookup<'_> {
    fn text_run(&mut self, key: TextKey) -> Option<TextRunId> {
        self.text.run(key)
    }

    fn image(&self, key: ImageKey) -> Option<ImageId> {
        let id = convert::image_id(key)?;
        self.images.info(id).map(|_| id)
    }
}

/// `space.<n>` capture through SkyLight.
fn capture_space(images: &mut ImageCache, index: u32) -> Result<ImageInfo, ImageError> {
    let img: CFRetained<CGImage> = spaces::capture_space(index).ok_or(ImageError::NotFound)?;
    images.set_space(index, &img).ok_or(ImageError::NotFound)
}

impl Resources for MacResources {
    fn text_metrics(&mut self, font: &FontSpec, text: &str) -> TextMetrics {
        self.text.metrics(font, text)
    }

    fn load_image(&mut self, source: &ImageSource) -> Result<ImageInfo, ImageError> {
        match source {
            ImageSource::Space { index, .. } => capture_space(&mut self.images, *index),
            other => self.images.load(other),
        }
    }

    fn displays(&self) -> &[DisplayInfo] {
        &self.displays
    }

    fn spaces(&self) -> &[SpaceInfo] {
        &self.spaces
    }

    fn active_display(&self) -> u32 {
        if self.displays.len() <= 1 {
            return self.displays.first().map(|d| d.adid).unwrap_or(1);
        }
        displays::active_display_adid()
    }

    fn menu_bar_visible(&self) -> bool {
        self.menu_bar_visible
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn menu_extras(&mut self) -> Option<Vec<MenuExtra>> {
        if !alias::screen_capture_preflight() {
            return None;
        }
        Some(
            alias::list_menu_extras()
                .into_iter()
                .map(|w| MenuExtra {
                    owner: w.owner,
                    name: w.name,
                    window_id: w.window_id,
                    frame: core_rect(&w.frame),
                })
                .collect(),
        )
    }

    fn query_system(&mut self, q: SystemQuery) -> Option<SystemValue> {
        match q {
            SystemQuery::Wifi => Some(SystemValue::Text(
                self.last_ssid
                    .clone()
                    .unwrap_or_else(events::wifi::current_ssid),
            )),
            SystemQuery::Volume => Some(SystemValue::Level(events::volume::read().0)),
            SystemQuery::Brightness => {
                events::brightness::read(displays::active_display_id()).map(SystemValue::Level)
            }
            SystemQuery::PowerSource => events::power::providing_power_source()
                .and_then(|p| convert::power_source(p.as_str()))
                .map(SystemValue::Power),
            SystemQuery::FrontApp => events::front_app()
                .and_then(|(name, _, _)| name)
                .map(SystemValue::Text),
            SystemQuery::SpaceWindows => {
                let infos = spaces::forced_space_windows();
                if self.suppressed_space_windows.len() > 256 {
                    self.suppressed_space_windows.clear();
                }
                self.suppressed_space_windows.extend(infos.iter().cloned());
                Some(SystemValue::SpaceWindows(infos))
            }
        }
    }

    fn accessibility_trusted(&mut self) -> bool {
        menus::is_trusted(false)
    }
}
