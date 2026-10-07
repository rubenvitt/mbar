//! Headless platform: a plain thread main loop over an mpsc channel.
//!
//! Background threads (IPC readers, script reapers, hotload watcher) post
//! [`Event`]s; the loop sleeps in `recv_timeout` until the next event or
//! `Driver::next_deadline`. Frames are computed (so layout runs and `--query` sees
//! geometry) but not presented; platform requests are ignored.

use mbar_core::platform::HeadlessResources;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Instant;

use super::Platform;
use crate::daemon::DaemonSetup;
use crate::driver::{Driver, Event, Post};

pub struct HeadlessPlatform;

impl Platform for HeadlessPlatform {
    fn run(self, setup: DaemonSetup) -> i32 {
        let (tx, rx) = mpsc::channel::<Event>();
        let post: Post = Arc::new(move |e| {
            let _ = tx.send(e);
        });

        if let Err(e) = setup.listener.spawn(post.clone()) {
            log::error!("ipc: cannot start server: {e}");
            eprint!(
                "{}: could not initialize daemon! abort..\n",
                setup.driver.bar_name
            );
            return 1;
        }
        let hotload = crate::hotload::spawn(setup.watch_dir.clone(), post.clone());

        let mut res = HeadlessResources::default();
        res.now = Instant::now();
        let mut driver = Driver::new(setup.driver, post, hotload);
        driver.start(&mut res);

        while !driver.exit_requested() {
            let ev = match driver.next_deadline() {
                Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            res.now = Instant::now();
            match ev {
                Ok(ev) => driver.handle_event(ev, &mut res),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            res.now = Instant::now();
            if let Some(frame) = driver.poll(&mut res) {
                log::trace!(
                    "frame: {} window(s), {} closed",
                    frame.windows.len(),
                    frame.closed.len()
                );
            }
            for req in driver.take_platform_requests() {
                log::debug!("headless: ignoring {req:?}");
            }
        }
        drop(setup.lock);
        0
    }
}
