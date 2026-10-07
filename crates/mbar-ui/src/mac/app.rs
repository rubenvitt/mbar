//! Bundle location, activation policy, relaunch and distributed notifications.

use std::path::{Path, PathBuf};
use std::process::Command;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSString};

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

/// Copies the running bundle to /Applications/mbar.app, replacing an older copy only
/// once the new one is complete (copy to a temporary name, then swap by renaming).
pub fn move_to_applications(src: &Path) -> Result<PathBuf, String> {
    let dst = PathBuf::from("/Applications/mbar.app");
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canonical(src) == canonical(&dst) {
        return Err("mbar.app already runs from /Applications".into());
    }
    let tmp = PathBuf::from(format!("/Applications/.mbar.app.{}", std::process::id()));
    let old = PathBuf::from(format!(
        "/Applications/.mbar.app.old.{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let st = Command::new("/usr/bin/ditto")
        .arg(src)
        .arg(&tmp)
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("ditto exited with {st}"));
    }
    let had_old = dst.exists();
    if had_old {
        std::fs::rename(&dst, &old).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp);
            format!("move {} aside: {e}", dst.display())
        })?;
    }
    if let Err(e) = std::fs::rename(&tmp, &dst) {
        if had_old {
            let _ = std::fs::rename(&old, &dst);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("install {}: {e}", dst.display()));
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(dst)
}

/// Ends the app through AppKit (`-[NSApp terminate:]`), so termination hooks such as
/// Sparkle's install-on-quit run.
pub fn terminate() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).terminate(None);
    } else {
        std::process::exit(0);
    }
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
