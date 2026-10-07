//! Running-application lookup and distributed notifications for the update check.

use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSDistributedNotificationCenter, NSString};

/// Whether a process of the app `bundle_id` whose executable is named `executable` runs
/// in this session, this process excluded. The bundle's daemon (`Contents/MacOS/mbar`)
/// registers with the same bundle identifier as the UI, so the identifier alone does
/// not tell them apart.
pub fn is_app_running(bundle_id: &str, executable: &str) -> bool {
    let id = NSString::from_str(bundle_id);
    let me = std::process::id() as i32;
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&id)
        .iter()
        .filter(|app| app.processIdentifier() != me)
        .any(|app| {
            app.executableURL()
                .and_then(|url| url.lastPathComponent())
                .is_some_and(|name| name.to_string() == executable)
        })
}

/// Posts a distributed notification without object or user info, delivered immediately.
pub fn post_distributed(name: &str) {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(name);
    unsafe {
        center.postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true)
    };
}
