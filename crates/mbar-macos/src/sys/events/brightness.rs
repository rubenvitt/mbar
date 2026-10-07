//! `brightness_change` (`docs/spec/events.md` §5.7): private DisplayServices brightness
//! notifications per display that can change brightness; one global de-dup state.

use crate::sys::skylight as ds;
use crate::sys::{displays, Sink, SysEvent};
use objc2_core_foundation::CFString;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::Mutex;

struct State {
    sink: Option<Sink>,
    last: f32,
    registered: Option<HashSet<u32>>,
}

static STATE: Mutex<State> = Mutex::new(State {
    sink: None,
    last: -1.0,
    registered: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// `DisplayServicesGetBrightness(did)` (0..1); `None` if unsupported.
pub fn read(did: u32) -> Option<f32> {
    let f = ds::DisplayServicesGetBrightness()?;
    let mut b: f32 = 0.0;
    // SAFETY: valid out pointer.
    let err = unsafe { f(did, &mut b) };
    (err == 0).then_some(b)
}

fn can_change(did: u32) -> bool {
    match ds::DisplayServicesCanChangeBrightness() {
        // SAFETY: plain value argument.
        Some(f) => unsafe { f(did) },
        None => false,
    }
}

unsafe extern "C" fn brightness_callback(
    _center: *const c_void,
    observer: *mut c_void,
    _name: *const CFString,
    _object: *const c_void,
    _user_info: *const c_void,
) {
    // The observer pointer carries the display id (as in SketchyBar).
    let did = observer as usize as u32;
    handle(did, false);
}

fn handle(did: u32, force: bool) -> f32 {
    let b = read(did).unwrap_or(0.0);
    let sink = {
        let mut s = state();
        if force || (b - s.last).abs() > 0.01 {
            s.last = b;
            s.sink.clone()
        } else {
            None
        }
    };
    if let Some(sink) = sink {
        sink(SysEvent::BrightnessChange(b));
    }
    b
}

fn register(did: u32) {
    if !can_change(did) {
        return;
    }
    let Some(f) = ds::DisplayServicesRegisterForBrightnessChangeNotifications() else {
        return;
    };
    let fresh = state()
        .registered
        .get_or_insert_with(HashSet::new)
        .insert(did);
    if fresh {
        // SAFETY: the callback is a static function; the observer is the display id.
        unsafe { f(did, did as usize as *mut c_void, brightness_callback) };
    }
}

/// `begin_receiving_brightness_events` (idempotent).
pub fn start(sink: Sink) {
    {
        let mut s = state();
        if s.sink.is_some() {
            return;
        }
        s.sink = Some(sink);
    }
    for did in displays::active_display_ids() {
        register(did);
    }
}

/// Reconfiguration hook: a display was added (registers if brightness events are on).
pub(crate) fn display_added(did: u32) {
    if state().sink.is_some() {
        register(did);
    }
}

/// Reconfiguration hook: a display was removed.
pub(crate) fn display_removed(did: u32) {
    let was = state()
        .registered
        .as_mut()
        .map(|r| r.remove(&did))
        .unwrap_or(false);
    if was {
        if let Some(f) = ds::DisplayServicesUnregisterForBrightnessChangeNotifications() {
            // SAFETY: same display/observer pair as registered.
            unsafe { f(did, did as usize as *mut c_void) };
        }
    }
}

/// `forced_brightness_event` (`--update` only): reads the active display and always posts
/// (if started). Returns the value read.
pub fn forced() -> f32 {
    state().last = -1.0;
    handle(displays::active_display_id(), true)
}
