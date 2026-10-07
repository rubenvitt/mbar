//! Running-application lookup and distributed notifications for the update check.

use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSDistributedNotificationCenter, NSString};

/// Whether an application with this bundle identifier is running in this session.
pub fn is_app_running(bundle_id: &str) -> bool {
    let id = NSString::from_str(bundle_id);
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&id).count() > 0
}

/// Posts a distributed notification without object or user info, delivered immediately.
pub fn post_distributed(name: &str) {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(name);
    unsafe {
        center.postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true)
    };
}
