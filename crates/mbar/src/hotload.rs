//! Config directory watcher (`cli.md` §11.1) without OS notification APIs: while hotload
//! is enabled, the directory tree is stat'ed once per second and any change (file
//! created, modified, removed, renamed) posts `OsEvent::ConfigChanged`. Changes made while
//! hotload is off are not reported. The rate limit (one reload per 2^30 ns) is applied
//! here; the runtime decides whether to reload.
//!
//! While hotload is off (the default) the watcher thread is parked and costs nothing; the
//! main loop wakes it through [`Hotload::sync`] after the flag was switched on.
//!
//! Like SketchyBar, the watched directory is fixed at startup (never re-pointed after
//! `--reload <other path>`).

use mbar_core::platform::{Input, OsEvent};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::Thread;
use std::time::{Duration, Instant, SystemTime};

use crate::driver::{Event, Post};

/// Poll period.
const PERIOD: Duration = Duration::from_secs(1);
/// SketchyBar's rate limit: 2^30 ns.
const RATE_LIMIT: Duration = Duration::from_nanos(1 << 30);
/// Bounds for pathological directories.
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 10_000;

/// Handle of the watcher: the enable flag (`--hotload`, written by the driver) and the
/// thread to wake when it is switched on.
pub struct Hotload {
    flag: Arc<AtomicBool>,
    thread: Option<Thread>,
    /// Flag value at the last [`Hotload::sync`].
    seen: bool,
    /// Directory scans done so far (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    scans: Arc<AtomicUsize>,
}

impl Hotload {
    /// The enable flag, shared with the driver.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }

    /// Call after the flag may have changed: unparks the watcher when hotload was just
    /// switched on. (Switching it off needs no wake-up; the watcher parks itself.)
    pub fn sync(&mut self) {
        let on = self.flag.load(Ordering::Relaxed);
        if on && !self.seen {
            if let Some(t) = &self.thread {
                t.unpark();
            }
        }
        self.seen = on;
    }
}

/// Starts the watcher thread for `dir` (parked until hotload is enabled).
pub fn spawn(dir: Option<PathBuf>, post: Post) -> Hotload {
    spawn_with_period(dir, post, PERIOD)
}

fn spawn_with_period(dir: Option<PathBuf>, post: Post, period: Duration) -> Hotload {
    let mut handle = Hotload {
        flag: Arc::new(AtomicBool::new(false)),
        thread: None,
        seen: false,
        scans: Arc::new(AtomicUsize::new(0)),
    };
    let Some(dir) = dir else {
        return handle;
    };
    let flag = handle.flag.clone();
    let scans = handle.scans.clone();
    let spawned = std::thread::Builder::new()
        .name("mbar-hotload".into())
        .spawn(move || {
            let mut baseline: Option<Vec<Entry>> = None;
            let mut last_post: Option<Instant> = None;
            loop {
                if !flag.load(Ordering::Relaxed) {
                    // Changes made while hotload is off are not reported.
                    baseline = None;
                    // Spurious wake-ups are fine: the flag is checked again.
                    std::thread::park();
                    continue;
                }
                let now = snapshot(&dir);
                scans.fetch_add(1, Ordering::Relaxed);
                match &baseline {
                    None => baseline = Some(now),
                    Some(old) if *old != now => {
                        baseline = Some(now);
                        if last_post.is_none_or_elapsed(RATE_LIMIT) {
                            last_post = Some(Instant::now());
                            post(Event::Input(Input::Event(OsEvent::ConfigChanged)));
                        }
                    }
                    Some(_) => {}
                }
                std::thread::sleep(period);
            }
        });
    match spawned {
        Ok(h) => handle.thread = Some(h.thread().clone()),
        Err(e) => log::warn!("hotload: cannot start watcher: {e}"),
    }
    handle
}

trait Elapsed {
    fn is_none_or_elapsed(&self, d: Duration) -> bool;
}

impl Elapsed for Option<Instant> {
    fn is_none_or_elapsed(&self, d: Duration) -> bool {
        self.map_or(true, |t| t.elapsed() >= d)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

/// Sorted list of every entry under `dir` (bounded depth/size).
fn snapshot(dir: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if out.len() >= MAX_ENTRIES {
            return;
        }
        let path = e.path();
        // symlink_metadata: do not follow links out of the tree.
        let Ok(md) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        out.push(Entry {
            path: path.clone(),
            modified: md.modified().ok(),
            len: md.len(),
        });
        if md.is_dir() && depth < MAX_DEPTH {
            walk(&path, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_sees_changes() {
        let dir = std::env::temp_dir().join(format!("mbar-hotload-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/a"), "1").unwrap();
        let a = snapshot(&dir);
        assert_eq!(a, snapshot(&dir));
        std::fs::write(dir.join("sub/a"), "22").unwrap();
        let b = snapshot(&dir);
        assert_ne!(a, b);
        std::fs::write(dir.join("b"), "").unwrap();
        assert_ne!(b, snapshot(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(10), "timeout: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Review regression PERF-5: the watcher used to wake (and sleep again) every period
    /// while hotload was off; now it is parked until `sync` sees the flag switched on.
    #[test]
    fn watcher_is_parked_while_disabled() {
        let dir = std::env::temp_dir().join(format!("mbar-hotload-park-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let posted = Arc::new(AtomicUsize::new(0));
        let p2 = posted.clone();
        let post: Post = Arc::new(move |e| {
            if matches!(e, Event::Input(Input::Event(OsEvent::ConfigChanged))) {
                p2.fetch_add(1, Ordering::SeqCst);
            }
        });
        let mut h = spawn_with_period(Some(dir.clone()), post, Duration::from_millis(5));
        h.sync();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            h.scans.load(Ordering::SeqCst),
            0,
            "disabled watcher scanned"
        );

        // Enabled: the watcher wakes, takes a baseline and reports a change.
        h.flag().store(true, Ordering::SeqCst);
        h.sync();
        wait_until("baseline scan", || h.scans.load(Ordering::SeqCst) >= 2);
        std::fs::write(dir.join("rc"), "x").unwrap();
        wait_until("change reported", || posted.load(Ordering::SeqCst) == 1);

        // Disabled again: at most the scan already in flight, then parked.
        h.flag().store(false, Ordering::SeqCst);
        h.sync();
        std::thread::sleep(Duration::from_millis(30));
        let n = h.scans.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(h.scans.load(Ordering::SeqCst), n, "watcher kept scanning");
        std::fs::write(dir.join("rc"), "yy").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            posted.load(Ordering::SeqCst),
            1,
            "change reported while off"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
