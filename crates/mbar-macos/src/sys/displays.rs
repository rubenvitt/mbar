//! Display enumeration, arrangement ids, active display, menu bar height, notch and
//! reconfiguration callbacks (`docs/spec/bar.md` §6.1–6.3, §6.6).
//!
//! Coordinates: every rect returned here is in **global CG coordinates, points, origin at
//! the top-left of the main display, y down** (like `CGDisplayBounds`). `NSScreen` frames
//! (bottom-left origin) are flipped against the height of the primary screen.

use super::skylight as sls;
use super::util::{self, owned};
use super::{Sink, SysEvent};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CFArray, CFRetained, CFString, CFUUID, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayChangeSummaryFlags, CGDisplayIsBuiltin,
    CGDisplayRegisterReconfigurationCallback, CGDisplayRemoveReconfigurationCallback, CGEvent,
    CGGetActiveDisplayList, CGMainDisplayID,
};
use objc2_foundation::{NSNumber, NSString};
use std::ffi::c_void;

/// Notch geometry of a display (`safeAreaInsets.top > 0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Notch {
    /// `safeAreaInsets.top` (points).
    pub height: f64,
    /// Width of `auxiliaryTopLeftArea` (usable menu-bar area left of the notch).
    pub left_width: f64,
    /// Width of `auxiliaryTopRightArea`.
    pub right_width: f64,
}

impl Notch {
    /// Width of the notch itself given the display width.
    pub fn width(&self, display_width: f64) -> f64 {
        (display_width - self.left_width - self.right_width).max(0.0)
    }
}

/// One active display.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// `CGDirectDisplayID`.
    pub id: u32,
    /// Arrangement id (1-based index in `SLSCopyManagedDisplays`, 1 with a single display,
    /// 0 = unknown).
    pub adid: u32,
    /// Display UUID (`CGDisplayCreateUUIDFromDisplayID`).
    pub uuid: Option<String>,
    /// `CGDisplayBounds` (global, top-left, points).
    pub frame: CGRect,
    /// `NSScreen.visibleFrame` flipped to global top-left (excludes menu bar and Dock);
    /// equals `frame` when no `NSScreen` matches.
    pub visible_frame: CGRect,
    /// `NSScreen.backingScaleFactor` (1.0 when unknown).
    pub backing_scale: f64,
    /// `CGDisplayIsBuiltin`.
    pub builtin: bool,
    /// Notch geometry (built-in displays of notched MacBooks).
    pub notch: Option<Notch>,
    /// `display_menu_bar_rect(did).size.height` (`bar.md` §6.2).
    pub menu_bar_height: f64,
    /// Current space (`dsid`) via `SLSManagedDisplayGetCurrentSpace`; 0 = unknown.
    pub current_space: u64,
}

/// `CGGetActiveDisplayList` (includes mirrors).
pub fn active_display_ids() -> Vec<u32> {
    let mut count: u32 = 0;
    // SAFETY: querying the count with a null buffer is the documented usage.
    let err = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
    if err.0 != 0 || count == 0 {
        return Vec::new();
    }
    let mut ids = vec![0 as CGDirectDisplayID; count as usize];
    // SAFETY: `ids` has room for `count` entries.
    let err = unsafe { CGGetActiveDisplayList(count, ids.as_mut_ptr(), &mut count) };
    if err.0 != 0 {
        return Vec::new();
    }
    ids.truncate(count as usize);
    ids
}

/// `CGMainDisplayID()`.
pub fn main_display_id() -> u32 {
    CGMainDisplayID()
}

/// UUID string of a display (`display_uuid`); `None` if the display has no UUID.
pub fn display_uuid(did: u32) -> Option<String> {
    let create = sls::CGDisplayCreateUUIDFromDisplayID()?;
    // SAFETY: plain value argument; the result follows the Create rule.
    let uuid: CFRetained<CFUUID> = unsafe { owned(create(did)) }?;
    let s = CFUUID::new_string(None, Some(&uuid))?;
    Some(s.to_string())
}

/// `CGDisplayGetDisplayIDFromUUID(CFUUIDCreateFromString(uuid))`; 0 if unknown.
pub fn display_id_for_uuid(uuid: &str) -> u32 {
    let Some(get) = sls::CGDisplayGetDisplayIDFromUUID() else {
        return 0;
    };
    let Some(cf) = CFUUID::from_string(None, Some(&util::cfstr(uuid))) else {
        return 0;
    };
    // SAFETY: `cf` is a valid CFUUID for the duration of the call.
    unsafe { get(CFRetained::as_ptr(&cf).as_ptr()) }
}

/// Display UUIDs in arrangement order (`SLSCopyManagedDisplays`).
pub fn managed_display_uuids() -> Vec<String> {
    let Some(f) = sls::SLSCopyManagedDisplays() else {
        return Vec::new();
    };
    // SAFETY: valid connection id; Copy rule.
    let Some(arr): Option<CFRetained<CFArray>> = (unsafe { owned(f(sls::cid())) }) else {
        return Vec::new();
    };
    util::array_items(&arr)
        .iter()
        .filter_map(|v| util::cf_string(v))
        .collect()
}

/// 1-based position of `uuid` in `uuids`, 0 if absent (pure helper of `display_arrangement`).
pub fn arrangement_index(uuids: &[String], uuid: &str) -> u32 {
    uuids
        .iter()
        .position(|u| u == uuid)
        .map(|i| i as u32 + 1)
        .unwrap_or(0)
}

/// `display_arrangement(did)`: with exactly one active display 1 (if `did` is it) or 0;
/// otherwise the 1-based index of its UUID in `SLSCopyManagedDisplays`.
pub fn arrangement(did: u32) -> u32 {
    let active = active_display_ids();
    if active.len() == 1 {
        return (active[0] == did) as u32;
    }
    match display_uuid(did) {
        Some(uuid) => arrangement_index(&managed_display_uuids(), &uuid),
        None => 0,
    }
}

/// `display_arrangement_display_id(adid)`: 0 if out of range.
pub fn arrangement_display_id(adid: u32) -> u32 {
    if adid == 0 {
        return 0;
    }
    managed_display_uuids()
        .get(adid as usize - 1)
        .map(|u| display_id_for_uuid(u))
        .unwrap_or(0)
}

/// `SLSGetSpaceManagementMode` read once (1 = "Displays have separate Spaces").
pub fn space_management_mode() -> i32 {
    static MODE: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match sls::SLSGetSpaceManagementMode() {
        // SAFETY: valid connection id.
        Some(f) => unsafe { f(sls::cid()) },
        None => 0,
    })
}

/// Cursor location in global top-left coordinates (`CGEventGetLocation(CGEventCreate(NULL))`).
pub fn cursor_location() -> CGPoint {
    let ev = CGEvent::new(None);
    CGEvent::location(ev.as_deref())
}

fn rect_contains(r: &CGRect, p: CGPoint) -> bool {
    // CGRectContainsPoint: half-open.
    p.x >= r.origin.x
        && p.x < r.origin.x + r.size.width
        && p.y >= r.origin.y
        && p.y < r.origin.y + r.size.height
}

/// `display_active_display_uuid()` (`bar.md` §6.1): the menu-bar display when "Displays
/// have separate Spaces" is on (mode 1), else the display containing the cursor.
pub fn active_display_uuid() -> Option<String> {
    if space_management_mode() == 1 {
        let f = sls::SLSCopyActiveMenuBarDisplayIdentifier()?;
        // SAFETY: valid connection id; Copy rule.
        let s: CFRetained<CFString> = unsafe { owned(f(sls::cid())) }?;
        return Some(s.to_string());
    }
    let p = cursor_location();
    let did = active_display_ids()
        .into_iter()
        .find(|d| rect_contains(&CGDisplayBounds(*d), p))?;
    display_uuid(did)
}

/// `display_active_display_id()`.
pub fn active_display_id() -> u32 {
    let active = active_display_ids();
    if active.len() == 1 {
        return active[0];
    }
    active_display_uuid()
        .map(|u| display_id_for_uuid(&u))
        .unwrap_or(0)
}

/// `display_active_display_adid()`: 1 with one display; else the 1-based arrangement index
/// of the active display (0 on failure).
pub fn active_display_adid() -> u32 {
    if active_display_ids().len() == 1 {
        return 1;
    }
    match active_display_uuid() {
        Some(u) => arrangement_index(&managed_display_uuids(), &u),
        None => 0,
    }
}

/// `display_menu_bar_visible()`: `!SLSGetMenuBarAutohideEnabled`.
pub fn menu_bar_visible() -> bool {
    let Some(f) = sls::SLSGetMenuBarAutohideEnabled() else {
        return true;
    };
    let mut enabled: i32 = 0;
    // SAFETY: valid connection id and out pointer.
    unsafe { f(sls::cid(), &mut enabled) };
    enabled == 0
}

/// Flips an AppKit rect (bottom-left origin relative to the primary screen) into global
/// top-left coordinates.
pub fn flip_rect(r: CGRect, primary_height: f64) -> CGRect {
    CGRect {
        origin: CGPoint {
            x: r.origin.x,
            y: primary_height - (r.origin.y + r.size.height),
        },
        size: r.size,
    }
}

/// Menu bar height rule (arm64 path of `display_menu_bar_rect`, `bar.md` §6.2):
/// `inset` = rounded `NSMaxY(frame) - NSMaxY(visibleFrame) - 1` (`None` if no screen
/// matched), `notch` = `safeAreaInsets.top` of a built-in display (0 otherwise).
pub fn menu_bar_height_from(inset: Option<f64>, notch: f64) -> f64 {
    if let Some(i) = inset {
        if i > 0.0 {
            return i;
        }
    }
    if notch != 0.0 {
        notch + 6.0
    } else {
        24.0
    }
}

/// `inset` as computed by `display_nsscreen_top_inset`: clamped to ≥ 0 and rounded.
pub fn top_inset(frame_max_y: f64, visible_max_y: f64) -> f64 {
    let inset = (frame_max_y - visible_max_y - 1.0).max(0.0);
    (inset + 0.5).floor()
}

struct ScreenInfo {
    frame: CGRect,
    visible: CGRect,
    scale: f64,
    inset: f64,
    notch: Option<Notch>,
}

fn screen_number(screen: &NSScreen) -> Option<u32> {
    let desc = screen.deviceDescription();
    let num = desc.objectForKey(&NSString::from_str("NSScreenNumber"))?;
    let num: Retained<NSNumber> = num.downcast::<NSNumber>().ok()?;
    Some(num.unsignedIntValue())
}

fn screen_notch(screen: &NSScreen) -> Option<Notch> {
    // safeAreaInsets / auxiliaryTop*Area exist since macOS 12.
    if !screen.respondsToSelector(sel!(safeAreaInsets)) {
        return None;
    }
    let top = screen.safeAreaInsets().top;
    if top <= 0.0 {
        return None;
    }
    let (left, right) = if screen.respondsToSelector(sel!(auxiliaryTopLeftArea)) {
        (
            screen.auxiliaryTopLeftArea().size.width,
            screen.auxiliaryTopRightArea().size.width,
        )
    } else {
        (0.0, 0.0)
    };
    Some(Notch {
        height: top,
        left_width: left,
        right_width: right,
    })
}

fn screen_info(mtm: MainThreadMarker, did: u32) -> Option<ScreenInfo> {
    let screens = NSScreen::screens(mtm);
    let primary_h = screens.firstObject().map(|s| s.frame().size.height)?;
    for screen in screens.iter() {
        if screen_number(&screen) != Some(did) {
            continue;
        }
        let frame = screen.frame();
        let visible = screen.visibleFrame();
        let inset = top_inset(
            frame.origin.y + frame.size.height,
            visible.origin.y + visible.size.height,
        );
        return Some(ScreenInfo {
            frame: flip_rect(frame, primary_h),
            visible: flip_rect(visible, primary_h),
            scale: screen.backingScaleFactor(),
            inset,
            notch: screen_notch(&screen),
        });
    }
    None
}

/// Largest `backingScaleFactor` over all screens (≥ 1), used for app-icon sizing.
pub fn max_backing_scale(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .iter()
        .map(|s| s.backingScaleFactor())
        .fold(1.0, f64::max)
}

/// `display_menu_bar_rect(did).size.height` (`bar.md` §6.2).
pub fn menu_bar_height(mtm: MainThreadMarker, did: u32) -> f64 {
    #[cfg(target_arch = "x86_64")]
    if let Some(f) = sls::SLSGetRevealedMenuBarBounds() {
        let mut rect = CGRect::default();
        // SAFETY: valid out pointer and connection id.
        unsafe { f(&mut rect, sls::cid(), current_space_of(did)) };
        return rect.size.height;
    }
    let info = screen_info(mtm, did);
    let builtin = CGDisplayIsBuiltin(did);
    let notch = if builtin {
        info.as_ref()
            .and_then(|i| i.notch)
            .map(|n| n.height)
            .unwrap_or(0.0)
    } else {
        0.0
    };
    menu_bar_height_from(info.map(|i| i.inset), notch)
}

/// Current space id of a display (`display_space_id`), 0 if unknown.
pub fn current_space_of(did: u32) -> u64 {
    match display_uuid(did) {
        Some(u) => super::spaces::current_space(&u),
        None => 0,
    }
}

/// All active displays, ordered by arrangement id (displays without a managed UUID — e.g.
/// mirrors — come last with `adid = 0`).
pub fn list(mtm: MainThreadMarker) -> Vec<Display> {
    let ids = active_display_ids();
    let uuids = managed_display_uuids();
    let single = ids.len() == 1;
    let mut out: Vec<Display> = ids
        .iter()
        .map(|&did| {
            let uuid = display_uuid(did);
            let adid = if single {
                1
            } else {
                uuid.as_deref()
                    .map(|u| arrangement_index(&uuids, u))
                    .unwrap_or(0)
            };
            let frame = CGDisplayBounds(did);
            let builtin = CGDisplayIsBuiltin(did);
            let info = screen_info(mtm, did);
            let notch = info.as_ref().and_then(|i| i.notch);
            let menu_bar_height = menu_bar_height(mtm, did);
            let current_space = uuid
                .as_deref()
                .map(super::spaces::current_space)
                .unwrap_or(0);
            Display {
                id: did,
                adid,
                uuid,
                frame,
                visible_frame: info.as_ref().map(|i| i.visible).unwrap_or(frame),
                backing_scale: info.as_ref().map(|i| i.scale).unwrap_or(1.0),
                builtin,
                notch,
                menu_bar_height,
                current_space,
            }
        })
        .collect();
    out.sort_by_key(|d| if d.adid == 0 { u32::MAX } else { d.adid });
    out
}

/// Frame of the `NSScreen` for `did` in global top-left coordinates (debug helper).
pub fn screen_frame(mtm: MainThreadMarker, did: u32) -> Option<CGRect> {
    screen_info(mtm, did).map(|i| i.frame)
}

/// Whether a reconfiguration callback with these flags rebuilds the bars (`bar.md` §6.6:
/// add, remove, moved, desktop-shape-changed; the begin-configuration pre-notification and
/// other flags do nothing).
pub fn reconfiguration_relevant(flags: u32) -> bool {
    let f = CGDisplayChangeSummaryFlags(flags);
    f.contains(CGDisplayChangeSummaryFlags::AddFlag)
        || f.contains(CGDisplayChangeSummaryFlags::RemoveFlag)
        || f.contains(CGDisplayChangeSummaryFlags::MovedFlag)
        || f.contains(CGDisplayChangeSummaryFlags::DesktopShapeChangedFlag)
}

/// Keeps a `CGDisplayRegisterReconfigurationCallback` registration alive; dropping it
/// unregisters.
pub struct ReconfigurationObserver {
    ctx: *mut Sink,
}

unsafe extern "C-unwind" fn reconfiguration_callback(
    did: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    user_info: *mut c_void,
) {
    if user_info.is_null() || !reconfiguration_relevant(flags.0) {
        return;
    }
    // SAFETY: `user_info` is the `Box<Sink>` leaked by `observe_reconfiguration`, alive
    // until the observer is dropped (which unregisters this callback first).
    let sink = unsafe { &*(user_info as *const Sink) };
    // DisplayServices brightness registration follows added/removed displays.
    let f = CGDisplayChangeSummaryFlags(flags.0);
    if f.contains(CGDisplayChangeSummaryFlags::AddFlag) {
        super::events::brightness::display_added(did);
    } else if f.contains(CGDisplayChangeSummaryFlags::RemoveFlag) {
        super::events::brightness::display_removed(did);
    }
    sink(SysEvent::DisplaysReconfigured {
        display: did,
        flags: flags.0,
    });
}

/// Registers the CG display reconfiguration callback (`display_begin`). Callbacks arrive on
/// the main thread; each relevant one posts [`SysEvent::DisplaysReconfigured`].
pub fn observe_reconfiguration(sink: Sink) -> ReconfigurationObserver {
    let ctx = Box::into_raw(Box::new(sink));
    // SAFETY: the callback has the required signature; `ctx` stays valid until
    // `CGDisplayRemoveReconfigurationCallback` in Drop.
    let err = unsafe {
        CGDisplayRegisterReconfigurationCallback(Some(reconfiguration_callback), ctx.cast())
    };
    if err.0 != 0 {
        log::warn!("CGDisplayRegisterReconfigurationCallback failed: {}", err.0);
    }
    ReconfigurationObserver { ctx }
}

impl Drop for ReconfigurationObserver {
    fn drop(&mut self) {
        // SAFETY: same callback/context pair as registered; afterwards no callback can
        // observe `ctx`, so it can be freed.
        unsafe {
            CGDisplayRemoveReconfigurationCallback(Some(reconfiguration_callback), self.ctx.cast());
            drop(Box::from_raw(self.ctx));
        }
    }
}

/// Size helper used by tests and the alias module.
pub(crate) fn size(w: f64, h: f64) -> CGSize {
    CGSize {
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrangement_index_is_one_based() {
        let u = vec!["A".to_string(), "B".to_string()];
        assert_eq!(arrangement_index(&u, "A"), 1);
        assert_eq!(arrangement_index(&u, "B"), 2);
        assert_eq!(arrangement_index(&u, "C"), 0);
    }

    #[test]
    fn menu_bar_height_rule() {
        assert_eq!(menu_bar_height_from(Some(24.0), 0.0), 24.0);
        assert_eq!(menu_bar_height_from(Some(37.0), 32.0), 37.0);
        assert_eq!(menu_bar_height_from(Some(0.0), 32.0), 38.0);
        assert_eq!(menu_bar_height_from(None, 0.0), 24.0);
        assert_eq!(top_inset(1117.0, 1080.0), 36.0);
        assert_eq!(top_inset(100.0, 100.0), 0.0);
    }

    #[test]
    fn flip() {
        let r = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: size(100.0, 20.0),
        };
        let f = flip_rect(r, 1000.0);
        assert_eq!(f.origin.y, 980.0);
    }

    #[test]
    fn notch_width() {
        let n = Notch {
            height: 32.0,
            left_width: 600.0,
            right_width: 600.0,
        };
        assert_eq!(n.width(1512.0), 312.0);
    }

    #[test]
    fn relevant_flags() {
        assert!(!reconfiguration_relevant(1)); // begin configuration only
        assert!(reconfiguration_relevant(1 << 4));
        assert!(reconfiguration_relevant(1 << 12));
    }
}
