//! `volume_change` (`docs/spec/events.md` §5.6): CoreAudio listeners on the default output
//! device (mute + volume scalar, output scope, elements main (0) and 1), re-attached when the
//! default output device changes; de-duplicated with a strict 0.01 threshold.

use crate::sys::{Sink, SysEvent};
use objc2_core_audio::{
    kAudioDevicePropertyMute, kAudioDevicePropertyVolumeScalar,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectRemovePropertyListener,
};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;

struct State {
    sink: Option<Sink>,
    device: AudioObjectID,
    last: f32,
}

static STATE: Mutex<State> = Mutex::new(State {
    sink: None,
    device: 0,
    last: -1.0,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn address(selector: u32, scope: u32, element: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: element,
    }
}

/// Reads a fixed-size property value; `None` on error.
fn get<T: Copy + Default>(object: AudioObjectID, addr: AudioObjectPropertyAddress) -> Option<T> {
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `addr`, `size` and `value` are valid for the call; `size` matches `T`.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    (status == 0).then_some(value)
}

/// The default output device id (0 if none).
pub fn default_output_device() -> AudioObjectID {
    get::<AudioObjectID>(
        kAudioObjectSystemObject as AudioObjectID,
        address(
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
            kAudioObjectPropertyElementMain,
        ),
    )
    .unwrap_or(0)
}

/// The volume rule of `volume.c:handler`.
pub fn effective_volume(
    muted_main: bool,
    muted_left: bool,
    volume_main: f32,
    volume_left: f32,
) -> f32 {
    if volume_left > 0.0 {
        if muted_left || muted_main {
            0.0
        } else {
            volume_left
        }
    } else if muted_main {
        0.0
    } else {
        volume_main
    }
}

/// Strict ±0.01 de-duplication (`v > last + 0.01 || v < last - 0.01`).
pub fn changed(last: f32, v: f32) -> bool {
    v > last + 0.01 || v < last - 0.01
}

/// INFO text of `volume_change` / `brightness_change`: `(int)(v*100 + 0.5)`.
pub fn percent(v: f32) -> i32 {
    (v * 100.0 + 0.5) as i32
}

/// Reads `(volume 0..1 with mute applied, muted)` from a device.
pub fn read_device(device: AudioObjectID) -> (f32, bool) {
    let scope = kAudioObjectPropertyScopeOutput;
    let mute =
        |el| get::<u32>(device, address(kAudioDevicePropertyMute, scope, el)).unwrap_or(0) != 0;
    let vol = |el| {
        get::<f32>(device, address(kAudioDevicePropertyVolumeScalar, scope, el)).unwrap_or(0.0)
    };
    let (mm, ml, vm, vl) = (mute(0), mute(1), vol(0), vol(1));
    (effective_volume(mm, ml, vm, vl), mm || (vl > 0.0 && ml))
}

/// Current default-output volume (for providers and forced reads).
pub fn read() -> (f32, bool) {
    read_device(default_output_device())
}

fn device_addresses() -> [AudioObjectPropertyAddress; 4] {
    let scope = kAudioObjectPropertyScopeOutput;
    [
        address(kAudioDevicePropertyMute, scope, 0),
        address(kAudioDevicePropertyMute, scope, 1),
        address(kAudioDevicePropertyVolumeScalar, scope, 0),
        address(kAudioDevicePropertyVolumeScalar, scope, 1),
    ]
}

unsafe extern "C-unwind" fn volume_listener(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    _client: *mut c_void,
) -> i32 {
    handle();
    0
}

unsafe extern "C-unwind" fn device_listener(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    _client: *mut c_void,
) -> i32 {
    let new = default_output_device();
    let old = {
        let mut s = state();
        let old = s.device;
        s.device = new;
        s.last = -1.0;
        old
    };
    attach(old, false);
    attach(new, true);
    handle();
    0
}

fn attach(device: AudioObjectID, add: bool) {
    if device == 0 {
        return;
    }
    for addr in device_addresses() {
        // SAFETY: valid address; the listener is a static function with no client data.
        unsafe {
            if add {
                AudioObjectAddPropertyListener(
                    device,
                    NonNull::from(&addr),
                    Some(volume_listener),
                    std::ptr::null_mut(),
                );
            } else {
                AudioObjectRemovePropertyListener(
                    device,
                    NonNull::from(&addr),
                    Some(volume_listener),
                    std::ptr::null_mut(),
                );
            }
        }
    }
}

fn handle() {
    let (device, sink) = {
        let s = state();
        (s.device, s.sink.clone())
    };
    let (v, _) = read_device(device);
    let post = {
        let mut s = state();
        if changed(s.last, v) {
            s.last = v;
            true
        } else {
            false
        }
    };
    if post {
        if let Some(sink) = sink {
            sink(SysEvent::VolumeChange(v));
        }
    }
}

/// `begin_receiving_volume_events` (idempotent). Listener callbacks run on a CoreAudio
/// thread and call the sink from there.
pub fn start(sink: Sink) {
    let device = {
        let mut s = state();
        if s.sink.is_some() {
            return;
        }
        s.sink = Some(sink);
        s.device = default_output_device();
        s.device
    };
    attach(device, true);
    let sys = address(
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    );
    // SAFETY: valid address on the system object; static listener without client data.
    unsafe {
        AudioObjectAddPropertyListener(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&sys),
            Some(device_listener),
            std::ptr::null_mut(),
        );
    }
}

/// `forced_volume_event` (`--update`, `--trigger volume_change`): resets the de-dup state
/// and re-reads; posts if listeners are installed. Returns the value read.
pub fn forced() -> f32 {
    state().last = -1.0;
    handle();
    let device = state().device;
    read_device(device).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_rule() {
        assert_eq!(effective_volume(false, false, 0.5, 0.0), 0.5);
        assert_eq!(effective_volume(true, false, 0.5, 0.0), 0.0);
        assert_eq!(effective_volume(false, false, 0.5, 0.3), 0.3);
        assert_eq!(effective_volume(false, true, 0.5, 0.3), 0.0);
        assert_eq!(effective_volume(true, false, 0.5, 0.3), 0.0);
    }

    #[test]
    fn dedup() {
        assert!(changed(-1.0, 0.0));
        assert!(!changed(0.5, 0.505));
        assert!(changed(0.5, 0.52));
        assert!(changed(0.5, 0.48));
        assert_eq!(percent(0.444), 44);
        assert_eq!(percent(0.446), 45);
        assert_eq!(percent(1.0), 100);
    }
}
