//! Global and local mouse monitors (`docs/spec/item.md` §9, `docs/spec/events.md` §6).
//!
//! SketchyBar receives mouse events through Carbon handlers on its own windows. mbar's bar
//! windows (gfx) get clicks/scrolls/tracking through their views; this module adds
//! `NSEvent` monitors so the integration layer can
//!
//! * see pointer motion over **other apps' windows** (global monitor) — needed to notice
//!   that the pointer left a bar/popup window without an exit event and to drive
//!   `mouse.entered.global` / `mouse.exited.global` from window geometry, and
//! * observe events targeted at mbar's own windows (local monitor; events pass through
//!   unchanged).
//!
//! Every event is reported as [`SysEvent::Mouse`] with the CG global location (top-left
//! origin), the raw `CGEventFlags` (truncated to u32 like `modfier_code`), the CG event type
//! and button number, and the number of the window under the pointer
//! (`+[NSWindow windowNumberAtPoint:belowWindowWithWindowNumber:]`, any application).
//! [`HoverTracker`] turns pointer positions + window rects into enter/exit transitions.
//!
//! Global monitors for mouse events need no permission (only key events require
//! Accessibility).

use super::{Sink, SysEvent};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSWindow};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{CGEvent, CGEventField, CGEventFlags};
use std::ptr::NonNull;

/// Kind of a monitored mouse event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseKind {
    Moved,
    Dragged,
    Down,
    /// Button release (SketchyBar's click).
    Up,
    /// `delta` = `kCGScrollWheelEventDeltaAxis1` (integer lines, SketchyBar's value);
    /// `point_delta` = pixel delta for smooth scrolling.
    Scrolled { delta: i32, point_delta: f64 },
}

/// One monitored mouse event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    /// Global location, points, top-left origin (`CGEventGetLocation`).
    pub location: CGPoint,
    /// `kCGMouseEventButtonNumber` (0 left, 1 right, 2 middle, …).
    pub button: i64,
    /// Raw `CGEventType` (e.g. 2 = left up, 4 = right up → `left`/`right`/`other`).
    pub cg_type: u32,
    /// `CGEventFlags` truncated to u32.
    pub modifiers: u32,
    /// Window number under the pointer (any app), 0 if none.
    pub window_number: i64,
    /// `true` when delivered by the global monitor (event targeted another application).
    pub global: bool,
}

/// Which events to monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorOptions {
    /// Events targeted at other applications.
    pub global: bool,
    /// Events targeted at mbar's own windows.
    pub local: bool,
    /// Include mouse-moved / dragged (high frequency).
    pub motion: bool,
    /// Include button down/up.
    pub clicks: bool,
    /// Include scroll wheel.
    pub scroll: bool,
}

impl Default for MonitorOptions {
    fn default() -> Self {
        MonitorOptions {
            global: true,
            local: false,
            motion: true,
            clicks: true,
            scroll: true,
        }
    }
}

fn mask(opts: &MonitorOptions) -> NSEventMask {
    let mut m = NSEventMask::empty();
    if opts.motion {
        m |= NSEventMask::MouseMoved
            | NSEventMask::LeftMouseDragged
            | NSEventMask::RightMouseDragged
            | NSEventMask::OtherMouseDragged;
    }
    if opts.clicks {
        m |= NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseUp
            | NSEventMask::RightMouseDown
            | NSEventMask::RightMouseUp
            | NSEventMask::OtherMouseDown
            | NSEventMask::OtherMouseUp;
    }
    if opts.scroll {
        m |= NSEventMask::ScrollWheel;
    }
    m
}

fn kind_of(event: &NSEvent, cg: Option<&CGEvent>) -> Option<MouseKind> {
    let t = event.r#type();
    Some(match t {
        NSEventType::MouseMoved => MouseKind::Moved,
        NSEventType::LeftMouseDragged | NSEventType::RightMouseDragged | NSEventType::OtherMouseDragged => {
            MouseKind::Dragged
        }
        NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown => MouseKind::Down,
        NSEventType::LeftMouseUp | NSEventType::RightMouseUp | NSEventType::OtherMouseUp => MouseKind::Up,
        NSEventType::ScrollWheel => MouseKind::Scrolled {
            delta: CGEvent::integer_value_field(cg, CGEventField::ScrollWheelEventDeltaAxis1) as i32,
            point_delta: event.scrollingDeltaY(),
        },
        _ => return None,
    })
}

/// Converts an `NSEvent` into a [`MouseEvent`] (main thread).
pub fn convert(event: &NSEvent, global: bool, mtm: MainThreadMarker) -> Option<MouseEvent> {
    let cg = event.CGEvent();
    let kind = kind_of(event, cg.as_deref())?;
    let window_number = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(NSEvent::mouseLocation(), 0, mtm);
    Some(MouseEvent {
        kind,
        location: CGEvent::location(cg.as_deref()),
        button: CGEvent::integer_value_field(cg.as_deref(), CGEventField::MouseEventButtonNumber),
        cg_type: CGEvent::r#type(cg.as_deref()).0,
        modifiers: CGEvent::flags(cg.as_deref()).0 as u32,
        window_number: window_number as i64,
        global,
    })
}

/// Installed NSEvent monitors; removed on drop.
pub struct MouseMonitor {
    monitors: Vec<Retained<AnyObject>>,
}

impl MouseMonitor {
    /// Installs the monitors (main thread). Handlers run on the main thread and call `sink`.
    pub fn start(sink: Sink, opts: MonitorOptions, _mtm: MainThreadMarker) -> MouseMonitor {
        let m = mask(&opts);
        let mut monitors = Vec::new();
        if opts.global {
            let s = sink.clone();
            let block = RcBlock::new(move |ev: NonNull<NSEvent>| {
                // SAFETY: AppKit passes a valid event for the duration of the handler.
                let ev = unsafe { ev.as_ref() };
                if let Some(mtm) = MainThreadMarker::new() {
                    if let Some(e) = convert(ev, true, mtm) {
                        s(SysEvent::Mouse(e));
                    }
                }
            });
            if let Some(mon) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(m, &block) {
                monitors.push(mon);
            }
        }
        if opts.local {
            let s = sink;
            let block = RcBlock::new(move |ev: NonNull<NSEvent>| -> *mut NSEvent {
                // SAFETY: AppKit passes a valid event for the duration of the handler.
                let e = unsafe { ev.as_ref() };
                if let Some(mtm) = MainThreadMarker::new() {
                    if let Some(e) = convert(e, false, mtm) {
                        s(SysEvent::Mouse(e));
                    }
                }
                // Pass the event through unchanged.
                ev.as_ptr()
            });
            // SAFETY: the handler returns the (unmodified) event, as required.
            if let Some(mon) = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(m, &block) } {
                monitors.push(mon);
            }
        }
        MouseMonitor { monitors }
    }

    /// Removes all monitors.
    pub fn stop(&mut self) {
        for m in self.monitors.drain(..) {
            // SAFETY: `m` was returned by `addGlobal/LocalMonitorForEventsMatchingMask`.
            unsafe { NSEvent::removeMonitor(&m) };
        }
    }
}

impl Drop for MouseMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `CGEventGetLocation(CGEventCreate(NULL))`: current pointer location (global top-left).
pub fn cursor_location() -> CGPoint {
    super::displays::cursor_location()
}

/// `CGEventFlags` → SketchyBar modifier description (`shift,ctrl,alt,cmd,fn` or `none`).
pub fn modifier_description(flags: u64) -> String {
    let f = CGEventFlags(flags);
    let mut parts = Vec::new();
    for (mask, name) in [
        (CGEventFlags::MaskShift, "shift"),
        (CGEventFlags::MaskControl, "ctrl"),
        (CGEventFlags::MaskAlternate, "alt"),
        (CGEventFlags::MaskCommand, "cmd"),
        (CGEventFlags::MaskSecondaryFn, "fn"),
    ] {
        if f.contains(mask) {
            parts.push(name);
        }
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(",")
    }
}

/// CG event type → `left` / `right` / `other` (`get_type_description`).
pub fn button_description(cg_type: u32) -> &'static str {
    match cg_type {
        2 => "left",
        4 => "right",
        _ => "other",
    }
}

/// A hover transition computed by [`HoverTracker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hover<K> {
    Entered(K),
    Exited(K),
}

/// Tracks which of a set of rectangles (bar/popup windows) contains the pointer and reports
/// enter/exit transitions. The first matching rect in the given order wins (pass windows
/// front to back). Containment is half-open (`CGRectContainsPoint`).
#[derive(Debug, Clone, Default)]
pub struct HoverTracker<K> {
    current: Option<K>,
}

impl<K: Clone + PartialEq> HoverTracker<K> {
    pub fn new() -> Self {
        HoverTracker { current: None }
    }

    /// Key currently hovered.
    pub fn current(&self) -> Option<&K> {
        self.current.as_ref()
    }

    /// Feeds a pointer position; returns the transitions (exit before enter).
    pub fn update(&mut self, p: CGPoint, windows: &[(K, CGRect)]) -> Vec<Hover<K>> {
        let hit = windows
            .iter()
            .find(|(_, r)| {
                p.x >= r.origin.x
                    && p.x < r.origin.x + r.size.width
                    && p.y >= r.origin.y
                    && p.y < r.origin.y + r.size.height
            })
            .map(|(k, _)| k.clone());
        if hit == self.current {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(old) = self.current.take() {
            out.push(Hover::Exited(old));
        }
        if let Some(new) = hit.clone() {
            out.push(Hover::Entered(new));
        }
        self.current = hit;
        out
    }

    /// Forgets the hovered key (e.g. after windows were rebuilt).
    pub fn reset(&mut self) {
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::CGSize;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width: w, height: h },
        }
    }

    #[test]
    fn modifiers() {
        assert_eq!(modifier_description(0x100), "none");
        assert_eq!(modifier_description(131072 | 1048576), "shift,cmd");
        assert_eq!(modifier_description(8388608 | 524288 | 262144), "ctrl,alt,fn");
        assert_eq!(button_description(2), "left");
        assert_eq!(button_description(4), "right");
        assert_eq!(button_description(26), "other");
    }

    #[test]
    fn hover() {
        let wins = vec![("bar", rect(0.0, 0.0, 100.0, 30.0)), ("popup", rect(10.0, 30.0, 50.0, 50.0))];
        let mut t = HoverTracker::new();
        assert_eq!(t.update(CGPoint { x: 5.0, y: 5.0 }, &wins), vec![Hover::Entered("bar")]);
        assert!(t.update(CGPoint { x: 6.0, y: 5.0 }, &wins).is_empty());
        assert_eq!(
            t.update(CGPoint { x: 20.0, y: 40.0 }, &wins),
            vec![Hover::Exited("bar"), Hover::Entered("popup")]
        );
        assert_eq!(t.update(CGPoint { x: 500.0, y: 500.0 }, &wins), vec![Hover::Exited("popup")]);
        assert_eq!(t.current(), None);
        // half-open edge
        assert!(t.update(CGPoint { x: 100.0, y: 5.0 }, &wins).is_empty());
    }
}
