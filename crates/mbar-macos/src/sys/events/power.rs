//! `power_source_change` (`docs/spec/events.md` §5.8) and battery readings for the
//! `battery` provider: IOKit power-source notifications on the main run loop.

use crate::sys::util;
use crate::sys::{Sink, SysEvent};
use objc2_core_foundation::{kCFRunLoopDefaultMode, CFDictionary, CFRetained, CFRunLoop, CFRunLoopSource};
use objc2_io_kit::{
    IOPSCopyPowerSourcesInfo, IOPSCopyPowerSourcesList, IOPSGetPowerSourceDescription,
    IOPSGetProvidingPowerSourceType, IOPSNotificationCreateRunLoopSource,
};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

/// Providing power source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Ac,
    Battery,
}

impl PowerState {
    /// INFO text (`AC` / `BATTERY`).
    pub fn as_str(self) -> &'static str {
        match self {
            PowerState::Ac => "AC",
            PowerState::Battery => "BATTERY",
        }
    }

    /// `IOPSGetProvidingPowerSourceType` string → state (`"UPS Power"` etc. → `None`).
    pub fn from_type(t: &str) -> Option<PowerState> {
        match t {
            "AC Power" => Some(PowerState::Ac),
            "Battery Power" => Some(PowerState::Battery),
            _ => None,
        }
    }
}

/// One internal battery reading.
#[derive(Debug, Clone, PartialEq)]
pub struct BatteryInfo {
    /// 0..100.
    pub percent: u32,
    pub charging: bool,
    /// On AC (charged or charging).
    pub plugged: bool,
    /// Minutes until empty/full when known.
    pub remaining_minutes: Option<i64>,
}

impl BatteryInfo {
    /// `h:mm` or empty (`battery` provider `remaining` key).
    pub fn remaining_text(&self) -> String {
        format_remaining(self.remaining_minutes)
    }
}

/// `h:mm` for a positive number of minutes, empty otherwise.
pub fn format_remaining(minutes: Option<i64>) -> String {
    match minutes {
        Some(m) if m > 0 => format!("{}:{:02}", m / 60, m % 60),
        _ => String::new(),
    }
}

/// Percentage from IOPS capacities (`Current Capacity` / `Max Capacity`), rounded.
pub fn battery_percent(current: i64, max: i64) -> u32 {
    if max <= 0 {
        return 0;
    }
    (((current as f64 / max as f64) * 100.0).round().clamp(0.0, 100.0)) as u32
}

struct State {
    sink: Option<Sink>,
    current: Option<PowerState>,
    listeners: Vec<Arc<dyn Fn() + Send + Sync>>,
    source: Option<SendSource>,
}

struct SendSource(#[allow(dead_code)] CFRetained<CFRunLoopSource>);
// SAFETY: CFRunLoopSource is a thread-safe CF object; we only keep it alive.
unsafe impl Send for SendSource {}

static STATE: Mutex<State> = Mutex::new(State {
    sink: None,
    current: None,
    listeners: Vec::new(),
    source: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Current providing power source.
pub fn providing_power_source() -> Option<PowerState> {
    let blob = IOPSCopyPowerSourcesInfo()?;
    // SAFETY: `blob` is a valid power-sources snapshot.
    let t = unsafe { IOPSGetProvidingPowerSourceType(Some(&blob)) }?;
    PowerState::from_type(&t.to_string())
}

/// The internal battery, if any.
pub fn battery() -> Option<BatteryInfo> {
    let blob = IOPSCopyPowerSourcesInfo()?;
    // SAFETY: `blob` is a valid snapshot; list entries are valid power source handles.
    let list = unsafe { IOPSCopyPowerSourcesList(Some(&blob)) }?;
    for ps in util::array_items(&list) {
        // SAFETY: see above.
        let Some(desc): Option<CFRetained<CFDictionary>> =
            (unsafe { IOPSGetPowerSourceDescription(Some(&blob), Some(&ps)) })
        else {
            continue;
        };
        if util::dict_string(&desc, "Type").as_deref() != Some("InternalBattery") {
            continue;
        }
        let current = util::dict_i64(&desc, "Current Capacity").unwrap_or(0);
        let max = util::dict_i64(&desc, "Max Capacity").unwrap_or(100);
        let charging = util::dict_bool(&desc, "Is Charging").unwrap_or(false);
        let plugged = util::dict_string(&desc, "Power Source State").as_deref() == Some("AC Power");
        let remaining = if charging {
            util::dict_i64(&desc, "Time to Full Charge")
        } else {
            util::dict_i64(&desc, "Time to Empty")
        };
        return Some(BatteryInfo {
            percent: battery_percent(current, max),
            charging,
            plugged,
            remaining_minutes: remaining.filter(|m| *m > 0),
        });
    }
    None
}

fn handle(force: bool) {
    let new = providing_power_source();
    let (sink, listeners) = {
        let mut s = state();
        let post = match new {
            Some(n) if force || s.current != Some(n) => {
                s.current = Some(n);
                true
            }
            _ => false,
        };
        (if post { s.sink.clone() } else { None }, s.listeners.clone())
    };
    if let (Some(sink), Some(n)) = (sink, new) {
        sink(SysEvent::PowerSourceChange(n.as_str().to_string()));
    }
    for l in listeners {
        l();
    }
}

unsafe extern "C-unwind" fn power_callback(_ctx: *mut c_void) {
    handle(false);
}

/// Installs the IOPS run-loop source on the main run loop (idempotent; callable from any
/// thread). `sink` (if given) receives [`SysEvent::PowerSourceChange`] on state changes.
pub fn start(sink: Option<Sink>) {
    let mut s = state();
    if let Some(sink) = sink {
        s.sink = Some(sink);
    }
    if s.source.is_some() {
        return;
    }
    // SAFETY: static callback without context.
    let Some(source) = (unsafe { IOPSNotificationCreateRunLoopSource(Some(power_callback), std::ptr::null_mut()) })
    else {
        log::warn!("IOPSNotificationCreateRunLoopSource failed");
        return;
    };
    if let Some(main) = CFRunLoop::main() {
        // SAFETY: reading an immutable CF constant.
        let mode = unsafe { kCFRunLoopDefaultMode };
        main.add_source(Some(&source), mode);
    }
    s.source = Some(SendSource(source));
}

/// Registers a callback run (on the main thread) after every IOPS change notification
/// (battery percentage changes included). Used by the `battery` provider.
pub fn add_change_listener(f: Arc<dyn Fn() + Send + Sync>) {
    start(None);
    state().listeners.push(f);
}

/// `forced_power_event` (`--update`, `--trigger power_source_change`): always posts the
/// current AC/BATTERY state. Returns it.
pub fn forced() -> Option<PowerState> {
    state().current = None;
    handle(true);
    providing_power_source()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_types() {
        assert_eq!(PowerState::from_type("AC Power"), Some(PowerState::Ac));
        assert_eq!(PowerState::from_type("Battery Power"), Some(PowerState::Battery));
        assert_eq!(PowerState::from_type("UPS Power"), None);
        assert_eq!(PowerState::Battery.as_str(), "BATTERY");
    }

    #[test]
    fn battery_math() {
        assert_eq!(battery_percent(50, 100), 50);
        assert_eq!(battery_percent(4000, 5000), 80);
        assert_eq!(battery_percent(1, 0), 0);
        assert_eq!(format_remaining(Some(125)), "2:05");
        assert_eq!(format_remaining(Some(-1)), "");
        assert_eq!(format_remaining(None), "");
    }
}
