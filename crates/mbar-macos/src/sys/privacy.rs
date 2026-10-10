//! Privacy indicator detection (`docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`,
//! §mbar-macos). One worker thread drives [`mbar_core::privacy::Tracker`]: it spawns
//! `log stream` (Control Center's attribution lines) when the tracker asks, runs one
//! `log show` after every spawn for the starting state, looks at the indicator windows
//! (`CGWindowListCopyWindowInfo`) when the tracker asks and posts
//! [`SysEvent::PrivacyIndicator`] whenever the sample changed. Nothing runs on the main
//! thread.

use super::{alias, util, Sink, SysEvent};
use mbar_core::geometry::Rect;
use mbar_core::privacy::{
    classify_stream_line, PrivacySample, StreamLine, Tracker, TrackerInput, LOG_PREDICATE,
};
use objc2_core_foundation::{CFArray, CFDictionary, CFRetained};
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Instant;

const LOG_BIN: &str = "/usr/bin/log";
/// Layer of WindowServer's `StatusIndicator` windows (measured on macOS 27.0.1:
/// owner `Window Server`, name `StatusIndicator`, 28×28 pt at the top-right corner).
pub(crate) const INDICATOR_LAYER: i64 = 2_147_483_630;
/// Larger windows on that layer are not the dot.
const MAX_SIZE: f32 = 64.0;

/// One on-screen window, as far as the filter needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowRecord {
    pub owner: String,
    pub layer: i64,
    /// Global points, top-left origin.
    pub frame: Rect,
}

/// The indicator windows among `windows`. The name (`StatusIndicator`) is not used: it
/// needs the Screen Recording permission, owner, layer and bounds do not.
pub(crate) fn indicator_frames(windows: &[WindowRecord]) -> Vec<Rect> {
    windows
        .iter()
        .filter(|w| {
            w.owner == "Window Server"
                && w.layer == INDICATOR_LAYER
                && w.frame.width > 0.0
                && w.frame.height > 0.0
                && w.frame.width <= MAX_SIZE
                && w.frame.height <= MAX_SIZE
        })
        .map(|w| w.frame)
        .collect()
}

fn list_windows() -> Vec<WindowRecord> {
    let Some(list): Option<CFRetained<CFArray>> =
        CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0)
    else {
        return Vec::new();
    };
    util::array_items(&list)
        .into_iter()
        .filter_map(util::downcast::<CFDictionary>)
        .filter_map(|d| {
            let owner = util::dict_string(&d, "kCGWindowOwnerName")?;
            let layer = util::dict_i64(&d, "kCGWindowLayer")?;
            let bounds =
                util::dict_get(&d, "kCGWindowBounds").and_then(util::downcast::<CFDictionary>)?;
            let r = alias::rect_from_bounds(&bounds)?;
            Some(WindowRecord {
                owner,
                layer,
                frame: Rect::new(
                    r.origin.x as f32,
                    r.origin.y as f32,
                    r.size.width as f32,
                    r.size.height as f32,
                ),
            })
        })
        .collect()
}

enum Msg {
    Input(TrackerInput),
    Stop,
}

struct Shared {
    tx: Sender<Msg>,
    /// The running `log stream`.
    child: Option<Child>,
    /// The running `log show` and the spawn number it belongs to.
    history: Option<(u64, Child)>,
    /// Set by [`stop`]; nothing is spawned afterwards.
    stopped: bool,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

fn with_shared<R>(f: impl FnOnce(&mut Option<Shared>) -> R) -> R {
    let mut guard = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

fn send(msg: Msg) {
    with_shared(|s| {
        if let Some(s) = s {
            let _ = s.tx.send(msg);
        }
    });
}

/// Starts the detection (idempotent; `PlatformRequest::StartPrivacyIndicator`).
pub fn start(sink: Sink) {
    let (tx, rx) = mpsc::channel();
    let first = with_shared(|s| {
        if s.is_some() {
            return false;
        }
        *s = Some(Shared {
            tx,
            child: None,
            history: None,
            stopped: false,
        });
        true
    });
    if !first {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mbar-privacy".into())
        .spawn(move || worker(sink, rx));
    if let Err(e) = spawned {
        log::warn!("privacy indicator: worker thread not started: {e}");
        // Let a later `start()` retry.
        with_shared(|s| *s = None);
    }
}

/// Display reconfiguration or wake: check the windows now.
pub fn nudge() {
    send(Msg::Input(TrackerInput::Nudge));
}

/// Stops the worker and kills `log stream` / `log show` (call on exit).
///
/// Children are spawned and stored under the lock and only while `stopped` is false
/// ([`spawn_tracked`]), so after this returns either a child was seen here (and is killed
/// and reaped) or none will ever be spawned.
pub fn stop() {
    let (a, b) = with_shared(|s| {
        let Some(s) = s else { return (None, None) };
        s.stopped = true;
        let _ = s.tx.send(Msg::Stop);
        (s.child.take(), s.history.take().map(|(_, c)| c))
    });
    for mut c in [a, b].into_iter().flatten() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

#[derive(Clone, Copy)]
enum Slot {
    Stream,
    History(u64),
}

/// Spawns `cmd` (stdout piped) and stores the child in `slot` under the lock; refuses once
/// stopped. A previous child in the slot is killed and reaped (outside the lock).
fn spawn_tracked(cmd: &mut Command, slot: Slot) -> Option<ChildStdout> {
    let (stdout, old) = with_shared(|s| {
        let s = s.as_mut().filter(|s| !s.stopped)?;
        let mut child = match cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!("privacy indicator: `log` not started: {e}");
                return None;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        };
        let old = match slot {
            Slot::Stream => s.child.replace(child),
            Slot::History(n) => s.history.replace((n, child)).map(|(_, c)| c),
        };
        Some((stdout, old))
    })?;
    if let Some(mut c) = old {
        let _ = c.kill();
        let _ = c.wait();
    }
    Some(stdout)
}

/// Kills and reaps the current `log stream` (the lock is not held while waiting).
fn kill_child() {
    let child = with_shared(|s| s.as_mut().and_then(|s| s.child.take()));
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn worker(sink: Sink, rx: mpsc::Receiver<Msg>) {
    let mut tracker = Tracker::new(Instant::now());
    let mut last: Option<PrivacySample> = None;
    loop {
        let now = Instant::now();
        if tracker.restart_at().is_some_and(|t| t <= now) {
            if spawn_stream() {
                // Handle the start first, then read the spawn number for the history.
                tracker.handle(TrackerInput::StreamStarted, now);
                spawn_history(tracker.spawns());
            } else {
                tracker.handle(TrackerInput::StreamExited, now);
            }
        }
        if tracker.next_check().is_some_and(|t| t <= now) {
            let frames = indicator_frames(&list_windows());
            tracker.handle(TrackerInput::Windows(frames), now);
        }
        if tracker.ready() {
            let sample = tracker.sample();
            if last.as_ref() != Some(&sample) {
                if sample.attributions.is_none()
                    && last.as_ref().is_some_and(|l| l.attributions.is_some())
                {
                    log::warn!("privacy indicator: app attribution unavailable (log stream down or its format changed)");
                }
                sink(SysEvent::PrivacyIndicator(sample.clone()));
                last = Some(sample);
            }
        }
        let deadline = [tracker.restart_at(), tracker.next_check()]
            .into_iter()
            .flatten()
            .min();
        let msg = match deadline {
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            },
        };
        match msg {
            Some(Msg::Stop) => return,
            Some(Msg::Input(input)) => tracker.handle(input, Instant::now()),
            None => {}
        }
    }
}

/// Spawns `log stream` and its reader thread. `false` if it could not be started.
fn spawn_stream() -> bool {
    let mut cmd = Command::new(LOG_BIN);
    cmd.args(["stream", "--style", "ndjson", "--predicate", LOG_PREDICATE]);
    let Some(stdout) = spawn_tracked(&mut cmd, Slot::Stream) else {
        return false;
    };
    let reader = std::thread::Builder::new()
        .name("mbar-privacy-log".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match classify_stream_line(&line) {
                    StreamLine::Attributions(a) => send(Msg::Input(TrackerInput::Line(a))),
                    StreamLine::Unparsed => send(Msg::Input(TrackerInput::Unparsed)),
                    StreamLine::Other => {}
                }
            }
            kill_child();
            send(Msg::Input(TrackerInput::StreamExited));
        });
    if reader.is_err() {
        kill_child();
        return false;
    }
    true
}

/// One `log show --last 1h` with the same predicate; the newest parsed line becomes
/// `History(spawn, …)` (the tracker ignores it if a live line came first or it is
/// from another spawn).
fn spawn_history(spawn: u64) {
    let _ = std::thread::Builder::new()
        .name("mbar-privacy-history".into())
        .spawn(move || {
            let mut cmd = Command::new(LOG_BIN);
            cmd.args([
                "show",
                "--last",
                "1h",
                "--style",
                "ndjson",
                "--predicate",
                LOG_PREDICATE,
            ]);
            let Some(stdout) = spawn_tracked(&mut cmd, Slot::History(spawn)) else {
                return;
            };
            let newest = BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .filter_map(|l| match classify_stream_line(&l) {
                    StreamLine::Attributions(a) => Some(a),
                    _ => None,
                })
                .last();
            // Reap our child unless a newer spawn replaced (and already reaped) it.
            let child = with_shared(|s| {
                let s = s.as_mut()?;
                match s.history.take() {
                    Some((n, c)) if n == spawn => Some(c),
                    other => {
                        s.history = other;
                        None
                    }
                }
            });
            if let Some(mut c) = child {
                let _ = c.wait();
            }
            send(Msg::Input(TrackerInput::History(spawn, newest)));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(owner: &str, layer: i64, x: f32, size: f32) -> WindowRecord {
        WindowRecord {
            owner: owner.into(),
            layer,
            frame: Rect::new(x, 3.0, size, size),
        }
    }

    #[test]
    fn filters_indicator_windows() {
        // As measured: two identical StatusIndicator windows, plus other Window Server
        // windows (backstop, underbelly) and an app window on a high layer.
        let list = [
            w("Window Server", INDICATOR_LAYER, 2025.0, 28.0),
            w("Window Server", INDICATOR_LAYER, 2025.0, 28.0),
            w("Window Server", -2147483626, 0.0, 2056.0),
            w("Window Server", INDICATOR_LAYER, 0.0, 2056.0),
            w("mbar", 3, 0.0, 28.0),
            w("Window Server", INDICATOR_LAYER, 10.0, 0.0),
        ];
        assert_eq!(
            indicator_frames(&list),
            vec![
                Rect::new(2025.0, 3.0, 28.0, 28.0),
                Rect::new(2025.0, 3.0, 28.0, 28.0)
            ]
        );
    }
}
