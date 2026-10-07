//! Headless platform: a plain thread main loop over an mpsc channel.
//!
//! Background threads (IPC readers, script reapers, hotload watcher, the signal forwarder)
//! post [`Event`]s; the loop sleeps in `recv_timeout` until the next event or
//! `Driver::next_deadline_paced`. There is no display link, so animation frames are paced
//! at 60 Hz (`animation::FRAME_INTERVAL`, D12) instead of being due "now" (which would
//! busy-spin a core while any animation runs). Frames are computed (so layout runs and
//! `--query` sees geometry) but not presented; platform requests are ignored.
//!
//! Like the macOS pump, every iteration drains the queued events (up to [`MAX_BATCH`])
//! before polling, so a burst of N requests or script exits costs one layout + scene
//! frame, not N.

use mbar_core::animation::FRAME_INTERVAL;
use mbar_core::platform::HeadlessResources;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::Instant;

use super::Platform;
use crate::daemon::DaemonSetup;
use crate::driver::{Driver, Event, Post};

pub struct HeadlessPlatform;

/// Events handled per loop iteration before the frame is rendered. Bounds the time a
/// flood of events can hold back a frame (and the platform requests it produces).
const MAX_BATCH: usize = 1024;

/// What one [`step`] saw on the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Continue,
    /// Every sender is gone: nothing can wake the loop any more.
    Disconnected,
}

impl Platform for HeadlessPlatform {
    fn run(self, mut setup: DaemonSetup) -> i32 {
        let (tx, rx) = mpsc::channel::<Event>();
        let post: Post = Arc::new(move |e| {
            let _ = tx.send(e);
        });
        if let Some(signals) = setup.signals.take() {
            let post = post.clone();
            if let Err(e) = signals.forward(move |signal| post(Event::Terminate { signal })) {
                log::warn!("cannot start signal thread: {e}");
            }
        }

        if let Err(e) = setup.listener.spawn(post.clone()) {
            log::error!("ipc: cannot start server: {e}");
            eprintln!(
                "{}: could not initialize daemon! abort..",
                setup.driver.bar_name
            );
            return 1;
        }
        let mut hotload = crate::hotload::spawn(setup.watch_dir.clone(), post.clone());

        let mut res = HeadlessResources {
            now: Instant::now(),
            ..HeadlessResources::default()
        };
        let mut driver = Driver::new(setup.driver, post, hotload.flag());
        driver.start(&mut res);
        hotload.sync();

        while !driver.exit_requested() {
            if step(&mut driver, &rx, &mut res, &mut hotload) == Step::Disconnected {
                break;
            }
        }
        drop(setup.lock);
        0
    }
}

/// One main-loop iteration: sleeps until the first event or the (paced) deadline, handles
/// that event and everything already queued behind it, then renders at most one frame.
fn step(
    driver: &mut Driver,
    rx: &Receiver<Event>,
    res: &mut HeadlessResources,
    hotload: &mut crate::hotload::Hotload,
) -> Step {
    let first = match driver.next_deadline_paced(FRAME_INTERVAL) {
        Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
    };
    let mut status = Step::Continue;
    match first {
        Ok(ev) => {
            res.now = Instant::now();
            driver.handle_event(ev, res);
            let mut handled = 1;
            while handled < MAX_BATCH && !driver.exit_requested() {
                match rx.try_recv() {
                    Ok(ev) => {
                        res.now = Instant::now();
                        driver.handle_event(ev, res);
                        handled += 1;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        status = Step::Disconnected;
                        break;
                    }
                }
            }
        }
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => return Step::Disconnected,
    }
    // `--hotload on` may have been executed: wake the (parked) watcher.
    hotload.sync();
    res.now = Instant::now();
    if let Some(frame) = driver.poll(res) {
        log::trace!(
            "frame: {} window(s), {} closed",
            frame.windows.len(),
            frame.closed.len()
        );
    }
    for req in driver.take_platform_requests() {
        log::debug!("headless: ignoring {req:?}");
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::DriverConfig;
    use crate::ipc::Responder;
    use std::sync::Mutex;

    struct Loop {
        driver: Driver,
        rx: Receiver<Event>,
        post: Post,
        res: HeadlessResources,
        hotload: crate::hotload::Hotload,
    }

    impl Loop {
        fn new() -> Loop {
            let (tx, rx) = mpsc::channel::<Event>();
            let tx = Mutex::new(tx);
            let post: Post = Arc::new(move |e| {
                let _ = tx.lock().unwrap().send(e);
            });
            let hotload = crate::hotload::spawn(None, post.clone());
            let cfg = DriverConfig {
                bar_name: "mbar".into(),
                home: "/nonexistent".into(),
                config_path: None,
                base_env: Vec::new(),
            };
            let mut driver = Driver::new(cfg, post.clone(), hotload.flag());
            let mut res = HeadlessResources {
                now: Instant::now(),
                ..HeadlessResources::default()
            };
            driver.start(&mut res);
            Loop {
                driver,
                rx,
                post,
                res,
                hotload,
            }
        }

        /// Queues a request; its reply lands in the returned slot.
        fn queue(&self, args: &[&str]) -> Arc<Mutex<Option<String>>> {
            let out = Arc::new(Mutex::new(None));
            let o2 = out.clone();
            (self.post)(Event::Request {
                args: args.iter().map(|s| s.to_string()).collect(),
                responder: Responder::Callback(Box::new(move |t| *o2.lock().unwrap() = Some(t))),
            });
            out
        }

        fn step(&mut self) {
            let s = step(&mut self.driver, &self.rx, &mut self.res, &mut self.hotload);
            assert_eq!(s, Step::Continue);
        }

        fn request(&mut self, args: &[&str]) -> String {
            let out = self.queue(args);
            self.step();
            let r = out.lock().unwrap().take();
            r.expect("no reply after one step")
        }

        fn frames(&mut self) -> u64 {
            let v: serde_json::Value =
                serde_json::from_str(&self.request(&["--query", "stats"])).unwrap();
            v["frames"].as_u64().unwrap()
        }
    }

    /// Review regression PERF-5: a burst of queued requests is handled in one iteration
    /// and costs one frame (the loop used to handle one event per layout + scene frame).
    #[test]
    fn queued_burst_is_one_frame() {
        let mut l = Loop::new();
        l.request(&["--add", "item", "a", "left"]);
        let before = l.frames();
        let replies: Vec<_> = (0..50)
            .map(|i| l.queue(&["--set", "a", &format!("label=v{i}")]))
            .collect();
        l.step();
        for (i, r) in replies.iter().enumerate() {
            assert!(r.lock().unwrap().is_some(), "request {i} not handled");
        }
        assert_eq!(l.frames(), before + 1, "a burst must render one frame");
        let v: serde_json::Value = serde_json::from_str(&l.request(&["--query", "a"])).unwrap();
        assert_eq!(v["label"]["value"], "v49");
    }

    /// The drain stops at `--exit`: nothing after it runs.
    #[test]
    fn drain_stops_at_exit() {
        let mut l = Loop::new();
        l.request(&["--add", "item", "a", "left"]);
        l.queue(&["--exit"]);
        let after = l.queue(&["--set", "a", "label=late"]);
        l.step();
        assert!(l.driver.exit_requested());
        assert!(after.lock().unwrap().is_none(), "request after --exit ran");
    }
}
