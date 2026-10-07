//! OS event sources (`docs/spec/events.md` §2, §5, §9): NSWorkspace and distributed
//! notifications, display reconfiguration, power source, Wi-Fi, and the lazily started
//! volume / brightness / media / space-window sources.
//!
//! [`SystemEvents::start`] installs everything SketchyBar installs unconditionally at
//! startup. The lazy sources (`volume_change`, `brightness_change`, `media_change`,
//! `space_windows_change`) are started on the first subscription through
//! [`SystemEvents::start_volume_events`] etc. and are never stopped (SketchyBar keeps them
//! across hotloads, `events.md` §9.4).

pub mod brightness;
pub mod media;
pub mod power;
pub mod volume;
pub mod wifi;

use super::{displays, spaces, Sink, SysEvent};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification,
    NSWorkspaceApplicationKey, NSWorkspaceDidActivateApplicationNotification,
    NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSJSONSerialization, NSJSONWritingOptions, NSNotification,
    NSNotificationCenter, NSString,
};
use std::collections::HashMap;
use std::ptr::NonNull;

/// A block-based notification observer; removed from its center on drop.
pub(crate) struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Observer {
    /// Observes `name` (all names when `None`) on `center`; `f` runs on the posting thread
    /// (the main thread for workspace and distributed notifications).
    pub(crate) fn new(
        center: &NSNotificationCenter,
        name: Option<&NSString>,
        f: impl Fn(&NSNotification) + 'static,
    ) -> Observer {
        let block = RcBlock::new(move |n: NonNull<NSNotification>| {
            // SAFETY: the notification center passes a valid notification for the
            // duration of the callback.
            let n = unsafe { n.as_ref() };
            f(n);
        });
        // SAFETY: `name` is an NSString, no object filter, `None` queue = synchronous
        // delivery on the posting thread; the block is copied by the center.
        let token = unsafe { center.addObserverForName_object_queue_usingBlock(name, None, None, &block) };
        Observer {
            center: center.retain(),
            token,
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        let obj: &AnyObject = self.token.as_ref();
        // SAFETY: `token` is the observer object returned by `addObserverForName:…`.
        unsafe { self.center.removeObserver(obj) };
    }
}

/// `NSRunningApplication` → `(localizedName, bundleIdentifier, pid)`.
pub fn app_identity(app: &NSRunningApplication) -> (Option<String>, Option<String>, i32) {
    (
        app.localizedName().map(|s| s.to_string()),
        app.bundleIdentifier().map(|s| s.to_string()),
        app.processIdentifier(),
    )
}

/// The frontmost application (`forced_front_app_event`).
pub fn front_app() -> Option<(Option<String>, Option<String>, i32)> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|a| app_identity(&a))
}

/// Pretty-printed JSON of a notification's `userInfo` (`NSJSONWritingPrettyPrinted`), only
/// if it is a valid JSON object and non-empty (`events.md` §3.5).
fn user_info_json(n: &NSNotification) -> Option<String> {
    let info = n.userInfo()?;
    let obj: &AnyObject = info.as_ref();
    // SAFETY: `obj` is an NSDictionary, a valid argument for both calls.
    unsafe {
        if !NSJSONSerialization::isValidJSONObject(obj) {
            return None;
        }
        let data =
            NSJSONSerialization::dataWithJSONObject_options_error(obj, NSJSONWritingOptions::PrettyPrinted)
                .ok()?;
        let bytes = data.to_vec();
        if bytes.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Startup-installed OS observers. Keep it alive for the life of the daemon; dropping it
/// removes every observer it installed.
pub struct SystemEvents {
    sink: Sink,
    observers: Vec<Observer>,
    custom: HashMap<String, Observer>,
    _reconfiguration: displays::ReconfigurationObserver,
}

impl SystemEvents {
    /// Installs (on the main thread): front-app, sleep/wake, active-space and private
    /// active-display workspace notifications; distributed `com.apple.screenIsUnlocked`
    /// (wake) and `AppleInterfaceMenuBarHidingChangedNotification`; CG display
    /// reconfiguration; SkyLight space notifications and capture gating; IOPS power source;
    /// SCDynamicStore Wi-Fi.
    pub fn start(sink: Sink, _mtm: MainThreadMarker) -> SystemEvents {
        let ws_center = NSWorkspace::sharedWorkspace().notificationCenter();
        let dist: Retained<NSNotificationCenter> =
            Retained::into_super(NSDistributedNotificationCenter::defaultCenter());
        let mut observers = Vec::new();

        let s = sink.clone();
        // SAFETY: AppKit's notification name constants are valid static NSStrings.
        let (activate, sleep, wake, space, app_key) = unsafe {
            (
                NSWorkspaceDidActivateApplicationNotification,
                NSWorkspaceWillSleepNotification,
                NSWorkspaceDidWakeNotification,
                NSWorkspaceActiveSpaceDidChangeNotification,
                NSWorkspaceApplicationKey,
            )
        };
        observers.push(Observer::new(&ws_center, Some(activate), move |n| {
            let app = n
                .userInfo()
                .and_then(|info| info.objectForKey(app_key as &AnyObject))
                .and_then(|o| o.downcast::<NSRunningApplication>().ok());
            let (name, bundle_id, pid) = match app {
                Some(a) => app_identity(&a),
                None => (None, None, 0),
            };
            s(SysEvent::FrontAppSwitched {
                name,
                bundle_id,
                pid,
            });
        }));

        let s = sink.clone();
        observers.push(Observer::new(&ws_center, Some(sleep), move |_| s(SysEvent::SystemWillSleep)));
        let s = sink.clone();
        observers.push(Observer::new(&ws_center, Some(wake), move |_| {
            s(SysEvent::SystemWoke {
                screen_unlocked: false,
            })
        }));
        observers.push(Observer::new(&ws_center, Some(space), move |_| {
            spaces::notify_space_changed();
        }));
        let s = sink.clone();
        let active_display = NSString::from_str("NSWorkspaceActiveDisplayDidChangeNotification");
        observers.push(Observer::new(&ws_center, Some(&active_display), move |_| {
            s(SysEvent::DisplayChange {
                adid: displays::active_display_adid(),
            })
        }));

        let s = sink.clone();
        let unlocked = NSString::from_str("com.apple.screenIsUnlocked");
        observers.push(Observer::new(&dist, Some(&unlocked), move |_| {
            s(SysEvent::SystemWoke {
                screen_unlocked: true,
            })
        }));
        let s = sink.clone();
        let hiding = NSString::from_str("AppleInterfaceMenuBarHidingChangedNotification");
        observers.push(Observer::new(&dist, Some(&hiding), move |_| {
            s(SysEvent::MenuBarHidingChanged)
        }));

        let reconfiguration = displays::observe_reconfiguration(sink.clone());
        spaces::start_space_events(sink.clone());
        power::start(Some(sink.clone()));
        wifi::start(sink.clone());

        SystemEvents {
            sink,
            observers,
            custom: HashMap::new(),
            _reconfiguration: reconfiguration,
        }
    }

    /// Observes a distributed notification for `--add event <name> <notification>`.
    /// Idempotent per notification name (D19: each notification is delivered once).
    pub fn observe(&mut self, notification: &str) {
        if self.custom.contains_key(notification) {
            return;
        }
        let dist: Retained<NSNotificationCenter> =
            Retained::into_super(NSDistributedNotificationCenter::defaultCenter());
        let s = self.sink.clone();
        let name = NSString::from_str(notification);
        let obs = Observer::new(&dist, Some(&name), move |n| {
            s(SysEvent::DistributedNotification {
                name: n.name().to_string(),
                user_info_json: user_info_json(n),
            })
        });
        self.custom.insert(notification.to_string(), obs);
    }

    /// Stops observing a custom distributed notification.
    pub fn unobserve(&mut self, notification: &str) {
        self.custom.remove(notification);
    }

    /// Names currently observed via [`Self::observe`].
    pub fn observed(&self) -> impl Iterator<Item = &str> {
        self.custom.keys().map(|s| s.as_str())
    }

    /// First `volume_change` subscription (`begin_receiving_volume_events`).
    pub fn start_volume_events(&self) {
        volume::start(self.sink.clone());
    }

    /// First `brightness_change` subscription.
    pub fn start_brightness_events(&self) {
        brightness::start(self.sink.clone());
    }

    /// First `media_change` subscription or `image=media.artwork`.
    pub fn start_media_events(&self) {
        media::start(self.sink.clone());
    }

    /// First `space_windows_change` subscription.
    pub fn start_space_window_events(&self) {
        spaces::start_space_window_events(self.sink.clone());
    }

    /// Number of installed startup observers (diagnostics).
    pub fn observer_count(&self) -> usize {
        self.observers.len() + self.custom.len()
    }
}
