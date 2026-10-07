//! Main run loop plumbing: the accessory [`App`], a coalescing cross-thread [`Waker`]
//! (GCD main queue), a single re-armable [`DeadlineTimer`] (CFRunLoopTimer) and the
//! display-synced [`FramePacer`] (CADisplayLink on macOS 14+, else a refresh-rate timer).
//!
//! All callbacks are plain `fn()`s invoked on the main thread, in the run loop's common
//! modes (so they keep firing during event tracking, e.g. while a menu is open).

use crate::sys::util::os_at_least;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType,
    NSScreen,
};
use objc2_core_foundation::{
    kCFRunLoopCommonModes, CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer,
};
use objc2_foundation::{NSObject, NSPoint, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The shared `NSApplication` with the accessory activation policy (no Dock icon, no menu
/// bar of its own, never activates on click).
pub struct App {
    app: Retained<NSApplication>,
    mtm: MainThreadMarker,
}

impl App {
    pub fn accessory(mtm: MainThreadMarker) -> App {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        App { app, mtm }
    }

    /// Runs the AppKit event loop until [`App::stop`].
    pub fn run(&self) {
        self.app.run();
    }

    /// Leaves [`App::run`] (posts a dummy event so the loop notices immediately).
    pub fn stop(&self) {
        self.app.stop(None);
        let ev = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        );
        if let Some(ev) = ev {
            self.app.postEvent_atStart(&ev, true);
        }
    }

    pub fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }
}

/// Wakes the main thread from any thread: at most one callback is queued on the GCD main
/// queue at a time (posts while one is pending coalesce into it).
pub struct Waker {
    scheduled: AtomicBool,
    callback: fn(),
}

impl Waker {
    pub fn new(callback: fn()) -> Arc<Waker> {
        Arc::new(Waker {
            scheduled: AtomicBool::new(false),
            callback,
        })
    }

    /// Schedules the callback on the main queue unless already scheduled.
    pub fn wake(self: &Arc<Self>) {
        if self.scheduled.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        dispatch2::DispatchQueue::main().exec_async(move || {
            // Clear first: posts made while the callback runs schedule another round.
            me.scheduled.store(false, Ordering::Release);
            (me.callback)();
        });
    }
}

/// "Never" for a CFRunLoopTimer (about 30 years).
const FAR_FUTURE: f64 = 1.0e9;

/// One CFRunLoopTimer on the main run loop (common modes), re-armed to the next deadline.
/// Disarmed timers sit in the far future; no work while idle.
pub struct DeadlineTimer {
    timer: CFRetained<CFRunLoopTimer>,
    armed: Option<Instant>,
}

impl DeadlineTimer {
    pub fn new(callback: fn(), _mtm: MainThreadMarker) -> Option<DeadlineTimer> {
        let block = RcBlock::new(move |_t: *mut CFRunLoopTimer| callback());
        let fire = CFAbsoluteTimeGetCurrent() + FAR_FUTURE;
        // SAFETY: default allocator; the block is copied by CF; a repeating timer with a
        // huge interval is never invalidated by firing, so it can be re-armed forever.
        let timer =
            unsafe { CFRunLoopTimer::with_handler(None, fire, FAR_FUTURE, 0, 0, Some(&block)) }?;
        let rl = CFRunLoop::main()?;
        // SAFETY: reading an immutable CF constant.
        rl.add_timer(Some(&timer), unsafe { kCFRunLoopCommonModes });
        Some(DeadlineTimer { timer, armed: None })
    }

    /// Fires at `at` (immediately if in the past); `None` disarms.
    pub fn arm(&mut self, at: Option<Instant>) {
        self.armed = at;
        let now_cf = CFAbsoluteTimeGetCurrent();
        let fire = match at {
            Some(t) => now_cf + t.saturating_duration_since(Instant::now()).as_secs_f64(),
            None => now_cf + FAR_FUTURE,
        };
        self.timer.set_next_fire_date(fire);
    }

    pub fn armed(&self) -> Option<Instant> {
        self.armed
    }
}

impl Drop for DeadlineTimer {
    fn drop(&mut self) {
        self.timer.invalidate();
    }
}

/// Instance variables of [`FrameTarget`].
pub struct FrameTargetIvars {
    callback: fn(),
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `FrameTarget` has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MbarFrameTarget"]
    #[ivars = FrameTargetIvars]
    pub struct FrameTarget;

    impl FrameTarget {
        #[unsafe(method(tick:))]
        fn tick(&self, _link: &AnyObject) {
            (self.ivars().callback)();
        }
    }
);

impl FrameTarget {
    fn new(mtm: MainThreadMarker, callback: fn()) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FrameTargetIvars { callback });
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Display-synced frame callbacks while animating. With a display link (macOS 14+,
/// `NSScreen.displayLinkWithTarget:selector:`) [`FramePacer::run`] returns `true` and the
/// callback fires every vsync until paused; otherwise the caller arms its timer at
/// [`FramePacer::next_frame`].
pub struct FramePacer {
    mtm: MainThreadMarker,
    callback: fn(),
    link: Option<Retained<CADisplayLink>>,
    running: bool,
    interval: Duration,
}

impl FramePacer {
    pub fn new(mtm: MainThreadMarker, callback: fn()) -> FramePacer {
        let mut p = FramePacer {
            mtm,
            callback,
            link: None,
            running: false,
            interval: Duration::from_secs_f64(1.0 / 60.0),
        };
        p.rebuild();
        p
    }

    /// (Re)creates the display link for the current main screen (after display changes).
    pub fn rebuild(&mut self) {
        if let Some(l) = self.link.take() {
            l.invalidate();
        }
        let Some(screen) = NSScreen::mainScreen(self.mtm) else {
            return;
        };
        if screen.respondsToSelector(sel!(maximumFramesPerSecond)) {
            let fps = screen.maximumFramesPerSecond();
            if fps > 0 {
                self.interval = Duration::from_secs_f64(1.0 / fps as f64);
            }
        }
        if !os_at_least(14, 0) || !screen.respondsToSelector(sel!(displayLinkWithTarget:selector:))
        {
            return;
        }
        let target = FrameTarget::new(self.mtm, self.callback);
        let obj: &AnyObject = &target;
        // SAFETY: `target` implements `tick:` taking the display link; the link retains
        // its target until invalidated.
        let link = unsafe { screen.displayLinkWithTarget_selector(obj, sel!(tick:)) };
        link.setPaused(!self.running);
        // SAFETY: the main run loop and the common-modes constant are valid.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        self.link = Some(link);
    }

    /// Starts (`true`) or pauses vsync callbacks. Returns whether a display link drives
    /// the frames (else the caller must use [`FramePacer::next_frame`]).
    pub fn run(&mut self, on: bool) -> bool {
        if self.running != on {
            self.running = on;
            if let Some(l) = &self.link {
                l.setPaused(!on);
            }
        }
        self.link.is_some()
    }

    /// Timer fallback: the next frame after `last` at the display refresh rate.
    pub fn next_frame(&self, last: Instant, now: Instant) -> Instant {
        (last + self.interval).max(now)
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }
}

impl Drop for FramePacer {
    fn drop(&mut self) {
        if let Some(l) = self.link.take() {
            l.invalidate();
        }
    }
}
