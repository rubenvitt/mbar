//! System integration: displays, spaces, events, mouse, menus, aliases, providers, mach.
//!
//! Every OS source reports through one [`Sink`]. A sink may be called from **any** thread
//! (most sources call it on the main thread: NSWorkspace/distributed notifications,
//! SkyLight notify procs, IOPS, SCDynamicStore, CFMachPort, AX observers; CoreAudio
//! listeners, the provider/alias schedulers, the media poller and script reapers call it
//! from background threads). The integration layer is expected to queue the event and wake
//! the main run loop, then map it to `mbar_core::platform::Input`.
//!
//! Private frameworks (SkyLight, DisplayServices, MediaRemote) are resolved at runtime with
//! `dlopen`/`dlsym` instead of `#[link(kind = "framework")]`: linking a private framework
//! needs `-F/System/Library/PrivateFrameworks`, and a missing symbol on a future macOS would
//! otherwise prevent the binary from starting. Missing symbols degrade to "no data".

use objc2_core_foundation::{CFRetained, CGPoint, CGRect};
use objc2_core_graphics::CGImage;
use std::sync::Arc;
use std::time::Duration;

pub(crate) mod skylight;
pub(crate) mod util;

pub mod alias;
pub mod displays;
pub mod events;
pub mod hotload;
pub mod mach_server;
pub mod menus;
pub mod mouse;
pub mod providers;
pub mod script;
pub mod spaces;

pub use mach_server::MachReply;
pub use mouse::{MouseEvent, MouseKind};
pub use util::{on_main, os_at_least, os_version};

/// Where every system event goes. Must be cheap and non-blocking (queue + wake main).
pub type Sink = Arc<dyn Fn(SysEvent) + Send + Sync>;

/// Everything the system layer reports. Mapping to `mbar_core::platform::{Input, OsEvent}`
/// is done by the integration layer; the comments name the target.
#[derive(Debug)]
pub enum SysEvent {
    /// `NSWorkspaceDidActivateApplicationNotification` → `OsEvent::FrontAppSwitched`
    /// (`front_app_switched`, INFO = `name` when present).
    FrontAppSwitched {
        name: Option<String>,
        bundle_id: Option<String>,
        pid: i32,
    },
    /// `NSWorkspaceActiveSpaceDidChangeNotification` or SkyLight 1327/1328 →
    /// `OsEvent::SpaceChanged`. `info_json` is the `space_change` INFO computed over **every
    /// active display** (`events.md` §5.2 format); the core rebuilds it for the displays
    /// that actually carry a bar with [`spaces::space_change_info`].
    SpaceChange { info_json: String },
    /// One `space_windows_change` INFO (`events.md` §5.11, exact bytes) for one space.
    SpaceWindowsChange { space: u32, info_json: String },
    /// Private `NSWorkspaceActiveDisplayDidChangeNotification` → `OsEvent::ActiveDisplayChanged`.
    /// `adid` = `display_active_display_adid()` at notification time (INFO = `"%d"`).
    DisplayChange { adid: u32 },
    /// `CGDisplayRegisterReconfigurationCallback` with add/remove/moved/desktop-shape
    /// flags (one per callback, `bar.md` §6.6) → `Input::DisplaysChanged`.
    DisplaysReconfigured { display: u32, flags: u32 },
    /// Distributed `AppleInterfaceMenuBarHidingChangedNotification` →
    /// `OsEvent::MenuBarHiddenChanged` (no script event).
    MenuBarHidingChanged,
    /// Default output volume **0..1** (muted reads 0), de-duplicated (Δ > 0.01).
    /// INFO = `(int)(v*100 + 0.5)`.
    VolumeChange(f32),
    /// Display brightness **0..1**, de-duplicated with one global threshold (Δ > 0.01).
    BrightnessChange(f32),
    /// `"AC"` or `"BATTERY"`, only on state change.
    PowerSourceChange(String),
    /// SSID (may be empty: no Wi-Fi, disconnected or no Location permission).
    WifiChange(String),
    /// `media_change` INFO (`events.md` §5.10 exact format), de-duplicated.
    MediaChange(String),
    /// Now-playing artwork (`COVER_CHANGED`, for `media.artwork` images).
    MediaArtwork(Option<CFRetained<CGImage>>),
    /// `NSWorkspaceWillSleepNotification`.
    SystemWillSleep,
    /// `NSWorkspaceDidWakeNotification` or distributed `com.apple.screenIsUnlocked`
    /// (`screen_unlocked`). The +500 ms re-post is the core's job (`events.md` §9.2).
    SystemWoke { screen_unlocked: bool },
    /// A distributed notification observed via [`events::SystemEvents::observe`].
    /// `user_info_json` = pretty-printed `NSJSONSerialization` of `userInfo` when valid.
    DistributedNotification {
        name: String,
        user_info_json: Option<String>,
    },
    /// Front application's top-level menu titles changed (`menus_change`, app_menu).
    MenusChanged {
        app: String,
        pid: i32,
        titles: Vec<String>,
    },
    /// A native provider sample for subscription `id` (`provider=` extension).
    ProviderSample {
        id: u64,
        provider: String,
        values: Vec<(String, String)>,
    },
    /// Result of an alias capture for subscription `id`. `image == None && disabled` means
    /// capture is suspended (keep the old picture); `image == None && !disabled` means the
    /// window is gone (drop the picture, look it up again next time).
    AliasUpdate {
        id: u64,
        owner: String,
        name: Option<String>,
        window_id: u32,
        /// Logical size/origin in points (global, top-left).
        frame: CGRect,
        image: Option<CFRetained<CGImage>>,
        disabled: bool,
    },
    /// SkyLight capture gating (1322/905 → disabled, 904/1401/1508 → enabled).
    CaptureGating { disabled: bool },
    /// One mach IPC request. Answer (now or later) with [`MachReply::send`].
    MachMessage {
        /// argv parsed from the payload (`\0`-separated, terminated by `\0\0`).
        args: Vec<String>,
        payload: Vec<u8>,
        reply: MachReply,
    },
    /// Global/local mouse monitor event (see [`mouse`]).
    Mouse(MouseEvent),
    /// The config directory changed while `--hotload` is on (rate limited, see [`hotload`]).
    ConfigChanged,
    /// A script spawned by [`script::ScriptRunner`] exited (or was killed).
    ScriptFinished {
        id: u64,
        pid: u32,
        /// Exit code; `None` if killed by a signal.
        status: Option<i32>,
        /// Captured stdout when requested.
        output: Option<String>,
        duration: Duration,
        timed_out: bool,
    },
}

/// Convenience: a point in global top-left coordinates.
pub type GlobalPoint = CGPoint;
