//! `wifi_change` (`docs/spec/events.md` §5.9): SCDynamicStore notifications for
//! `.*/Network/Global/IPv4` (no de-dup), SSID from CoreWLAN.
//!
//! macOS 14+ only returns the SSID to processes with Location Services authorization;
//! without it CoreWLAN answers nil. mbar then falls back to `ipconfig getsummary <if>`
//! (works on macOS 14; on 15+ it prints `<redacted>` unless `ipconfig setverbose 1` was run
//! once as root) and finally to `""` like SketchyBar.

use crate::sys::util::{cfstr, owned};
use crate::sys::{Sink, SysEvent};
use objc2_core_foundation::{
    kCFRunLoopDefaultMode, CFArray, CFRetained, CFRunLoop, CFRunLoopSource, CFString, CFType,
};
use objc2_core_wlan::CWWiFiClient;
use std::ffi::c_void;
use std::sync::Mutex;

#[repr(C)]
struct SCDynamicStoreContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type SCDynamicStoreCallBack = unsafe extern "C" fn(*const CFType, *const CFArray, *mut c_void);

#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCDynamicStoreCreate(
        allocator: *const c_void,
        name: *const CFString,
        callout: Option<SCDynamicStoreCallBack>,
        context: *mut SCDynamicStoreContext,
    ) -> *const CFType;
    fn SCDynamicStoreSetNotificationKeys(
        store: *const CFType,
        keys: *const CFArray,
        patterns: *const CFArray,
    ) -> u8;
    fn SCDynamicStoreCreateRunLoopSource(
        allocator: *const c_void,
        store: *const CFType,
        order: isize,
    ) -> *const CFRunLoopSource;
}

struct Installed {
    _store: CFRetained<CFType>,
    _source: CFRetained<CFRunLoopSource>,
}
// SAFETY: both are CF objects only kept alive here; CF retain/release is thread safe.
unsafe impl Send for Installed {}

struct State {
    sink: Option<Sink>,
    installed: Option<Installed>,
}

static STATE: Mutex<State> = Mutex::new(State {
    sink: None,
    installed: None,
});

/// SSID bytes as SketchyBar reads them: up to the first NUL, lossy UTF-8.
pub fn ssid_from_bytes(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Extracts the SSID from `ipconfig getsummary <if>` output (`  SSID : name`); ignores
/// `<redacted>`.
pub fn parse_ipconfig_ssid(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim_start();
        let rest = l.strip_prefix("SSID : ")?;
        let v = rest.trim_end();
        (!v.is_empty() && v != "<redacted>").then(|| v.to_string())
    })
}

fn ipconfig_ssid(interface: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/ipconfig")
        .args(["getsummary", interface])
        .output()
        .ok()?;
    parse_ipconfig_ssid(&String::from_utf8_lossy(&out.stdout))
}

/// Current SSID (empty when unknown).
pub fn current_ssid() -> String {
    // SAFETY: CoreWLAN accessors without preconditions.
    unsafe {
        let Some(iface) = CWWiFiClient::sharedWiFiClient().interface() else {
            return String::new();
        };
        if let Some(data) = iface.ssidData() {
            return ssid_from_bytes(&data.to_vec());
        }
        if !iface.powerOn() {
            return String::new();
        }
        let name = iface
            .interfaceName()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "en0".to_string());
        ipconfig_ssid(&name).unwrap_or_default()
    }
}

/// Current RSSI in dBm (`None` without a Wi-Fi interface).
pub fn current_rssi() -> Option<i64> {
    // SAFETY: CoreWLAN accessors without preconditions.
    unsafe {
        let iface = CWWiFiClient::sharedWiFiClient().interface()?;
        let r = iface.rssiValue() as i64;
        (r != 0).then_some(r)
    }
}

fn post() {
    let sink = STATE.lock().unwrap_or_else(|e| e.into_inner()).sink.clone();
    if let Some(sink) = sink {
        sink(SysEvent::WifiChange(current_ssid()));
    }
}

unsafe extern "C" fn store_callback(_store: *const CFType, _keys: *const CFArray, _info: *mut c_void) {
    post();
}

/// Installs the SCDynamicStore watcher on the main run loop (idempotent).
pub fn start(sink: Sink) {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    s.sink = Some(sink);
    if s.installed.is_some() {
        return;
    }
    let name = cfstr("network");
    let pattern = cfstr(".*/Network/Global/IPv4");
    let patterns = CFArray::from_retained_objects(&[pattern]);
    // SAFETY: valid CF arguments; the Create results are owned and kept in `Installed`.
    unsafe {
        let store: Option<CFRetained<CFType>> = owned(SCDynamicStoreCreate(
            std::ptr::null(),
            CFRetained::as_ptr(&name).as_ptr(),
            Some(store_callback),
            std::ptr::null_mut(),
        ));
        let Some(store) = store else {
            log::warn!("SCDynamicStoreCreate failed");
            return;
        };
        let sp = CFRetained::as_ptr(&store).as_ptr() as *const CFType;
        SCDynamicStoreSetNotificationKeys(sp, std::ptr::null(), patterns.as_opaque());
        let Some(source): Option<CFRetained<CFRunLoopSource>> =
            owned(SCDynamicStoreCreateRunLoopSource(std::ptr::null(), sp, 0))
        else {
            return;
        };
        if let Some(main) = CFRunLoop::main() {
            main.add_source(Some(&source), kCFRunLoopDefaultMode);
        }
        s.installed = Some(Installed {
            _store: store,
            _source: source,
        });
    }
}

/// `forced_network_event` (`--update`, `--trigger wifi_change`). Returns the SSID.
pub fn forced() -> String {
    let ssid = current_ssid();
    let sink = STATE.lock().unwrap_or_else(|e| e.into_inner()).sink.clone();
    if let Some(sink) = sink {
        sink(SysEvent::WifiChange(ssid.clone()));
    }
    ssid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssid_bytes() {
        assert_eq!(ssid_from_bytes(b"Home\0junk"), "Home");
        assert_eq!(ssid_from_bytes(b"Caf\xc3\xa9"), "Café");
        assert_eq!(ssid_from_bytes(b""), "");
    }

    #[test]
    fn ipconfig_parse() {
        let t = "<dictionary> {\n  BSSID : aa:bb\n  SSID : My Net\n  Security : WPA2\n}";
        assert_eq!(parse_ipconfig_ssid(t).as_deref(), Some("My Net"));
        assert_eq!(parse_ipconfig_ssid("  SSID : <redacted>\n"), None);
        assert_eq!(parse_ipconfig_ssid("nothing"), None);
    }
}
