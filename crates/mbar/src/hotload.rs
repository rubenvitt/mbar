//! Config directory watcher (`cli.md` §11.1) without OS notification APIs: while hotload
//! is enabled, the directory tree is stat'ed once per second and any change (file
//! created, modified, removed, renamed) posts `OsEvent::ConfigChanged`. Changes made while
//! hotload is off are not reported. The rate limit (one reload per 2^30 ns) is applied
//! here; the runtime decides whether to reload.
//!
//! Like SketchyBar, the watched directory is fixed at startup (never re-pointed after
//! `--reload <other path>`).

use mbar_core::platform::{Input, OsEvent};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::driver::{Event, Post};

/// Poll period.
const PERIOD: Duration = Duration::from_secs(1);
/// SketchyBar's rate limit: 2^30 ns.
const RATE_LIMIT: Duration = Duration::from_nanos(1 << 30);
/// Bounds for pathological directories.
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 10_000;

/// Starts the watcher thread for `dir`; returns the enable flag (`--hotload`).
pub fn spawn(dir: Option<PathBuf>, post: Post) -> Arc<AtomicBool> {
    let enabled = Arc::new(AtomicBool::new(false));
    let Some(dir) = dir else {
        return enabled;
    };
    let flag = enabled.clone();
    let spawned = std::thread::Builder::new()
        .name("mbar-hotload".into())
        .spawn(move || {
            let mut baseline: Option<Vec<Entry>> = None;
            let mut last_post: Option<Instant> = None;
            loop {
                std::thread::sleep(PERIOD);
                if !flag.load(Ordering::Relaxed) {
                    baseline = None;
                    continue;
                }
                let now = snapshot(&dir);
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
            }
        });
    if let Err(e) = spawned {
        log::warn!("hotload: cannot start watcher: {e}");
    }
    enabled
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
}
