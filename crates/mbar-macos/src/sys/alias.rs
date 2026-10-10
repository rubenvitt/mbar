//! Menu-extra aliases (`docs/spec/components.md` §9): discovery through
//! `CGWindowListCopyWindowInfo` (status-bar layer 25), `--query default_menu_items`, window
//! lookup by `Owner[,Name]` / indexed name, capture and periodic refresh.
//!
//! Capture uses SketchyBar's private path `SLSCaptureWindowsContentsToRectWithOptions`
//! (still functional on macOS 15/26, synchronous and cheap), falling back to
//! `SLSHWCaptureWindowList`. `CGWindowListCreateImage` is obsoleted on macOS 15 and
//! ScreenCaptureKit's `SCScreenshotManager` is asynchronous and needs an
//! `SCShareableContent` enumeration per window lookup, which is too heavy for a 1 s refresh,
//! so it is not used. All paths need the **Screen Recording** permission (without it window
//! names of other apps are hidden, so the extras list is empty).

use super::skylight as sls;
use super::util::{self, owned};
use super::{spaces, Sink, SysEvent};
use objc2_core_foundation::{CFArray, CFDictionary, CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGDataProvider, CGImage, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
    CGWindowListCopyWindowInfo, CGWindowListOption,
};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// `kCGStatusWindowLevel` (SketchyBar's `MENUBAR_LAYER`).
pub const MENUBAR_LAYER: i64 = 0x19;

/// One menu-bar extra window.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuExtraWindow {
    pub owner: String,
    pub name: String,
    pub pid: i32,
    pub window_id: u32,
    /// Global, top-left, points.
    pub frame: CGRect,
}

/// `--add alias <spec>`: `Owner,Name` → (Owner, Some(Name)); `Owner` → (Owner, None);
/// `Owner,` → ("Owner,", None) (never matches; SketchyBar quirk).
pub fn parse_alias_spec(spec: &str) -> (String, Option<String>) {
    match spec.split_once(',') {
        Some((owner, name)) if !name.is_empty() => (owner.to_string(), Some(name.to_string())),
        _ => (spec.to_string(), None),
    }
}

/// SketchyBar's selection sort by x descending, including its quirk: the best index starts
/// at 0 with threshold −9999, so with only `x ≤ −9999` left, element `i` is swapped with 0.
#[allow(clippy::needless_range_loop)] // mirrors the C loop exactly
pub fn sort_menu_extras(items: &mut [MenuExtraWindow]) {
    for i in 0..items.len() {
        let mut best_index = 0;
        let mut best_x = -9999.0;
        for j in i..items.len() {
            if items[j].frame.origin.x > best_x {
                best_x = items[j].frame.origin.x;
                best_index = j;
            }
        }
        items.swap(i, best_index);
    }
}

pub(crate) fn rect_from_bounds(d: &CFDictionary) -> Option<CGRect> {
    let get = |k| {
        util::dict_get(d, k)
            .and_then(|v| util::cf_f64(&v).or_else(|| util::cf_i64(&v).map(|i| i as f64)))
    };
    Some(CGRect {
        origin: CGPoint {
            x: get("X")?,
            y: get("Y")?,
        },
        size: CGSize {
            width: get("Width")?,
            height: get("Height")?,
        },
    })
}

/// `get_menu_item_list`: every status-layer window (owner ≠ "Window Server") with all
/// required keys, sorted rightmost first.
pub fn list_menu_extras() -> Vec<MenuExtraWindow> {
    let Some(list): Option<CFRetained<CFArray>> =
        CGWindowListCopyWindowInfo(CGWindowListOption::OptionAll, 0)
    else {
        return Vec::new();
    };
    let mut items: Vec<MenuExtraWindow> = util::array_items(&list)
        .into_iter()
        .filter_map(util::downcast::<CFDictionary>)
        .filter_map(|d| {
            let name = util::dict_string(&d, "kCGWindowName")?;
            let owner = util::dict_string(&d, "kCGWindowOwnerName")?;
            let pid = util::dict_i64(&d, "kCGWindowOwnerPID")?;
            let layer = util::dict_i64(&d, "kCGWindowLayer")?;
            let bounds =
                util::dict_get(&d, "kCGWindowBounds").and_then(util::downcast::<CFDictionary>)?;
            let wid = util::dict_i64(&d, "kCGWindowNumber")?;
            if layer != MENUBAR_LAYER || owner == "Window Server" {
                return None;
            }
            Some(MenuExtraWindow {
                owner,
                name,
                pid: pid as i32,
                window_id: wid as u32,
                frame: rect_from_bounds(&bounds)?,
            })
        })
        .collect();
    sort_menu_extras(&mut items);
    items
}

/// `--query default_menu_items` body: `[\n\t"<owner>,<name>(1)", \n\t"…(2)"\n]\n`; empty
/// list → empty string.
pub fn default_menu_items(items: &[MenuExtraWindow]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let entries: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(i, it)| format!("\t\"{},{}({})\"", it.owner, it.name, i + 1))
        .collect();
    format!("[\n{}\n]\n", entries.join(", \n"))
}

/// Response of `--query default_menu_items` without Screen Recording permission.
pub const NO_PERMISSION_RESPONSE: &str =
    "[!] Query (default_menu_items): Screen Recording Permissions not given. Restart SketchyBar after granting permissions.\n";

/// `alias_find_window` (`components.md` §9.5).
pub fn find_window<'a>(
    items: &'a [MenuExtraWindow],
    owner: &str,
    name: Option<&str>,
) -> Option<&'a MenuExtraWindow> {
    items.iter().enumerate().find_map(|(i, it)| {
        if it.owner != owner {
            return None;
        }
        let ok = match name {
            None => it.name.is_empty(),
            Some(n) => n == it.name || n == format!("{}({})", it.name, i + 1),
        };
        ok.then_some(it)
    })
}

/// `CGPreflightScreenCaptureAccess()` (no prompt).
pub fn screen_capture_preflight() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// `CGRequestScreenCaptureAccess()` (prompts once per process).
pub fn request_screen_capture() -> bool {
    CGRequestScreenCaptureAccess()
}

/// Result of one window capture.
#[derive(Debug)]
pub enum Capture {
    /// Image plus logical frame (size from `SLSGetScreenRectForWindow`, width rounded).
    Image {
        image: CFRetained<CGImage>,
        frame: CGRect,
    },
    /// WindowServer suspended captures (1322/905); keep the old picture.
    Disabled,
    /// Capture failed (window gone, no permission).
    Failed,
}

fn screen_rect(wid: u32) -> Option<CGRect> {
    let f = sls::SLSGetScreenRectForWindow()?;
    let mut rect = CGRect::default();
    // SAFETY: valid connection id and out pointer.
    let err = unsafe { f(sls::cid(), wid, &mut rect) };
    if err != 0 {
        return None;
    }
    rect.size.width = (rect.size.width + 0.5).floor();
    Some(rect)
}

/// `window_capture` for a single window id.
pub fn capture_window(wid: u32) -> Capture {
    if spaces::capture_disabled() {
        return Capture::Disabled;
    }
    let mut image: Option<CFRetained<CGImage>> = None;
    if let Some(f) = sls::SLSCaptureWindowsContentsToRectWithOptions() {
        let null_rect = CGRect {
            origin: CGPoint {
                x: f64::INFINITY,
                y: f64::INFINITY,
            },
            size: CGSize::default(),
        };
        let mut out: *const CGImage = std::ptr::null();
        // SAFETY: valid connection id, pointer to one window id, CGRectNull, options
        // `1 << 8` and out pointer; the image follows the Create rule.
        unsafe {
            f(sls::cid(), &wid, true, null_rect, 1 << 8, &mut out);
            image = owned(out);
        }
    }
    if image.is_none() {
        if let Some(f) = sls::SLSHWCaptureWindowList() {
            // SAFETY: one window id; Copy rule for the returned array.
            let arr: Option<CFRetained<CFArray>> =
                unsafe { owned(f(sls::cid(), &wid, 1, (1 << 11) | (1 << 8))) };
            image = arr
                .and_then(|a| util::array_items(&a).into_iter().next())
                .and_then(util::downcast::<CGImage>);
        }
    }
    match (image, screen_rect(wid)) {
        (Some(image), Some(frame)) => Capture::Image { image, frame },
        _ => Capture::Failed,
    }
}

/// Hash of an image's pixels and size (SketchyBar skips redraws of unchanged pictures).
pub fn image_hash(image: &CGImage) -> u64 {
    let mut h = DefaultHasher::new();
    CGImage::width(Some(image)).hash(&mut h);
    CGImage::height(Some(image)).hash(&mut h);
    if let Some(data) =
        CGImage::data_provider(Some(image)).and_then(|p| CGDataProvider::data(Some(&p)))
    {
        data.to_vec().hash(&mut h);
    }
    h.finish()
}

/// One alias capture request result (`Input::AliasImage`).
#[derive(Debug)]
pub struct AliasCapture {
    pub window_id: u32,
    pub frame: CGRect,
    pub image: Option<CFRetained<CGImage>>,
    pub disabled: bool,
}

/// `alias_update_image`: looks the window up (unless `cached_window_id` is non-zero) and
/// captures it. On failure (not disabled) the window id is reset to 0.
pub fn capture_alias(owner: &str, name: Option<&str>, cached_window_id: u32) -> AliasCapture {
    let mut wid = cached_window_id;
    let mut frame = CGRect::default();
    if wid == 0 {
        let items = list_menu_extras();
        if let Some(w) = find_window(&items, owner, name) {
            wid = w.window_id;
            frame = w.frame;
        }
    }
    if wid == 0 {
        return AliasCapture {
            window_id: 0,
            frame,
            image: None,
            disabled: false,
        };
    }
    match capture_window(wid) {
        Capture::Image { image, frame: f } => AliasCapture {
            window_id: wid,
            frame: CGRect {
                origin: if frame.size.width > 0.0 {
                    frame.origin
                } else {
                    f.origin
                },
                size: f.size,
            },
            image: Some(image),
            disabled: false,
        },
        Capture::Disabled => AliasCapture {
            window_id: wid,
            frame,
            image: None,
            disabled: true,
        },
        Capture::Failed => AliasCapture {
            window_id: 0,
            frame,
            image: None,
            disabled: false,
        },
    }
}

// ---------------------------------------------------------------------------------------
// Periodic refresh
// ---------------------------------------------------------------------------------------

struct Entry {
    owner: String,
    name: Option<String>,
    freq: Duration,
    next: Instant,
    window_id: u32,
    last_hash: Option<u64>,
    force: bool,
}

#[derive(Default)]
struct Sched {
    entries: HashMap<u64, Entry>,
    shutdown: bool,
}

/// Background scheduler capturing each registered alias every `update_freq` seconds and
/// posting [`SysEvent::AliasUpdate`] only when the picture changed (or capture state
/// changed). `update_freq = 0` never refreshes (SketchyBar). Dropping it stops the thread.
pub struct AliasScheduler {
    shared: Arc<(Mutex<Sched>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AliasScheduler {
    pub fn new(sink: Sink) -> AliasScheduler {
        let shared = Arc::new((Mutex::new(Sched::default()), Condvar::new()));
        let s2 = shared.clone();
        let thread = std::thread::Builder::new()
            .name("mbar-alias".into())
            .spawn(move || run(s2, sink))
            .ok();
        AliasScheduler { shared, thread }
    }

    /// Adds or updates alias `id` (`update_freq` in seconds; 0 = capture once now only).
    pub fn set(&self, id: u64, owner: &str, name: Option<&str>, update_freq: u32) {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        let e = s.entries.entry(id).or_insert_with(|| Entry {
            owner: owner.to_string(),
            name: name.map(|n| n.to_string()),
            freq: Duration::ZERO,
            next: Instant::now(),
            window_id: 0,
            last_hash: None,
            force: true,
        });
        if e.owner != owner || e.name.as_deref() != name {
            e.owner = owner.to_string();
            e.name = name.map(|n| n.to_string());
            e.window_id = 0;
            e.force = true;
            e.next = Instant::now();
        }
        e.freq = Duration::from_secs(update_freq as u64);
        cv.notify_all();
    }

    /// Captures `id` as soon as possible and posts even if unchanged (`forced`).
    pub fn refresh_now(&self, id: u64) {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = s.entries.get_mut(&id) {
            e.force = true;
            e.next = Instant::now();
        }
        cv.notify_all();
    }

    pub fn remove(&self, id: u64) {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .remove(&id);
    }
}

impl Drop for AliasScheduler {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            lock.lock().unwrap_or_else(|e| e.into_inner()).shutdown = true;
            cv.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Due {
    id: u64,
    owner: String,
    name: Option<String>,
    window_id: u32,
    last_hash: Option<u64>,
    force: bool,
}

fn run(shared: Arc<(Mutex<Sched>, Condvar)>, sink: Sink) {
    let (lock, cv) = &*shared;
    loop {
        // Pick due entries.
        let due: Vec<Due> = {
            let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if s.shutdown {
                    return;
                }
                let now = Instant::now();
                let due: Vec<u64> = s
                    .entries
                    .iter()
                    .filter(|(_, e)| e.next <= now && (e.force || !e.freq.is_zero()))
                    .map(|(id, _)| *id)
                    .collect();
                if !due.is_empty() {
                    break due
                        .into_iter()
                        .filter_map(|id| {
                            let e = s.entries.get_mut(&id)?;
                            let force = e.force;
                            e.force = false;
                            e.next = now
                                + if e.freq.is_zero() {
                                    Duration::from_secs(3600 * 24)
                                } else {
                                    e.freq
                                };
                            Some(Due {
                                id,
                                owner: e.owner.clone(),
                                name: e.name.clone(),
                                window_id: e.window_id,
                                last_hash: e.last_hash,
                                force,
                            })
                        })
                        .collect();
                }
                let next = s
                    .entries
                    .values()
                    .filter(|e| e.force || !e.freq.is_zero())
                    .map(|e| e.next)
                    .min();
                s = match next {
                    Some(t) => cv
                        .wait_timeout(s, t.saturating_duration_since(now))
                        .map(|r| r.0)
                        .unwrap_or_else(|e| e.into_inner().0),
                    None => cv.wait(s).unwrap_or_else(|e| e.into_inner()),
                };
            }
        };
        for Due {
            id,
            owner,
            name,
            window_id,
            last_hash,
            force,
        } in due
        {
            let cap = capture_alias(&owner, name.as_deref(), window_id);
            let hash = cap.image.as_deref().map(image_hash);
            {
                let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(e) = s.entries.get_mut(&id) {
                    e.window_id = cap.window_id;
                    if !cap.disabled {
                        e.last_hash = hash;
                    }
                }
            }
            let changed = force || cap.disabled || hash != last_hash;
            if changed && !(cap.disabled && !force) {
                sink(SysEvent::AliasUpdate {
                    id,
                    owner,
                    name,
                    window_id: cap.window_id,
                    frame: cap.frame,
                    image: cap.image,
                    disabled: cap.disabled,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(owner: &str, name: &str, x: f64) -> MenuExtraWindow {
        MenuExtraWindow {
            owner: owner.into(),
            name: name.into(),
            pid: 1,
            window_id: x as u32,
            frame: CGRect {
                origin: CGPoint { x, y: 0.0 },
                size: CGSize {
                    width: 20.0,
                    height: 24.0,
                },
            },
        }
    }

    #[test]
    fn spec_parsing() {
        assert_eq!(
            parse_alias_spec("Control Center,Battery"),
            ("Control Center".into(), Some("Battery".into()))
        );
        assert_eq!(parse_alias_spec("A,B,C"), ("A".into(), Some("B,C".into())));
        assert_eq!(parse_alias_spec("Owner"), ("Owner".into(), None));
        assert_eq!(parse_alias_spec("Owner,"), ("Owner,".into(), None));
    }

    #[test]
    fn sorting() {
        let mut v = vec![
            item("a", "1", 10.0),
            item("b", "2", 30.0),
            item("c", "3", 20.0),
            item("d", "4", 30.0),
        ];
        sort_menu_extras(&mut v);
        let owners: Vec<&str> = v.iter().map(|i| i.owner.as_str()).collect();
        assert_eq!(owners, vec!["b", "d", "c", "a"]);
    }

    #[test]
    fn sorting_quirk() {
        let mut v = vec![
            item("a", "1", 10.0),
            item("b", "2", -10000.0),
            item("c", "3", -10000.0),
        ];
        sort_menu_extras(&mut v);
        // i=0: a; i=1: nothing > -9999 → swap(1, 0); i=2: swap(2, 0).
        let owners: Vec<&str> = v.iter().map(|i| i.owner.as_str()).collect();
        assert_eq!(owners, vec!["c", "a", "b"]);
    }

    #[test]
    fn default_items_format() {
        let v = vec![
            item("Control Center", "Battery", 30.0),
            item("Clock", "Clock", 20.0),
        ];
        assert_eq!(
            default_menu_items(&v),
            "[\n\t\"Control Center,Battery(1)\", \n\t\"Clock,Clock(2)\"\n]\n"
        );
        assert_eq!(default_menu_items(&[]), "");
    }

    #[test]
    fn lookup() {
        let v = vec![
            item("CC", "Battery", 30.0),
            item("CC", "WiFi", 20.0),
            item("X", "", 10.0),
        ];
        assert_eq!(find_window(&v, "CC", Some("WiFi")).unwrap().name, "WiFi");
        assert_eq!(find_window(&v, "CC", Some("WiFi(2)")).unwrap().name, "WiFi");
        assert!(find_window(&v, "CC", Some("WiFi(1)")).is_none());
        assert!(find_window(&v, "CC", None).is_none());
        assert_eq!(find_window(&v, "X", None).unwrap().owner, "X");
    }
}
