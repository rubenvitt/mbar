//! Config-directory watcher for `--hotload` (`docs/spec/events.md` §9.4): an FSEvents
//! stream on the config directory (recursive, file events, no-defer, 0.5 s latency, since
//! now), delivered on the main queue. While enabled, a change posts
//! [`SysEvent::ConfigChanged`] at most once per 2^30 ns (≈1.07 s).

use super::util::cfstr;
use super::{Sink, SysEvent};
use objc2_core_foundation::{CFArray, CFIndex, CFRetained, CFString};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[repr(C)]
struct FSEventStreamContext {
    version: CFIndex,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type FSEventStreamCallback =
    unsafe extern "C" fn(*const c_void, *mut c_void, usize, *mut c_void, *const u32, *const u64);

const SINCE_NOW: u64 = 0xFFFF_FFFF_FFFF_FFFF;
const FLAG_NO_DEFER: u32 = 0x02;
const FLAG_FILE_EVENTS: u32 = 0x10;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: FSEventStreamCallback,
        context: *mut FSEventStreamContext,
        paths: *const CFArray,
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> *mut c_void;
    fn FSEventStreamSetDispatchQueue(stream: *mut c_void, queue: *const c_void);
    fn FSEventStreamStart(stream: *mut c_void) -> u8;
    fn FSEventStreamStop(stream: *mut c_void);
    fn FSEventStreamInvalidate(stream: *mut c_void);
    fn FSEventStreamRelease(stream: *mut c_void);
}

/// Minimum spacing between two hotloads (2^30 ns).
pub const MIN_INTERVAL: Duration = Duration::from_nanos(1 << 30);

/// Pure rate limit: may a hotload fire at `now` given the last one?
pub fn should_fire(last: Option<Instant>, now: Instant) -> bool {
    match last {
        Some(t) => now.duration_since(t) > MIN_INTERVAL,
        None => true,
    }
}

struct Ctx {
    sink: Sink,
    enabled: AtomicBool,
    last: Mutex<Option<Instant>>,
}

unsafe extern "C" fn callback(
    _stream: *const c_void,
    info: *mut c_void,
    count: usize,
    _paths: *mut c_void,
    _flags: *const u32,
    _ids: *const u64,
) {
    if info.is_null() || count == 0 {
        return;
    }
    // SAFETY: `info` is the `Box<Ctx>` owned by the watcher, alive until the stream is
    // invalidated in Drop.
    let ctx = unsafe { &*(info as *const Ctx) };
    if !ctx.enabled.load(Ordering::SeqCst) {
        return;
    }
    let now = Instant::now();
    {
        let mut last = ctx.last.lock().unwrap_or_else(|e| e.into_inner());
        if !should_fire(*last, now) {
            return;
        }
        *last = Some(now);
    }
    (ctx.sink)(SysEvent::ConfigChanged);
}

/// A running FSEvents stream. Dropping it stops the stream.
pub struct HotloadWatcher {
    stream: *mut c_void,
    ctx: *mut Ctx,
}

impl HotloadWatcher {
    /// Watches `dir` recursively (main queue). `enabled` is the initial `--hotload` state
    /// (SketchyBar default: off).
    pub fn start(dir: &str, enabled: bool, sink: Sink) -> Option<HotloadWatcher> {
        let ctx = Box::into_raw(Box::new(Ctx {
            sink,
            enabled: AtomicBool::new(enabled),
            last: Mutex::new(None),
        }));
        let mut context = FSEventStreamContext {
            version: 0,
            info: ctx.cast(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        let path: CFRetained<CFString> = cfstr(dir);
        let paths = CFArray::from_retained_objects(&[path]);
        // SAFETY: valid callback/context/paths; the context is copied by FSEvents and `ctx`
        // outlives the stream.
        let stream = unsafe {
            FSEventStreamCreate(
                std::ptr::null(),
                callback,
                &mut context,
                paths.as_opaque(),
                SINCE_NOW,
                0.5,
                FLAG_NO_DEFER | FLAG_FILE_EVENTS,
            )
        };
        if stream.is_null() {
            // SAFETY: reclaim the context on failure.
            unsafe { drop(Box::from_raw(ctx)) };
            return None;
        }
        let main =
            dispatch2::DispatchQueue::main() as *const dispatch2::DispatchQueue as *const c_void;
        // SAFETY: freshly created stream; the main queue lives forever.
        unsafe {
            FSEventStreamSetDispatchQueue(stream, main);
            FSEventStreamStart(stream);
        }
        Some(HotloadWatcher { stream, ctx })
    }

    /// `--hotload on|off`.
    pub fn set_enabled(&self, on: bool) {
        // SAFETY: `ctx` is alive while `self` exists.
        unsafe { &*self.ctx }.enabled.store(on, Ordering::SeqCst);
    }

    pub fn enabled(&self) -> bool {
        // SAFETY: `ctx` is alive while `self` exists.
        unsafe { &*self.ctx }.enabled.load(Ordering::SeqCst)
    }
}

impl Drop for HotloadWatcher {
    fn drop(&mut self) {
        // SAFETY: stop → invalidate → release is the documented teardown; afterwards no
        // callback can run, so the context can be freed.
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
            drop(Box::from_raw(self.ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit() {
        let t = Instant::now();
        assert!(should_fire(None, t));
        assert!(!should_fire(Some(t), t + Duration::from_millis(500)));
        assert!(should_fire(Some(t), t + Duration::from_millis(1100)));
    }
}
