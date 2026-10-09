//! The boundary between the core and a platform (`docs/DESIGN-CORE.md`, "Platform
//! surface"; `docs/ARCHITECTURE.md`, "Data flow").
//!
//! * [`Resources`]: synchronous queries the core needs while handling an input (text
//!   metrics, image loading, displays/spaces, system values, the clock).
//! * [`Input`]: everything that happens to the core.
//! * [`Effect`]: everything the core asks the platform to do.
//! * [`FrameOutput`]: windows to (re)draw, produced by `Runtime::frame`.
//!
//! [`HeadlessResources`] is the deterministic implementation used on Linux and in tests.

use crate::components::{FontSpec, ImageSource};
use crate::geometry::{Point, Rect, Size};
use crate::item::ItemId;
use crate::scene::Scene;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;

/// Opaque handle of a measured text line; renderers resolve it to a rasterized run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextKey(pub u64);

/// Opaque handle of a decoded picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageKey(pub u64);

/// Line metrics (`CTLineGetTypographicBounds` + `CTLineGetBoundsWithOptions(GlyphPathBounds)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    pub key: TextKey,
    /// Glyph-path (ink) bounds relative to the pen origin on the baseline, CG orientation
    /// (y up). An empty string has an empty (zero) rect.
    pub ink: Rect,
    /// Typographic advance width.
    pub typographic_width: f32,
    /// Typographic ascent / descent (descent positive).
    pub ascent: f32,
    pub descent: f32,
}

/// A loaded picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageInfo {
    pub key: ImageKey,
    /// Logical size in points before `scale` (app icons 32×32, files 1 px = 1 pt, …).
    pub size: Size,
    /// Content hash (`image_set_image` skips unchanged pictures).
    pub hash: u64,
}

/// Why [`Resources::load_image`] failed (mapped to SketchyBar's messages in
/// `components/image.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// File missing / is a directory; unknown app; space not found.
    NotFound,
    /// `CGDataProviderCreateWithFilename` failed.
    InvalidFormat,
    /// PNG/JPEG decode failed (`Could not open image file at: …`, still "success").
    DecodeFailed,
}

/// One display (`bar.md` §6.1).
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    /// `CGDirectDisplayID` (or the platform's output id).
    pub id: u32,
    /// Arrangement id, 1-based (`display_arrangement`).
    pub adid: u32,
    /// UUID string; `None` prints `<unknown>` in `--query displays`.
    pub uuid: Option<String>,
    /// Global frame in points, top-left origin (`CGDisplayBounds`).
    pub frame: Rect,
    /// `CGDisplayIsBuiltin` (notch settings apply).
    pub builtin: bool,
    /// Menu bar height on this display (`display_menu_bar_rect`, `bar.md` §6.2).
    pub menu_bar_height: f32,
    /// Current space id (`dsid`) of this display; 0 = unknown.
    pub current_space: u64,
}

/// One Mission Control space, in `SLSCopyManagedDisplaySpaces` order (the 1-based position
/// in the platform's list is the SID / mission-control index).
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceInfo {
    /// `dsid`.
    pub id: u64,
    /// `adid` of the display owning the space.
    pub display: u32,
    /// Native fullscreen space (`SLSSpaceGetType == 4`).
    pub fullscreen: bool,
}

/// A menu-bar extra window (`alias.c:get_menu_item_list`), sorted by x descending.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuExtra {
    pub owner: String,
    pub name: String,
    pub window_id: u32,
    pub frame: Rect,
}

/// `power_source_change` states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    Ac,
    Battery,
}

impl PowerSource {
    /// INFO text: `AC` / `BATTERY`.
    pub fn as_str(self) -> &'static str {
        match self {
            PowerSource::Ac => "AC",
            PowerSource::Battery => "BATTERY",
        }
    }
}

/// Synchronous system reads used by forced events (`--update`, `--trigger <builtin>`;
/// `events.md` §8.2/§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemQuery {
    /// Current SSID (`wifi_change`).
    Wifi,
    /// Default output volume 0..1, muted reads 0 (`volume_change`).
    Volume,
    /// Brightness of the active display 0..1 (`brightness_change`).
    Brightness,
    /// Providing power source (`power_source_change`).
    PowerSource,
    /// Front application name (`front_app_switched`).
    FrontApp,
    /// One `space_windows_change` INFO per space of every display (if tracking started).
    SpaceWindows,
}

/// Answer to a [`SystemQuery`].
#[derive(Debug, Clone, PartialEq)]
pub enum SystemValue {
    Text(String),
    Level(f32),
    Power(PowerSource),
    /// Ready-made INFO payloads (`events.md` §5.11 format), one per space.
    SpaceWindows(Vec<String>),
}

/// Synchronous services the platform provides to the core.
pub trait Resources {
    /// Measures `text` in `font` (CoreText on macOS).
    fn text_metrics(&mut self, font: &FontSpec, text: &str) -> TextMetrics;
    /// Loads a picture (`app.`, `space.`, files). `MediaArtwork`/`Empty` never reach here.
    fn load_image(&mut self, source: &ImageSource) -> Result<ImageInfo, ImageError>;
    /// Active displays in arrangement order.
    fn displays(&self) -> &[DisplayInfo];
    /// All spaces in mission-control order.
    fn spaces(&self) -> &[SpaceInfo];
    /// `display_active_display_adid()` (cursor display or menu-bar display, `bar.md` §6.3).
    fn active_display(&self) -> u32;
    /// `display_menu_bar_visible()` (menu bar not auto-hidden).
    fn menu_bar_visible(&self) -> bool;
    /// Monotonic clock used for timers, animations and the scroll throttle.
    fn now(&self) -> Instant;
    /// Menu-bar extras for `--query default_menu_items` / alias lookup; `None` when Screen
    /// Recording permission is missing.
    fn menu_extras(&mut self) -> Option<Vec<MenuExtra>>;
    /// Synchronous system read for forced events; `None` = no value / not available.
    fn query_system(&mut self, q: SystemQuery) -> Option<SystemValue>;
    /// Whether the Accessibility permission (needed for `app_menu`, `--menu` and
    /// `--query menus`) is granted. Platforms without such a permission return `true`.
    fn accessibility_trusted(&mut self) -> bool {
        true
    }
}

/// Identifies the request an IPC reply belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReplyToken(pub u64);

/// A platform window (one per bar per display, one per open popup).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WindowKey {
    /// The bar on arrangement display `adid`.
    Bar(u32),
    /// The popup hosted by an item.
    Popup(ItemId),
}

/// Mouse buttons as reported by mouse-up (`get_type_description`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Other,
}

/// Kinds of mouse input (`events.md` §6). There is no mouse-down handling: clicks fire on
/// release. `Moved` is needed because mbar has no per-item windows: item enter/exit is
/// synthesized from pointer motion inside bar/popup windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseKind {
    Up {
        button: MouseButton,
        button_code: u32,
    },
    Dragged,
    Moved,
    /// The pointer entered `window` (bar or popup window).
    Entered,
    /// The pointer left `window`.
    Exited,
    /// Vertical delta in lines (`kCGScrollWheelEventDeltaAxis1`).
    Scrolled {
        delta: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseInput {
    pub kind: MouseKind,
    /// Global location, points, top-left origin.
    pub point: Point,
    /// Window under the event, if it is one of ours.
    pub window: Option<WindowKey>,
    /// Raw `CGEventFlags` truncated to u32 (printed as `modfier_code`).
    pub modifiers: u32,
}

/// OS notifications (`events.md` §2).
#[derive(Debug, Clone, PartialEq)]
pub enum OsEvent {
    /// `front_app_switched` (INFO = name if any). `bundle_id` feeds the `front_app` provider.
    FrontAppSwitched {
        name: Option<String>,
        bundle_id: Option<String>,
    },
    /// `SPACE_CHANGED` (non-forced handling).
    SpaceChanged,
    /// `DISPLAY_CHANGED` (active display notification).
    ActiveDisplayChanged,
    /// Menu bar auto-hide toggled: resize bars, no script event.
    MenuBarHiddenChanged,
    SystemWillSleep,
    SystemWoke,
    /// Volume 0..1 (already de-duplicated by the platform with the 0.01 threshold).
    VolumeChanged(f32),
    /// Brightness 0..1 (de-duplicated, global threshold).
    BrightnessChanged(f32),
    PowerSourceChanged(PowerSource),
    /// SSID (may be empty).
    WifiChanged(String),
    /// Ready-made `media_change` INFO (`events.md` §5.10), already de-duplicated.
    MediaChanged(String),
    /// Now-playing artwork (`COVER_CHANGED`).
    MediaArtwork(Option<ImageInfo>),
    /// Ready-made `space_windows_change` INFO for one space.
    SpaceWindowsChanged(String),
    /// A distributed notification registered by `--add event <name> <notification>`.
    DistributedNotification {
        name: String,
        info: Option<String>,
    },
    /// Hotload watcher saw a change in the config directory (rate limiting is the
    /// platform's job: one per 2^30 ns).
    ConfigChanged,
    /// Alias capture gating (SkyLight 1322/905/904/1401/1508): `true` = disabled.
    CaptureDisabled(bool),
}

/// Requests from `mbar-lua` (in-process scripting). The Lua API maps onto commands plus
/// in-process event handlers; the exact surface belongs to `mbar-lua`.
#[derive(Debug, Clone, PartialEq)]
pub enum LuaRequest {
    /// A command batch (`mbar.set(...)` etc. compiled to argv); the response text is
    /// delivered to `callback` (if any) as `Effect::LuaCallback` with env `RESPONSE`.
    Command {
        args: Vec<String>,
        callback: Option<u64>,
    },
    /// Subscribe an item to events with an in-process handler instead of a script
    /// (`script` then shows `lua:<handler>` in `--query`).
    Subscribe {
        item: String,
        events: Vec<String>,
        handler: u64,
    },
    /// Item-less ("global") in-process handler (Lua `mbar.aerospace.on`): whenever one of
    /// `events` is triggered, the runtime also emits `Effect::LuaCallback { handler, env }`
    /// with the event's variables and `SENDER=<event>` (no `NAME`), independent of any
    /// item's `updates` / `drawing` state. Cleared by `--reload` (the Lua config re-runs
    /// and registers again). Registering for an `aerospace_*` event starts the AeroSpace
    /// connection and, when the state for that event is known, delivers it to `handler`
    /// right away.
    On { events: Vec<String>, handler: u64 },
}

/// Everything that happens to the core.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// One IPC request (argv without argv[0]).
    Message {
        args: Vec<String>,
        reply: ReplyToken,
    },
    Event(OsEvent),
    Mouse(MouseInput),
    /// `next_deadline()` reached (routine clock, providers, delayed wake, …).
    Timer,
    /// A spawned script exited (stats / Lua `exec` output).
    ScriptFinished {
        pid: u32,
        item: Option<String>,
        output: Option<String>,
    },
    /// A native provider produced a sample for `item`.
    ProviderSample {
        item: ItemId,
        values: Vec<(String, String)>,
    },
    /// Result of `PlatformRequest::CaptureAlias`. `image: None` with `disabled` = capture
    /// currently suspended (keep the old picture).
    AliasImage {
        item: ItemId,
        window_id: u32,
        frame: Rect,
        image: Option<ImageInfo>,
        disabled: bool,
    },
    /// Display reconfiguration (add/remove/move/resize) or wake rebuild trigger.
    DisplaysChanged,
    Lua(LuaRequest),
    /// Front application menus (app_menu extension; also fires `menus_change`).
    MenuTitles {
        app: String,
        titles: Vec<String>,
    },
    /// An event from AeroSpace's `subscribe` stream (`crate::aerospace`).
    Aerospace(crate::aerospace::AerospaceEvent),
    /// The AeroSpace connection was established, lost or failed.
    AerospaceStatus(crate::aerospace::AerospaceStatus),
}

/// Things the platform must do on the core's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum PlatformRequest {
    /// First subscription to `volume_change`.
    StartVolumeEvents,
    /// First subscription to `brightness_change`.
    StartBrightnessEvents,
    /// First `media_change` subscription or `image=media.artwork`.
    StartMediaEvents,
    /// First subscription to `space_windows_change`.
    StartSpaceWindowEvents,
    /// Forced `media_change` (`--update`, `--trigger media_change`): re-query now playing.
    RefreshMedia,
    /// `--add event <name> <notification>`: observe a distributed notification.
    ObserveNotification(String),
    /// `--load-font <path>`.
    LoadFont(String),
    /// Capture an alias' menu-bar extra (reply: `Input::AliasImage`).
    CaptureAlias {
        item: ItemId,
        owner: String,
        name: Option<String>,
        forced: bool,
    },
    /// The alias item `item` is gone (`--remove`, a failed `--add`, `--reload`/hotload):
    /// stop recapturing it and free its picture. Captures still in flight for it are
    /// dropped by the platform.
    RemoveAlias {
        item: ItemId,
    },
    /// Ask for Screen Recording permission (alias setup).
    RequestScreenCapture,
    /// (Re)configure the native provider of `item` (`provider=` extension).
    StartProvider {
        item: ItemId,
        provider: String,
        freq: Option<f32>,
        args: Option<String>,
    },
    StopProvider {
        item: ItemId,
    },
    /// Open top-level menu `index` of the front app (app_menu / `--menu`).
    OpenMenu {
        index: usize,
    },
    /// `--menubar` / `hide_menubar=`: auto-hide the native menu bar.
    SetMenuBarHidden(bool),
    /// `--hotload <bool>`.
    SetHotload(bool),
    /// Send a payload to a `mach_helper` service (`k\0v\0…\0`; `"k\0"` on destroy).
    MachSend {
        service: String,
        payload: Vec<u8>,
    },
    /// The window-borders configuration changed (`--borders`, `--reload`;
    /// `docs/spec/borders.md`).
    SetBorders(Box<crate::borders::BordersUpdate>),
    /// First use of AeroSpace (an `aerospace_*` subscription, `provider=aerospace`, Lua
    /// `mbar.aerospace`; never `--query aerospace`): connect and keep the event stream open.
    /// Handled by the binary, not the macOS layer.
    StartAerospace,
}

/// Actions requested by `Runtime::handle`.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// IPC response (possibly empty). Never sent after `--exit`.
    Reply {
        reply: ReplyToken,
        text: String,
    },
    /// `fork_exec`: `/usr/bin/env sh -c <script>` with `env` added to the daemon's startup
    /// environment (D1), cwd = config dir, killed after 60 s.
    RunScript {
        script: String,
        env: Vec<(String, String)>,
        item: Option<String>,
    },
    /// `--exit` (after mach helpers got `"k"`).
    Exit,
    /// Run the config file again (after `--reload`/hotload reset the core). `path` replaces
    /// the stored config path when given.
    RunConfig {
        path: Option<String>,
    },
    Platform(PlatformRequest),
    /// Call an in-process Lua handler.
    LuaCallback {
        handler: u64,
        env: Vec<(String, String)>,
    },
    /// Daemon log line (`respond()` echo, "No bar on display", …); the platform prefixes the
    /// timestamp.
    Log(String),
    /// One line for `--monitor` subscribers (extension).
    Monitor(String),
    /// The message `reply` executed `--monitor` (extension): instead of a plain [`Effect::Reply`],
    /// `text` (the output of the message, usually empty) is the first frame of a stream that
    /// stays open for `mode`'s lines. Emitted only when the runtime actually ran the command,
    /// so platforms never have to guess from the raw arguments.
    MonitorStart {
        reply: ReplyToken,
        mode: crate::command::MonitorMode,
        text: String,
    },
}

/// Window level constants (`bar.md` §3.7).
pub mod level {
    /// `kCGBackstopMenuLevel`.
    pub const BACKSTOP_MENU: i32 = -20;
    /// `kCGFloatingWindowLevel`.
    pub const FLOATING: i32 = 3;
    /// `kCGStatusWindowLevel`.
    pub const STATUS: i32 = 25;
    /// `kCGPopUpMenuWindowLevel`.
    pub const POPUP_MENU: i32 = 101;
}

/// One window to create/update.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowUpdate {
    pub key: WindowKey,
    /// Screen frame in points (top-left origin).
    pub frame: Rect,
    pub level: i32,
    pub scene: Scene,
    /// Window background blur (`--bar blur_radius`, `popup.blur_radius`).
    pub blur_radius: u32,
    /// System window shadow (`--bar shadow`).
    pub shadow: bool,
    /// Visible on all spaces (`--bar sticky`).
    pub sticky: bool,
    /// `--bar font_smoothing`.
    pub font_smoothing: bool,
    /// Ordering hint within the level (popups above their host's bar).
    pub order: u32,
}

/// Output of `Runtime::frame`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameOutput {
    /// Only dirty windows.
    pub windows: Vec<WindowUpdate>,
    /// Windows to close (popup closed, bar removed).
    pub closed: Vec<WindowKey>,
    /// Non-sticky windows to move to another space (`bar_change_space`, `bar.md` §6.4):
    /// after a space change with `--bar sticky=off`, the bar window of each display whose
    /// current space changed and the popups open on that bar. Applied after `windows`.
    pub space_moves: Vec<SpaceMove>,
}

/// One `window_send_to_space(dsid)` (`SLSMoveWindowsToManagedSpace`), see
/// [`FrameOutput::space_moves`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceMove {
    pub key: WindowKey,
    /// Target space id (`DisplayInfo::current_space` / `SpaceInfo::id`).
    pub dsid: u64,
}

/// Deterministic platform for Linux and tests (`DESIGN-CORE.md` "Testing"):
/// monospace metrics (`width = 0.6·size·chars`, ascent `0.8·size`, descent `0.2·size`),
/// one 1920×1080 display, one space, files/apps registered in maps.
#[derive(Debug, Clone)]
pub struct HeadlessResources {
    pub displays: Vec<DisplayInfo>,
    pub spaces: Vec<SpaceInfo>,
    pub active_adid: u32,
    pub menu_bar_visible: bool,
    pub now: Instant,
    /// Resolved file path → logical size or error.
    pub files: HashMap<String, Result<Size, ImageError>>,
    /// Known application names / bundle ids (32×32 icons).
    pub apps: Vec<String>,
    pub menu_extras: Option<Vec<MenuExtra>>,
    pub system: HashMap<String, SystemValue>,
}

impl Default for HeadlessResources {
    fn default() -> Self {
        HeadlessResources {
            displays: vec![DisplayInfo {
                id: 1,
                adid: 1,
                uuid: Some("00000000-0000-0000-0000-000000000001".to_string()),
                frame: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                builtin: false,
                menu_bar_height: 24.0,
                current_space: 1,
            }],
            spaces: vec![SpaceInfo {
                id: 1,
                display: 1,
                fullscreen: false,
            }],
            active_adid: 1,
            menu_bar_visible: true,
            now: Instant::now(),
            files: HashMap::new(),
            apps: Vec::new(),
            menu_extras: Some(Vec::new()),
            system: HashMap::new(),
        }
    }
}

fn hash_of(h: impl Hash) -> u64 {
    let mut s = DefaultHasher::new();
    h.hash(&mut s);
    s.finish()
}

impl Resources for HeadlessResources {
    fn text_metrics(&mut self, font: &FontSpec, text: &str) -> TextMetrics {
        let chars = text.chars().count() as f32;
        let width = 0.6 * font.size * chars;
        let ascent = 0.8 * font.size;
        let descent = 0.2 * font.size;
        let ink = if text.is_empty() {
            Rect::ZERO
        } else {
            Rect::new(0.0, -descent, width, ascent + descent)
        };
        TextMetrics {
            key: TextKey(hash_of((
                &font.family,
                &font.style,
                font.size.to_bits(),
                text,
            ))),
            ink,
            typographic_width: width,
            ascent,
            descent,
        }
    }

    fn load_image(&mut self, source: &ImageSource) -> Result<ImageInfo, ImageError> {
        match source {
            ImageSource::App(name) => {
                if self.apps.iter().any(|a| a == name) {
                    Ok(ImageInfo {
                        key: ImageKey(hash_of(("app", name))),
                        size: Size::new(32.0, 32.0),
                        hash: hash_of(("app", name)),
                    })
                } else {
                    Err(ImageError::NotFound)
                }
            }
            ImageSource::Space { index, .. } => {
                if *index >= 1 && (*index as usize) <= self.spaces.len() {
                    let d = &self.displays[0].frame;
                    Ok(ImageInfo {
                        key: ImageKey(hash_of(("space", index))),
                        size: d.size(),
                        hash: hash_of(("space", index)),
                    })
                } else {
                    Err(ImageError::NotFound)
                }
            }
            ImageSource::File(path) => match self.files.get(path) {
                Some(Ok(size)) => Ok(ImageInfo {
                    key: ImageKey(hash_of(("file", path))),
                    size: *size,
                    hash: hash_of(("file", path)),
                }),
                Some(Err(e)) => Err(*e),
                None => Err(ImageError::NotFound),
            },
            ImageSource::MediaArtwork | ImageSource::Empty => Err(ImageError::NotFound),
        }
    }

    fn displays(&self) -> &[DisplayInfo] {
        &self.displays
    }

    fn spaces(&self) -> &[SpaceInfo] {
        &self.spaces
    }

    fn active_display(&self) -> u32 {
        self.active_adid
    }

    fn menu_bar_visible(&self) -> bool {
        self.menu_bar_visible
    }

    fn now(&self) -> Instant {
        self.now
    }

    fn menu_extras(&mut self) -> Option<Vec<MenuExtra>> {
        self.menu_extras.clone()
    }

    fn query_system(&mut self, q: SystemQuery) -> Option<SystemValue> {
        self.system.get(&format!("{q:?}")).cloned()
    }
}
