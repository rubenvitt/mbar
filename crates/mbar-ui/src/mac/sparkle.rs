//! Sparkle 2 via the Objective-C runtime. Sparkle.framework is embedded in
//! Contents/Frameworks and loaded at runtime (no link-time dependency).
//!
//! The controller is created without starting the updater; `startUpdater:` is called
//! explicitly so its `NSError` can be logged. Sparkle 2.10 refuses to start without
//! `SUPublicEDKey` and would otherwise fail silently (spike notes).

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSBundle, NSError, NSString};

pub struct DelegateIvars {
    on_cycle_end: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the methods match the
    // SPUUpdaterDelegate / SPUStandardUserDriverDelegate selectors of Sparkle 2 and
    // `SparkleDelegate` has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MbarSparkleDelegate"]
    #[ivars = DelegateIvars]
    pub struct SparkleDelegate;

    impl SparkleDelegate {
        #[unsafe(method(updater:didFinishUpdateCycleForUpdateCheck:error:))]
        fn did_finish(&self, _updater: &AnyObject, _check: isize, error: Option<&NSError>) {
            // SUNoUpdateError (1001) is the normal "up to date" outcome.
            if let Some(e) = error.filter(|e| e.code() != 1001) {
                log_warn(&format!("update cycle ended: {}", e.localizedDescription()));
            }
            (self.ivars().on_cycle_end)();
        }

        #[unsafe(method(supportsGentleScheduledUpdateReminders))]
        fn gentle(&self) -> bool {
            true
        }

        // The app may be an accessory (no Dock icon) or in the background; Sparkle's
        // window would otherwise appear without keyboard focus.
        #[unsafe(method(standardUserDriverWillHandleShowingUpdate:forUpdate:state:))]
        fn will_show(&self, _handle: bool, _update: &AnyObject, _state: &AnyObject) {
            super::app::activate();
        }
    }
);

pub struct Updater {
    controller: Retained<AnyObject>,
    _delegate: Retained<SparkleDelegate>,
}

fn load_framework() -> bool {
    let Some(dir) = NSBundle::mainBundle().privateFrameworksPath() else {
        return false;
    };
    let path = dir.stringByAppendingPathComponent(&NSString::from_str("Sparkle.framework"));
    // SAFETY: loads the signed Sparkle.framework from our own bundle; its initializers
    // have no requirements on the caller.
    NSBundle::bundleWithPath(&path).is_some_and(|b| unsafe { b.load() })
}

impl Updater {
    /// Loads Sparkle and starts the updater. `None` outside mbar.app or when Sparkle
    /// refuses to start (the reason is logged). `on_cycle_end` runs on the main thread
    /// after every update cycle (no update, dismissed, failed or installed).
    pub fn start(on_cycle_end: impl Fn() + 'static) -> Option<Updater> {
        let mtm = MainThreadMarker::new()?;
        if !load_framework() {
            log_warn("Sparkle.framework not found (not running from mbar.app)");
            return None;
        }
        let Some(cls) = AnyClass::get(c"SPUStandardUpdaterController") else {
            log_warn("SPUStandardUpdaterController missing in Sparkle.framework");
            return None;
        };
        let delegate = SparkleDelegate::alloc(mtm).set_ivars(DelegateIvars {
            on_cycle_end: Box::new(on_cycle_end),
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let delegate: Retained<SparkleDelegate> = unsafe { msg_send![super(delegate), init] };
        // SAFETY: selectors of SPUStandardUpdaterController (Sparkle 2); the delegates
        // are weak references in Sparkle, `Updater` keeps `delegate` alive.
        let controller: Option<Retained<AnyObject>> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
            msg_send![
                alloc,
                initWithStartingUpdater: false,
                updaterDelegate: &*delegate,
                userDriverDelegate: &*delegate
            ]
        };
        let controller = controller?;
        let updater = Updater {
            controller,
            _delegate: delegate,
        };
        let mut error: *mut NSError = std::ptr::null_mut();
        let u = updater.updater();
        // SAFETY: `-[SPUUpdater startUpdater:]` takes an `NSError **` out parameter.
        let ok: bool = unsafe { msg_send![&*u, startUpdater: &mut error] };
        if !ok {
            // SAFETY: on failure Sparkle sets `error` to an autoreleased NSError or nil.
            let why = unsafe { error.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "unknown error".into());
            log_warn(&format!("Sparkle did not start: {why}"));
            return None;
        }
        Some(updater)
    }

    fn updater(&self) -> Retained<AnyObject> {
        // SAFETY: `-[SPUStandardUpdaterController updater]` returns the SPUUpdater.
        unsafe { msg_send![&*self.controller, updater] }
    }

    /// Background check: Sparkle shows its dialog only when an update is found.
    pub fn check_in_background(&self) {
        let u = self.updater();
        // SAFETY: `-[SPUUpdater checkForUpdatesInBackground]`, no arguments.
        let _: () = unsafe { msg_send![&*u, checkForUpdatesInBackground] };
    }

    /// User-initiated check ("Check for Updates…"): also reports "up to date".
    pub fn check_now(&self) {
        super::app::activate();
        // SAFETY: `-[SPUStandardUpdaterController checkForUpdates:]` takes a sender.
        let _: () =
            unsafe { msg_send![&*self.controller, checkForUpdates: std::ptr::null::<AnyObject>()] };
    }

    pub fn auto_checks(&self) -> bool {
        let u = self.updater();
        // SAFETY: BOOL property of SPUUpdater.
        unsafe { msg_send![&*u, automaticallyChecksForUpdates] }
    }

    pub fn set_auto_checks(&self, on: bool) {
        let u = self.updater();
        // SAFETY: BOOL property setter of SPUUpdater (persisted in the user defaults).
        let _: () = unsafe { msg_send![&*u, setAutomaticallyChecksForUpdates: on] };
    }
}

fn log_warn(msg: &str) {
    eprintln!("mbar-ui: {msg}");
}
