//! Private SkyLight / DisplayServices / CoreGraphics symbols (resolved with `dlsym`, so a
//! missing symbol on some macOS version degrades instead of failing to load the binary).
//!
//! Signatures follow SketchyBar (`src/misc/extern.h`) and yabai.

#![allow(non_snake_case)]

use super::util::{private_fns, COLOR_SYNC, DISPLAY_SERVICES, SKYLIGHT};
use objc2_core_foundation::{CFArray, CFString, CFType, CGRect, CFUUID};
use objc2_core_graphics::CGImage;
use std::ffi::c_void;

/// `SLSRegisterNotifyProc` handler: `(event, data, data_length, context)`.
pub(crate) type NotifyProc = unsafe extern "C" fn(u32, *mut c_void, usize, *mut c_void);

private_fns! { SKYLIGHT =>
    fn SLSMainConnectionID() -> i32;
    fn SLSGetSpaceManagementMode(i32) -> i32;
    fn SLSCopyManagedDisplays(i32) -> *const CFArray;
    fn SLSCopyManagedDisplaySpaces(i32) -> *const CFArray;
    fn SLSManagedDisplayGetCurrentSpace(i32, *const CFString) -> u64;
    fn SLSGetActiveSpace(i32) -> u64;
    fn SLSCopyManagedDisplayForSpace(i32, u64) -> *const CFString;
    fn SLSCopyActiveMenuBarDisplayIdentifier(i32) -> *const CFString;
    fn SLSSpaceGetType(i32, u64) -> i32;
    fn SLSGetMenuBarAutohideEnabled(i32, *mut i32) -> i32;
    fn SLSGetRevealedMenuBarBounds(*mut CGRect, i32, u64) -> i32;
    fn SLSRegisterNotifyProc(NotifyProc, u32, *mut c_void) -> i32;
    fn SLSCopyWindowsWithOptionsAndTags(i32, u32, *const CFArray, u32, *mut u64, *mut u64) -> *const CFArray;
    fn SLSWindowQueryWindows(i32, *const CFArray, u32) -> *const CFType;
    fn SLSWindowQueryResultCopyWindows(*const CFType) -> *const CFType;
    fn SLSWindowIteratorGetCount(*const CFType) -> i32;
    fn SLSWindowIteratorAdvance(*const CFType) -> bool;
    fn SLSWindowIteratorGetParentID(*const CFType) -> u32;
    fn SLSWindowIteratorGetWindowID(*const CFType) -> u32;
    fn SLSWindowIteratorGetTags(*const CFType) -> u64;
    fn SLSWindowIteratorGetAttributes(*const CFType) -> u64;
    fn SLSGetWindowOwner(i32, u32, *mut i32) -> i32;
    fn SLSConnectionGetPID(i32, *mut libc::pid_t) -> i32;
    fn SLSRequestNotificationsForWindows(i32, *const u32, i32) -> i32;
    fn SLSHWCaptureSpace(i64, i64, i64) -> *const CFArray;
    fn SLSHWCaptureWindowList(i32, *const u32, i32, u32) -> *const CFArray;
    fn SLSCaptureWindowsContentsToRectWithOptions(i32, *const u32, bool, CGRect, u32, *mut *const CGImage) -> i32;
    fn SLSGetScreenRectForWindow(i32, u32, *mut CGRect) -> i32;
}

/// DisplayServices brightness callback (a `CFNotificationCallback`).
pub(crate) type BrightnessCallback =
    unsafe extern "C" fn(*const c_void, *mut c_void, *const CFString, *const c_void, *const c_void);

private_fns! { DISPLAY_SERVICES =>
    fn DisplayServicesGetBrightness(u32, *mut f32) -> i32;
    fn DisplayServicesCanChangeBrightness(u32) -> bool;
    fn DisplayServicesRegisterForBrightnessChangeNotifications(u32, *mut c_void, BrightnessCallback) -> i32;
    fn DisplayServicesUnregisterForBrightnessChangeNotifications(u32, *mut c_void) -> i32;
}

private_fns! { COLOR_SYNC =>
    fn CGDisplayCreateUUIDFromDisplayID(u32) -> *const CFUUID;
    fn CGDisplayGetDisplayIDFromUUID(*const CFUUID) -> u32;
}

/// The process' main SkyLight connection (0 if SkyLight is unavailable).
pub(crate) fn cid() -> i32 {
    static CID: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *CID.get_or_init(|| match SLSMainConnectionID() {
        // SAFETY: no arguments; returns the per-process connection id.
        Some(f) => unsafe { f() },
        None => 0,
    })
}
