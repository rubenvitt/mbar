//! The private SkyLight / CoreGraphics / HIServices symbols the borders use
//! (`docs/spec/borders-skylight.md`; signatures exactly as JankyBorders declares them in
//! `src/misc/extern.h`), resolved lazily with `dlsym` through [`private_fns!`], plus thin
//! wrappers so the rest of the module never touches raw pointers.
//!
//! [`available`] checks every **required** symbol once: if one is missing the borders stay
//! off with a single warning. Optional symbols degrade: `SLSGetWindowSubLevel` → sub-level
//! 0 (JankyBorders' raw-MIG sub-level request is deliberately not ported, spec §7.6),
//! `SLSWindowIteratorGetCornerRadii` → radius 9, `SLSWindowSetShadowProperties` → the
//! default shadow, `SLSCopyManagedDisplayForWindow` → space 0, `_AXUIElementGetWindow` →
//! the SkyLight focus path.

#![allow(non_snake_case)]

use crate::sys::skylight as sls;
use crate::sys::util::{owned, private_fns, SKYLIGHT};
use objc2_application_services::AXUIElement;
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGAffineTransform, CGPoint,
    CGRect,
};
use objc2_core_graphics::CGContext;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::OnceLock;

const APPLICATION_SERVICES: &str =
    "/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices";

/// `kCGBackingStoreBuffered`.
const BACKING_STORE_BUFFERED: i32 = 2;
/// Tag-mask size in bits for `SLSSetWindowTags` / `SLSClearWindowTags`.
const TAG_SIZE: i32 = 64;
/// `SLSCopyWindowsWithOptionsAndTags` options used by every JankyBorders query.
const WINDOW_LIST_OPTIONS: u32 = 0x2;
/// `SLSCopySpacesForWindows` selector: all spaces.
const ALL_SPACES: i32 = 0x7;

/// `ProcessSerialNumber`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ProcessSerialNumber {
    pub high: u32,
    pub low: u32,
}

private_fns! { SKYLIGHT =>
    fn SLSNewConnection(i32, *mut i32) -> i32;
    fn SLSReleaseConnection(i32) -> i32;
    fn SLSNewWindow(i32, i32, f32, f32, *const CFType, *mut u32) -> i32;
    fn SLSReleaseWindow(i32, u32) -> i32;
    fn SLSSetWindowTags(i32, u32, *mut u64, i32) -> i32;
    fn SLSClearWindowTags(i32, u32, *mut u64, i32) -> i32;
    fn SLSSetWindowShape(i32, u32, f32, f32, *const CFType) -> i32;
    fn SLSSetWindowResolution(i32, u32, f64) -> i32;
    fn SLSSetWindowOpacity(i32, u32, bool) -> i32;
    fn SLSWindowSetShadowProperties(u32, *const CFDictionary) -> i32;
    fn SLWindowContextCreate(i32, u32, *const CFDictionary) -> *mut CGContext;
    fn SLSFlushWindowContentRegion(i32, u32, *mut c_void) -> i32;
    fn SLSWindowFreezeWithOptions(i32, u32, *const CFType) -> i32;
    fn SLSWindowThaw(i32, u32) -> i32;
    fn SLSDisableUpdate(i32) -> i32;
    fn SLSReenableUpdate(i32) -> i32;
    fn CGSNewRegionWithRect(*const CGRect, *mut *const CFType) -> i32;
    fn SLSGetWindowBounds(i32, u32, *mut CGRect) -> i32;
    fn SLSWindowIsOrderedIn(i32, u32, *mut bool) -> i32;
    fn SLSGetWindowSubLevel(i32, u32) -> i32;
    fn SLSWindowIteratorGetLevel(*const CFType) -> i32;
    fn SLSWindowIteratorGetCornerRadii(*const CFType) -> *const CFArray;
    fn SLSTransactionCreate(i32) -> *const CFType;
    fn SLSTransactionCommit(*const CFType, i32) -> i32;
    fn SLSTransactionMoveWindowWithGroup(*const CFType, u32, CGPoint) -> i32;
    fn SLSTransactionOrderWindow(*const CFType, u32, i32, u32) -> i32;
    fn SLSTransactionSetWindowLevel(*const CFType, u32, i32) -> i32;
    fn SLSTransactionSetWindowSubLevel(*const CFType, u32, i32) -> i32;
    fn SLSTransactionSetWindowTransform(*const CFType, u32, i32, i32, CGAffineTransform) -> i32;
    fn SLSCopySpacesForWindows(i32, i32, *const CFArray) -> *const CFArray;
    fn SLSCopyManagedDisplayForWindow(i32, u32) -> *const CFString;
    fn _SLPSGetFrontProcess(*mut ProcessSerialNumber) -> i32;
    fn SLSGetConnectionIDForPSN(i32, *mut ProcessSerialNumber, *mut i32) -> i32;
}

private_fns! { APPLICATION_SERVICES =>
    // JankyBorders declares it `void`; the real return type is `AXError`.
    fn _AXUIElementGetWindow(*const AXUIElement, *mut u32) -> i32;
}

macro_rules! missing {
    ($($module:ident :: $f:ident),* $(,)?) => {{
        let mut v: Vec<&'static str> = Vec::new();
        $( if $module::$f().is_none() { v.push(stringify!($f)); } )*
        v
    }};
}

/// Whether every required symbol resolved (checked once; one warning when not).
pub(super) fn available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let missing = missing![
            sls::SLSMainConnectionID,
            sls::SLSRegisterNotifyProc,
            sls::SLSRequestNotificationsForWindows,
            sls::SLSGetWindowOwner,
            sls::SLSConnectionGetPID,
            sls::SLSCopyManagedDisplays,
            sls::SLSCopyManagedDisplaySpaces,
            sls::SLSManagedDisplayGetCurrentSpace,
            sls::SLSCopyActiveMenuBarDisplayIdentifier,
            sls::SLSCopyWindowsWithOptionsAndTags,
            sls::SLSWindowQueryWindows,
            sls::SLSWindowQueryResultCopyWindows,
            sls::SLSWindowIteratorAdvance,
            sls::SLSWindowIteratorGetParentID,
            sls::SLSWindowIteratorGetWindowID,
            sls::SLSWindowIteratorGetTags,
            sls::SLSWindowIteratorGetAttributes,
            sls::SLSMoveWindowsToManagedSpace,
            this::SLSNewConnection,
            this::SLSReleaseConnection,
            this::SLSNewWindow,
            this::SLSReleaseWindow,
            this::SLSSetWindowTags,
            this::SLSClearWindowTags,
            this::SLSSetWindowShape,
            this::SLSSetWindowResolution,
            this::SLSSetWindowOpacity,
            this::SLWindowContextCreate,
            this::SLSFlushWindowContentRegion,
            this::SLSWindowFreezeWithOptions,
            this::SLSWindowThaw,
            this::SLSDisableUpdate,
            this::SLSReenableUpdate,
            this::CGSNewRegionWithRect,
            this::SLSGetWindowBounds,
            this::SLSWindowIsOrderedIn,
            this::SLSWindowIteratorGetLevel,
            this::SLSTransactionCreate,
            this::SLSTransactionCommit,
            this::SLSTransactionMoveWindowWithGroup,
            this::SLSTransactionOrderWindow,
            this::SLSTransactionSetWindowLevel,
            this::SLSTransactionSetWindowSubLevel,
            this::SLSTransactionSetWindowTransform,
            this::SLSCopySpacesForWindows,
            this::_SLPSGetFrontProcess,
            this::SLSGetConnectionIDForPSN,
        ];
        if missing.is_empty() {
            true
        } else {
            log::warn!(
                "window borders disabled: SkyLight lacks {}",
                missing.join(", ")
            );
            false
        }
    })
}

/// Lets [`missing!`] name this module's functions as `this::NAME`.
mod this {
    pub(super) use super::*;
}

/// The process' main SkyLight connection.
pub(super) fn main_cid() -> i32 {
    sls::cid()
}

/// `SLSNewConnection(0, &cid)`; `None` on failure or a zero id.
pub(super) fn new_connection() -> Option<i32> {
    let f = SLSNewConnection()?;
    let mut cid = 0;
    // SAFETY: `0` is the documented first argument and `cid` a valid out pointer.
    let err = unsafe { f(0, &mut cid) };
    (err == 0 && cid != 0).then_some(cid)
}

/// `SLSReleaseConnection(cid)`.
pub(super) fn release_connection(cid: i32) {
    if let Some(f) = SLSReleaseConnection() {
        // SAFETY: plain value argument; releasing an unknown id fails harmlessly.
        unsafe { f(cid) };
    }
}

/// `CGSNewRegionWithRect(&rect, &region)` (+1).
pub(super) fn new_region(rect: CGRect) -> Option<CFRetained<CFType>> {
    let f = CGSNewRegionWithRect()?;
    let mut region: *const CFType = std::ptr::null();
    // SAFETY: `rect` is passed by pointer as declared; `region` is a valid out pointer.
    let err = unsafe { f(&rect, &mut region) };
    // SAFETY: on success the region is a CF object owned by the caller (Create rule).
    let region = unsafe { owned(region) };
    if err != 0 {
        return None;
    }
    region
}

/// `SLSNewWindow(cid, kCGBackingStoreBuffered, -9999, -9999, region, &wid)`; 0 on failure.
pub(super) fn new_window(cid: i32, region: &CFType) -> u32 {
    let Some(f) = SLSNewWindow() else { return 0 };
    let mut wid = 0;
    // SAFETY: valid connection, a live region object and a valid out pointer.
    let err = unsafe {
        f(
            cid,
            BACKING_STORE_BUFFERED,
            -9999.0,
            -9999.0,
            region,
            &mut wid,
        )
    };
    if err != 0 {
        return 0;
    }
    wid
}

/// `SLSReleaseWindow(cid, wid)`.
pub(super) fn release_window(cid: i32, wid: u32) {
    if let Some(f) = SLSReleaseWindow() {
        // SAFETY: plain values; the window belongs to `cid`.
        unsafe { f(cid, wid) };
    }
}

/// `SLSSetWindowTags(cid, wid, &tags, 64)`.
pub(super) fn set_tags(cid: i32, wid: u32, tags: u64) {
    if let Some(f) = SLSSetWindowTags() {
        let mut tags = tags;
        // SAFETY: a fresh 64-bit tag mask (the call may read it as an array of 64 bits).
        unsafe { f(cid, wid, &mut tags, TAG_SIZE) };
    }
}

/// `SLSClearWindowTags(cid, wid, &tags, 64)`.
pub(super) fn clear_tags(cid: i32, wid: u32, tags: u64) {
    if let Some(f) = SLSClearWindowTags() {
        let mut tags = tags;
        // SAFETY: as in `set_tags`.
        unsafe { f(cid, wid, &mut tags, TAG_SIZE) };
    }
}

/// `SLSSetWindowShape(cid, wid, x, y, region)`.
pub(super) fn set_shape(cid: i32, wid: u32, x: f32, y: f32, region: &CFType) {
    if let Some(f) = SLSSetWindowShape() {
        // SAFETY: valid connection/window and a live region object.
        unsafe { f(cid, wid, x, y, region) };
    }
}

/// `SLSSetWindowResolution(cid, wid, res)`.
pub(super) fn set_resolution(cid: i32, wid: u32, res: f64) {
    if let Some(f) = SLSSetWindowResolution() {
        // SAFETY: plain values.
        unsafe { f(cid, wid, res) };
    }
}

/// `SLSSetWindowOpacity(cid, wid, opaque)`.
pub(super) fn set_opacity(cid: i32, wid: u32, opaque: bool) {
    if let Some(f) = SLSSetWindowOpacity() {
        // SAFETY: plain values.
        unsafe { f(cid, wid, opaque) };
    }
}

/// `SLSWindowSetShadowProperties(wid, {"com.apple.WindowShadowDensity": 0})`: no shadow.
pub(super) fn disable_shadow(wid: u32) {
    let Some(f) = SLSWindowSetShadowProperties() else {
        return;
    };
    let key = crate::sys::util::cfstr("com.apple.WindowShadowDensity");
    let value = CFNumber::new_isize(0);
    let dict = CFDictionary::<CFString, CFNumber>::from_slices(&[&*key], &[&*value]);
    // SAFETY: a valid dictionary with CFString keys; SkyLight retains what it keeps.
    unsafe { f(wid, dict.as_opaque()) };
}

/// `SLWindowContextCreate(cid, wid, NULL)` (+1, released with `CGContextRelease`).
pub(super) fn window_context(cid: i32, wid: u32) -> Option<CFRetained<CGContext>> {
    let f = SLWindowContextCreate()?;
    // SAFETY: valid connection/window; NULL options are allowed.
    let ctx = unsafe { f(cid, wid, std::ptr::null()) };
    // SAFETY: the context follows the Create rule.
    NonNull::new(ctx).map(|p| unsafe { CFRetained::from_raw(p) })
}

/// `SLSFlushWindowContentRegion(cid, wid, NULL)`.
pub(super) fn flush_window(cid: i32, wid: u32) {
    if let Some(f) = SLSFlushWindowContentRegion() {
        // SAFETY: NULL dirty region = the whole window.
        unsafe { f(cid, wid, std::ptr::null_mut()) };
    }
}

/// `SLSWindowFreezeWithOptions(cid, wid, NULL)`.
pub(super) fn freeze(cid: i32, wid: u32) {
    if let Some(f) = SLSWindowFreezeWithOptions() {
        // SAFETY: NULL options are allowed.
        unsafe { f(cid, wid, std::ptr::null()) };
    }
}

/// `SLSWindowThaw(cid, wid)` (harmless on a window that is not frozen).
pub(super) fn thaw(cid: i32, wid: u32) {
    if let Some(f) = SLSWindowThaw() {
        // SAFETY: plain values.
        unsafe { f(cid, wid) };
    }
}

/// `SLSDisableUpdate(cid)`.
pub(super) fn disable_update(cid: i32) {
    if let Some(f) = SLSDisableUpdate() {
        // SAFETY: plain value.
        unsafe { f(cid) };
    }
}

/// `SLSReenableUpdate(cid)`.
pub(super) fn reenable_update(cid: i32) {
    if let Some(f) = SLSReenableUpdate() {
        // SAFETY: plain value.
        unsafe { f(cid) };
    }
}

/// `SLSGetWindowBounds(cid, wid, &frame)`: global coordinates, top-left origin.
pub(super) fn window_bounds(cid: i32, wid: u32) -> Option<CGRect> {
    let f = SLSGetWindowBounds()?;
    let mut frame = CGRect::default();
    // SAFETY: valid out pointer.
    let err = unsafe { f(cid, wid, &mut frame) };
    (err == 0).then_some(frame)
}

/// `SLSWindowIsOrderedIn(cid, wid, &shown)`.
pub(super) fn is_ordered_in(cid: i32, wid: u32) -> bool {
    let Some(f) = SLSWindowIsOrderedIn() else {
        return false;
    };
    let mut shown = false;
    // SAFETY: valid out pointer to a C `bool`.
    unsafe { f(cid, wid, &mut shown) };
    shown
}

/// `SLSGetWindowSubLevel(cid, wid)`; 0 when the symbol is missing (spec BQ27 fallback).
pub(super) fn window_sub_level(cid: i32, wid: u32) -> i32 {
    match SLSGetWindowSubLevel() {
        // SAFETY: plain values; unknown windows return 0.
        Some(f) => unsafe { f(cid, wid) },
        None => 0,
    }
}

/// `SLSMoveWindowsToManagedSpace(cid, [wid], sid)`.
pub(super) fn move_to_space(cid: i32, wid: u32, sid: u64) {
    let Some(f) = sls::SLSMoveWindowsToManagedSpace() else {
        return;
    };
    let list = wid_array(wid);
    // SAFETY: valid connection, a CFArray of one CFNumber window id, a plain space id.
    unsafe { f(cid, list.as_opaque(), sid) };
}

/// `[wid]` as a CFArray of one `kCFNumberSInt32Type` CFNumber.
pub(super) fn wid_array(wid: u32) -> CFRetained<CFArray<CFNumber>> {
    CFArray::from_retained_objects(&[CFNumber::new_i32(wid as i32)])
}

/// Space ids as a CFArray of `kCFNumberSInt64Type` CFNumbers.
pub(super) fn sid_array(sids: &[u64]) -> CFRetained<CFArray<CFNumber>> {
    let nums: Vec<CFRetained<CFNumber>> =
        sids.iter().map(|s| CFNumber::new_i64(*s as i64)).collect();
    CFArray::from_retained_objects(&nums)
}

/// `SLSCopySpacesForWindows(cid, 0x7, [wid])[0]`, 0 when there is none.
pub(super) fn first_space_of_window(cid: i32, wid: u32) -> u64 {
    let Some(f) = SLSCopySpacesForWindows() else {
        return 0;
    };
    let list = wid_array(wid);
    // SAFETY: valid connection and a CFArray of CFNumbers; Copy rule for the result.
    let Some(spaces): Option<CFRetained<CFArray>> =
        (unsafe { owned(f(cid, ALL_SPACES, list.as_opaque())) })
    else {
        return 0;
    };
    crate::sys::util::array_items(&spaces)
        .first()
        .and_then(|v| crate::sys::util::cf_i64(v))
        .map(|v| v as u64)
        .unwrap_or(0)
}

/// `SLSCopyManagedDisplayForWindow(cid, wid)`.
pub(super) fn managed_display_for_window(cid: i32, wid: u32) -> Option<String> {
    let f = SLSCopyManagedDisplayForWindow()?;
    // SAFETY: valid connection; Copy rule for the result.
    let s: CFRetained<CFString> = unsafe { owned(f(cid, wid)) }?;
    Some(s.to_string())
}

/// `_SLPSGetFrontProcess` → `SLSGetConnectionIDForPSN`: the connection of the front app.
pub(super) fn front_process_cid(cid: i32) -> Option<i32> {
    let (front, for_psn) = (_SLPSGetFrontProcess()?, SLSGetConnectionIDForPSN()?);
    let mut psn = ProcessSerialNumber::default();
    let mut target = 0;
    // SAFETY: valid out pointers.
    unsafe {
        if front(&mut psn) != 0 {
            return None;
        }
        if for_psn(cid, &mut psn, &mut target) != 0 {
            return None;
        }
    }
    (target != 0).then_some(target)
}

/// `SLSGetWindowOwner` → `SLSConnectionGetPID`: the pid owning a window (0 if unknown).
pub(super) fn window_owner_pid(wid: u32) -> i32 {
    let (Some(owner), Some(pid_of)) = (sls::SLSGetWindowOwner(), sls::SLSConnectionGetPID()) else {
        return 0;
    };
    let mut owner_cid = 0;
    let mut pid: libc::pid_t = 0;
    // SAFETY: valid connection id and out pointers.
    unsafe {
        owner(main_cid(), wid, &mut owner_cid);
        pid_of(owner_cid, &mut pid);
    }
    pid
}

/// `SLSConnectionGetPID(cid, &pid)`.
pub(super) fn connection_pid(cid: i32) -> i32 {
    let Some(f) = sls::SLSConnectionGetPID() else {
        return 0;
    };
    let mut pid: libc::pid_t = 0;
    // SAFETY: valid out pointer.
    unsafe { f(cid, &mut pid) };
    pid
}

/// `_AXUIElementGetWindow(element, &wid)`; `None` when the symbol is missing or fails.
pub(super) fn ax_window_id(element: &AXUIElement) -> Option<u32> {
    let f = _AXUIElementGetWindow()?;
    let mut wid = 0;
    // SAFETY: a live AX element and a valid out pointer.
    let err = unsafe { f(element, &mut wid) };
    (err == 0 && wid != 0).then_some(wid)
}

/// Whether `_AXUIElementGetWindow` exists (needed by the AX focus path).
pub(super) fn has_ax_window() -> bool {
    _AXUIElementGetWindow().is_some()
}

/// One position of a SkyLight window iterator (valid only inside [`query_windows`]).
pub(super) struct WindowIter(*const CFType);

impl WindowIter {
    pub(super) fn wid(&self) -> u32 {
        // SAFETY: `self.0` is a live iterator positioned on a window (see `query_windows`).
        sls::SLSWindowIteratorGetWindowID().map_or(0, |f| unsafe { f(self.0) })
    }

    pub(super) fn parent(&self) -> u32 {
        // SAFETY: as above.
        sls::SLSWindowIteratorGetParentID().map_or(0, |f| unsafe { f(self.0) })
    }

    pub(super) fn tags(&self) -> u64 {
        // SAFETY: as above.
        sls::SLSWindowIteratorGetTags().map_or(0, |f| unsafe { f(self.0) })
    }

    pub(super) fn attributes(&self) -> u64 {
        // SAFETY: as above.
        sls::SLSWindowIteratorGetAttributes().map_or(0, |f| unsafe { f(self.0) })
    }

    pub(super) fn level(&self) -> i32 {
        // SAFETY: as above.
        SLSWindowIteratorGetLevel().map_or(0, |f| unsafe { f(self.0) })
    }

    /// `window_suitable` (BR-WIN-01) for the current window.
    pub(super) fn suitable(&self) -> bool {
        mbar_core::borders::window_suitable(self.parent(), self.attributes(), self.tags())
    }

    /// Element 0 of `SLSWindowIteratorGetCornerRadii` as SInt32 (macOS 26+ only, like
    /// JankyBorders' `load_symbols`).
    pub(super) fn corner_radius(&self) -> Option<i32> {
        if !crate::sys::util::os_at_least(26, 0) {
            return None;
        }
        let f = SLSWindowIteratorGetCornerRadii()?;
        // SAFETY: as above; the returned array follows the Copy rule.
        let radii: CFRetained<CFArray> = unsafe { owned(f(self.0)) }?;
        let first = crate::sys::util::array_items(&radii).into_iter().next()?;
        crate::sys::util::cf_i64(&first).map(|r| r as i32)
    }
}

/// Runs `SLSWindowQueryWindows(cid, windows, 0)` and calls `f` for every window of the
/// result in SkyLight's order (front to back) until it returns `false`.
pub(super) fn query_windows(cid: i32, windows: &CFArray, mut f: impl FnMut(&WindowIter) -> bool) {
    let (Some(query), Some(copy), Some(advance)) = (
        sls::SLSWindowQueryWindows(),
        sls::SLSWindowQueryResultCopyWindows(),
        sls::SLSWindowIteratorAdvance(),
    ) else {
        return;
    };
    // SAFETY: SkyLight query objects are CF objects following the Create/Copy rule; the
    // iterator pointer is only used while `iter` is alive.
    unsafe {
        let Some(q): Option<CFRetained<CFType>> = owned(query(cid, windows, 0)) else {
            return;
        };
        let Some(iter): Option<CFRetained<CFType>> = owned(copy(CFRetained::as_ptr(&q).as_ptr()))
        else {
            return;
        };
        let it = WindowIter(CFRetained::as_ptr(&iter).as_ptr());
        while advance(it.0) {
            if !f(&it) {
                break;
            }
        }
    }
}

/// `SLSCopyWindowsWithOptionsAndTags(cid, owner, spaces, 0x2, &1, &0)` (fresh tag
/// variables on every call, spec §14 open question 4).
pub(super) fn windows_on_spaces(cid: i32, owner: i32, sids: &[u64]) -> Option<CFRetained<CFArray>> {
    let f = sls::SLSCopyWindowsWithOptionsAndTags()?;
    let spaces = sid_array(sids);
    let mut set_tags: u64 = 1;
    let mut clear_tags: u64 = 0;
    // SAFETY: valid connection, a CFArray of CFNumber space ids and tag in/out pointers;
    // Copy rule for the result.
    unsafe {
        owned(f(
            cid,
            owner as u32,
            spaces.as_opaque(),
            WINDOW_LIST_OPTIONS,
            &mut set_tags,
            &mut clear_tags,
        ))
    }
}

/// One SkyLight transaction (`SLSTransactionCreate` … `SLSTransactionCommit(t, 0)`).
pub(super) struct Transaction(CFRetained<CFType>);

impl Transaction {
    /// `SLSTransactionCreate(cid)`; `None` when SkyLight returns NULL.
    pub(super) fn new(cid: i32) -> Option<Transaction> {
        let f = SLSTransactionCreate()?;
        // SAFETY: valid connection; Create rule.
        unsafe { owned(f(cid)) }.map(Transaction)
    }

    fn ptr(&self) -> *const CFType {
        CFRetained::as_ptr(&self.0).as_ptr()
    }

    pub(super) fn move_with_group(&self, wid: u32, origin: CGPoint) {
        if let Some(f) = SLSTransactionMoveWindowWithGroup() {
            // SAFETY: a live transaction; CGPoint by value.
            unsafe { f(self.ptr(), wid, origin) };
        }
    }

    pub(super) fn set_transform(&self, wid: u32, transform: CGAffineTransform) {
        if let Some(f) = SLSTransactionSetWindowTransform() {
            // SAFETY: a live transaction; the 48-byte struct is passed by value as in C.
            unsafe { f(self.ptr(), wid, 0, 0, transform) };
        }
    }

    pub(super) fn set_level(&self, wid: u32, level: i32) {
        if let Some(f) = SLSTransactionSetWindowLevel() {
            // SAFETY: a live transaction.
            unsafe { f(self.ptr(), wid, level) };
        }
    }

    pub(super) fn set_sub_level(&self, wid: u32, sub_level: i32) {
        if let Some(f) = SLSTransactionSetWindowSubLevel() {
            // SAFETY: a live transaction.
            unsafe { f(self.ptr(), wid, sub_level) };
        }
    }

    /// `order` +1 above, −1 below, 0 out, relative to `relative_to`.
    pub(super) fn order(&self, wid: u32, order: i32, relative_to: u32) {
        if let Some(f) = SLSTransactionOrderWindow() {
            // SAFETY: a live transaction.
            unsafe { f(self.ptr(), wid, order, relative_to) };
        }
    }

    /// `SLSTransactionCommit(t, 0)` (asynchronous), then release.
    pub(super) fn commit(self) {
        if let Some(f) = SLSTransactionCommit() {
            // SAFETY: a live transaction.
            unsafe { f(self.ptr(), 0) };
        }
    }
}
