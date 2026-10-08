//! macOS platform: `NSApplication` (accessory policy) run loop on the main thread around
//! the [`Driver`], with `mbar_macos::platform` providing resources, windows and system
//! services.
//!
//! Threads → main: IPC readers, the mach server (main run loop source), script reapers,
//! providers, alias captures and every OS source push into one queue and wake the main
//! thread through a coalescing GCD main-queue [`Waker`]. The main thread then
//!
//! 1. drains the queue (system events are translated by `Services::translate`, mach
//!    requests become `Event::Request` with a `Responder::Callback` → `MachReply::send`,
//!    view mouse events by `Services::view_mouse`) into `Driver::handle_event`,
//! 2. calls `Driver::poll` and hands the `FrameOutput` (dirty windows only) to the
//!    `WindowManager`,
//! 3. executes `Driver::take_platform_requests`,
//! 4. re-arms the single deadline timer from `Driver::next_deadline`, or — when a frame
//!    is due right away (animations) — runs the display-synced `FramePacer` instead.
//!
//! Nothing runs while idle: no deadline → the timer is disarmed and the display link
//! paused. Scripts and IPC never block the main thread (spawned/reaped and read on
//! background threads by the driver and the IPC listener).

use mbar_core::platform::{Input, WindowKey};
use mbar_macos::gfx::window::MouseEvent as ViewMouse;
use mbar_macos::platform::{
    App, DeadlineTimer, FramePacer, MacResources, MainThreadMarker, Services, Translated,
    ViewMouseSink, Waker, WindowManager,
};
use mbar_macos::sys::{Sink, SysEvent};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use super::Platform;
use crate::daemon::DaemonSetup;
use crate::driver::{Driver, Event, Post};
use crate::ipc::Responder;

/// One unit of work for the main thread.
enum Msg {
    Driver(Event),
    Sys(SysEvent),
    View(WindowKey, ViewMouse),
}

/// `SysEvent`s are created on arbitrary threads by the system layer (its `Sink` is
/// `Send + Sync`); some payloads (`CGImage`) are thread-safe CF objects that are not
/// marked `Send` by their bindings.
struct SendMsg(Msg);
// SAFETY: every payload is either plain data, a thread-safe CF object (CGImage), a mach
// port right (MachReply) or a Unix stream / boxed `Send` closure (driver events); the
// receiving side only touches them on the main thread.
unsafe impl Send for SendMsg {}

struct Shared {
    queue: Mutex<VecDeque<SendMsg>>,
    waker: Arc<Waker>,
}

impl Shared {
    fn push(&self, m: Msg) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(SendMsg(m));
        self.waker.wake();
    }

    fn take_into(&self, out: &mut VecDeque<SendMsg>) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::swap(&mut *q, out);
    }

    fn is_empty(&self) -> bool {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

thread_local! {
    static STATE: RefCell<Option<MacState>> = const { RefCell::new(None) };
}

/// Entry point of every main-thread callback (waker, timer, display link).
fn pump_global() {
    let reentrant = STATE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => {
            if let Some(st) = guard.as_mut() {
                st.pump();
            }
            false
        }
        Err(_) => true,
    });
    if reentrant {
        // A nested run loop ran us from inside a pump: try again afterwards.
        if let Some(s) = SHARED.get() {
            s.waker.wake();
        }
    }
}

struct MacState {
    app: App,
    shared: Arc<Shared>,
    driver: Driver,
    res: MacResources,
    wm: WindowManager,
    services: Services,
    hotload: Arc<AtomicBool>,
    timer: Option<DeadlineTimer>,
    pacer: FramePacer,
    batch: VecDeque<SendMsg>,
    last_frame: Instant,
    displays_changed: bool,
    finished: bool,
}

impl MacState {
    fn pump(&mut self) {
        if self.finished {
            return;
        }
        loop {
            self.shared.take_into(&mut self.batch);
            while let Some(SendMsg(msg)) = self.batch.pop_front() {
                if self.driver.exit_requested() {
                    break;
                }
                self.dispatch(msg);
            }
            self.batch.clear();
            if let Some(frame) = self.driver.poll(&mut self.res) {
                self.wm.apply(frame, &mut self.res);
                self.last_frame = Instant::now();
            }
            if self.res.text.needs_prune() {
                // Exact pruning: keys the core or a kept scene still draws survive.
                let (rt, wm) = (self.driver.runtime(), &self.wm);
                self.res.text.prune_live(|f| {
                    rt.for_each_text_key(f);
                    wm.for_each_text_key(f);
                });
            }
            for req in self.driver.take_platform_requests() {
                self.services.execute(req, &mut self.res);
            }
            if self.driver.exit_requested() || self.shared.is_empty() {
                break;
            }
        }
        if std::mem::take(&mut self.displays_changed) {
            self.pacer.rebuild();
        }
        self.services
            .sync_hotload(self.hotload.load(Ordering::Relaxed));
        if self.driver.exit_requested() {
            self.finish();
            return;
        }
        self.schedule();
    }

    fn dispatch(&mut self, msg: Msg) {
        match msg {
            Msg::Driver(ev) => self.driver.handle_event(ev, &mut self.res),
            Msg::Sys(ev) => {
                if matches!(ev, SysEvent::DisplaysReconfigured { .. }) {
                    self.displays_changed = true;
                }
                match self.services.translate(ev, &mut self.res, &mut self.wm) {
                    Translated::Input(input) => self.input(input),
                    Translated::Request { args, reply } => {
                        let responder = Responder::Callback(Box::new(move |text: String| {
                            reply.send(&text);
                        }));
                        self.driver
                            .handle_event(Event::Request { args, responder }, &mut self.res);
                    }
                    Translated::Nothing => {}
                }
            }
            Msg::View(key, ev) => {
                if let Some(input) = self.services.view_mouse(key, &ev, &self.wm) {
                    self.input(input);
                }
            }
        }
    }

    fn input(&mut self, input: Input) {
        self.driver.handle_event(Event::Input(input), &mut self.res);
    }

    /// Timer to the next deadline, or display-synced frames while one is due now.
    fn schedule(&mut self) {
        let now = Instant::now();
        let deadline = self.driver.next_deadline();
        let Some(timer) = self.timer.as_mut() else {
            return;
        };
        match deadline {
            Some(d) if d <= now => {
                if self.pacer.run(true) {
                    timer.arm(None);
                } else {
                    timer.arm(Some(self.pacer.next_frame(self.last_frame, now)));
                }
            }
            other => {
                self.pacer.run(false);
                timer.arm(other);
            }
        }
    }

    fn finish(&mut self) {
        self.finished = true;
        self.pacer.run(false);
        if let Some(t) = self.timer.as_mut() {
            t.arm(None);
        }
        self.wm.close_all();
        self.services.shutdown();
        self.app.stop();
    }
}

pub struct MacPlatform;

impl Platform for MacPlatform {
    fn run(self, mut setup: DaemonSetup) -> i32 {
        let Some(mtm) = MainThreadMarker::new() else {
            log::error!("the macOS platform must run on the main thread; running headless");
            return super::headless::HeadlessPlatform.run(setup);
        };
        let res = match MacResources::new(mtm) {
            Ok(r) => r,
            Err(e) => {
                log::error!("cannot initialize Metal ({e}); running headless");
                return super::headless::HeadlessPlatform.run(setup);
            }
        };
        let app = App::accessory(mtm);
        // Inside mbar.app: opening the app while the bar runs shows the management UI.
        if let Some(root) = std::env::current_exe()
            .ok()
            .and_then(|e| std::fs::canonicalize(e).ok())
            .and_then(|e| mbar_app::bundle::bundle_root_from_exe(&e))
        {
            mbar_macos::sys::apps::forward_reopen_to_ui(mtm, mbar_app::BUNDLE_ID, root);
        }

        let shared = SHARED
            .get_or_init(|| {
                Arc::new(Shared {
                    queue: Mutex::new(VecDeque::new()),
                    waker: Waker::new(pump_global),
                })
            })
            .clone();
        let post: Post = {
            let s = shared.clone();
            Arc::new(move |e| s.push(Msg::Driver(e)))
        };
        // SIGTERM/SIGINT/SIGHUP → `Event::Terminate` on the main queue: the same exit path
        // as `--exit` (`finish`: windows closed, menu-bar auto-hide restored).
        if let Some(signals) = setup.signals.take() {
            let post = post.clone();
            if let Err(e) = signals.forward(move |signal| post(Event::Terminate { signal })) {
                log::warn!("cannot start signal thread: {e}");
            }
        }
        let sink: Sink = {
            let s = shared.clone();
            Arc::new(move |e| s.push(Msg::Sys(e)))
        };
        let view_sink: ViewMouseSink = {
            let s = shared.clone();
            Rc::new(move |key, ev| s.push(Msg::View(key, ev)))
        };

        if let Err(e) = setup.listener.spawn(post.clone()) {
            log::error!("ipc: cannot start server: {e}");
            eprintln!(
                "{}: could not initialize daemon! abort..",
                setup.driver.bar_name
            );
            return 1;
        }

        let watch_dir = setup
            .watch_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let services = Services::start(sink, mtm, &setup.driver.bar_name, watch_dir.as_deref());
        if let Some(name) = services.mach_name() {
            log::info!("mach server registered as {name}");
        }
        let hotload = Arc::new(AtomicBool::new(false));
        let mut driver = Driver::new(setup.driver, post, hotload.clone());
        let mut res = res;
        driver.start(&mut res);

        let timer = DeadlineTimer::new(pump_global, mtm);
        if timer.is_none() {
            log::error!("cannot create the run-loop timer; timers will only fire on events");
        }
        let state = MacState {
            app,
            shared: shared.clone(),
            driver,
            res,
            wm: WindowManager::new(mtm, view_sink),
            services,
            hotload,
            timer,
            pacer: FramePacer::new(mtm, pump_global),
            batch: VecDeque::new(),
            last_frame: Instant::now(),
            displays_changed: false,
            finished: false,
        };
        STATE.with(|s| *s.borrow_mut() = Some(state));
        shared.waker.wake();

        // A second handle to the shared application (the state owns one for `stop`); the
        // state cell must not be borrowed while the loop runs callbacks.
        App::accessory(mtm).run();

        // Tear down on the main thread (windows, timers, observers, display link).
        let state = STATE.with(|s| s.borrow_mut().take());
        drop(state);
        drop(setup.lock);
        0
    }
}
