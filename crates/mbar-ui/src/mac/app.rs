//! Bundle location, activation policy, relaunch and distributed notifications.

use std::path::{Path, PathBuf};
use std::process::Command;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSString};

pub fn bundle() -> Option<mbar_app::bundle::AppBundle> {
    let exe = std::env::current_exe().ok()?;
    let root = mbar_app::bundle::bundle_root_from_exe(&exe)?;
    mbar_app::bundle::read_bundle(&root)
}

/// No Dock icon / menu bar while only the Sparkle dialog is shown.
pub fn set_accessory(accessory: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let policy = if accessory {
        NSApplicationActivationPolicy::Accessory
    } else {
        NSApplicationActivationPolicy::Regular
    };
    NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
}

pub fn activate() {
    if let Some(mtm) = MainThreadMarker::new() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}

/// Copies the running bundle to /Applications/mbar.app (replacing an older copy).
pub fn move_to_applications(src: &Path) -> Result<PathBuf, String> {
    let dst = PathBuf::from("/Applications/mbar.app");
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(|e| format!("remove {}: {e}", dst.display()))?;
    }
    let st = Command::new("/usr/bin/ditto")
        .arg(src)
        .arg(&dst)
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(format!("ditto exited with {st}"));
    }
    Ok(dst)
}

pub fn relaunch(path: &Path) -> ! {
    let _ = Command::new("/usr/bin/open").arg("-n").arg(path).spawn();
    std::process::exit(0);
}

/// Calls `f` on the main thread whenever the distributed notification `name` arrives.
pub fn on_distributed(name: &str, f: impl Fn() + 'static) {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(name);
    let block = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| f());
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(&name),
            None,
            Some(&objc2_foundation::NSOperationQueue::mainQueue()),
            &block,
        )
    };
    std::mem::forget(observer);
}
