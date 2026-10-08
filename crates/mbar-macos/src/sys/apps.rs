//! Running-application lookup, distributed notifications and the "open the app again"
//! hand-off for mbar.app.

use std::path::PathBuf;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationDelegate, NSRunningApplication,
};
use objc2_foundation::{NSDistributedNotificationCenter, NSString};

/// Whether a process of the app `bundle_id` whose executable is named `executable` runs
/// in this session, this process excluded. The bundle's daemon (`Contents/MacOS/mbar`)
/// registers with the same bundle identifier as the UI, so the identifier alone does
/// not tell them apart.
pub fn is_app_running(bundle_id: &str, executable: &str) -> bool {
    running_app(bundle_id, executable).is_some()
}

fn running_app(bundle_id: &str, executable: &str) -> Option<Retained<NSRunningApplication>> {
    let id = NSString::from_str(bundle_id);
    let me = std::process::id() as i32;
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&id)
        .iter()
        .filter(|app| app.processIdentifier() != me)
        .find(|app| {
            app.executableURL()
                .and_then(|url| url.lastPathComponent())
                .is_some_and(|name| name.to_string() == executable)
        })
}

/// Bundle identifier and root of the app whose UI a reopen should show.
static REOPEN_TARGET: OnceLock<(String, PathBuf)> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `ReopenDelegate` has no Drop impl
    // and implements only an optional NSApplicationDelegate method.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MbarReopenDelegate"]
    struct ReopenDelegate;

    unsafe impl NSObjectProtocol for ReopenDelegate {}

    unsafe impl NSApplicationDelegate for ReopenDelegate {
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            show_ui();
            false
        }
    }
);

/// Activates the running management UI or launches a new one (`open -n -a <bundle>`).
fn show_ui() {
    let Some((bundle_id, root)) = REOPEN_TARGET.get() else {
        return;
    };
    if let Some(ui) = running_app(bundle_id, "mbar-ui") {
        #[allow(deprecated)]
        ui.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
        return;
    }
    if let Err(e) = std::process::Command::new("/usr/bin/open")
        .arg("-n")
        .arg("-a")
        .arg(root)
        .spawn()
    {
        log::warn!("cannot open {}: {e}", root.display());
    }
}

/// The daemon inside mbar.app shares the bundle identifier with the UI, so Finder and
/// `open` treat "open mbar.app" as reopening the running daemon. This delegate forwards
/// that to the UI. Call once on the main thread, before the run loop.
pub fn forward_reopen_to_ui(mtm: MainThreadMarker, bundle_id: &str, bundle_root: PathBuf) {
    if REOPEN_TARGET
        .set((bundle_id.to_string(), bundle_root))
        .is_err()
    {
        return;
    }
    // SAFETY: `init` is NSObject's designated initializer; the class adds no ivars.
    let delegate: Retained<ReopenDelegate> = unsafe { msg_send![ReopenDelegate::alloc(mtm), init] };
    NSApplication::sharedApplication(mtm).setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // NSApplication holds its delegate weakly; this one lives as long as the process.
    std::mem::forget(delegate);
}

/// Posts a distributed notification without object or user info, delivered immediately.
pub fn post_distributed(name: &str) {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(name);
    unsafe {
        center.postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true)
    };
}
