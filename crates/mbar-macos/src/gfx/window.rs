//! Bar / popup windows: a borderless, non-activating `NSPanel` whose content view hosts a
//! `CAMetalLayer` (spec `bar.md` §3).
//!
//! * Geometry is given in global **top-left** points (CG display coordinates) and
//!   converted to AppKit's bottom-left space with the primary screen's height.
//! * Levels follow SketchyBar ([`level`]); `sticky` windows join all spaces.
//! * Background blur uses the private SkyLight call `SLSSetWindowBackgroundBlurRadius`.
//! * Mouse input is forwarded from an `NSView` subclass to a closure with locations in
//!   window points (top-left origin).
//!
//! Drawables are only requested inside [`BarWindow::render`]; the integration layer
//! renders only dirty windows (and those reporting [`BarWindow::needs_redraw`]).

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{
    define_class, msg_send, AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSEventModifierFlags, NSPanel, NSResponder, NSScreen,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{kCAGravityTopLeft, CAMetalLayer, CATransaction};

use super::image::ImageStore;
use super::renderer::Renderer;
use super::scene::{DrawList, Point, Rect};
use super::text::TextSystem;
use super::util;

/// SketchyBar window levels (`bar.md` §3.7, CGWindowLevel values).
pub mod level {
    /// `kCGBackstopMenuLevel` — default bar level (below normal windows).
    pub const BACKSTOP_MENU: isize = -20;
    /// `kCGBackstopMenuLevel + 1` — popups with `popup.topmost=off`.
    pub const BACKSTOP_MENU_ABOVE: isize = -19;
    /// `kCGNormalWindowLevel`.
    pub const NORMAL: isize = 0;
    /// `kCGFloatingWindowLevel` — `topmost=window`.
    pub const FLOATING: isize = 3;
    /// `kCGStatusWindowLevel` — `topmost=on`.
    pub const STATUS: isize = 25;
    /// `kCGPopUpMenuWindowLevel` — popups (`popup.topmost=on`, the default).
    pub const POPUP_MENU: isize = 101;

    /// Bar level for SketchyBar's `topmost` value.
    pub fn for_topmost(topmost: Topmost) -> isize {
        match topmost {
            Topmost::Off => BACKSTOP_MENU,
            Topmost::Window => FLOATING,
            Topmost::On => STATUS,
        }
    }

    /// Popup level for `popup.topmost`.
    pub fn for_popup(topmost: bool) -> isize {
        if topmost {
            POPUP_MENU
        } else {
            BACKSTOP_MENU_ABOVE
        }
    }

    /// `topmost` property values.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Topmost {
        Off,
        Window,
        On,
    }
}

#[link(name = "SkyLight", kind = "framework")]
extern "C" {
    fn SLSMainConnectionID() -> i32;
    fn SLSSetWindowBackgroundBlurRadius(cid: i32, wid: u32, radius: u32) -> i32;
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Down,
    Up,
    Dragged,
    Moved,
    Entered,
    Exited,
    Scroll,
}

/// Which button (for scroll/move/enter/exit: [`MouseButton::None`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    None,
    Left,
    Right,
    /// `buttonNumber` of other buttons (2 = middle).
    Other(i32),
}

/// Keyboard modifiers held during the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
    pub caps_lock: bool,
    pub function: bool,
    /// Raw `NSEventModifierFlags`.
    pub raw: u64,
}

/// A mouse event on a bar window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub button: MouseButton,
    /// Location in window points, origin top-left.
    pub location: Point,
    pub modifiers: Modifiers,
    /// Scroll deltas (`scrollingDeltaX/Y`).
    pub scroll_delta: (f32, f32),
    /// `hasPreciseScrollingDeltas` (trackpad).
    pub precise_scroll: bool,
    pub click_count: i32,
}

/// Mouse event callback.
pub type MouseHandler = Box<dyn FnMut(MouseEvent)>;

/// Instance variables of [`BarView`].
pub struct BarViewIvars {
    handler: RefCell<Option<MouseHandler>>,
}

define_class!(
    // SAFETY:
    // - NSView has no special subclassing requirements; we only override event methods.
    // - `BarView` does not implement `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MbarBarView"]
    #[ivars = BarViewIvars]
    pub struct BarView;

    impl BarView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Down, MouseButton::Left);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Up, MouseButton::Left);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Dragged, MouseButton::Left);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Down, MouseButton::Right);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Up, MouseButton::Right);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Dragged, MouseButton::Right);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            let b = MouseButton::Other(event.buttonNumber() as i32);
            self.forward(event, MouseEventKind::Down, b);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            let b = MouseButton::Other(event.buttonNumber() as i32);
            self.forward(event, MouseEventKind::Up, b);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            let b = MouseButton::Other(event.buttonNumber() as i32);
            self.forward(event, MouseEventKind::Dragged, b);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Moved, MouseButton::None);
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Entered, MouseButton::None);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Exited, MouseButton::None);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.forward(event, MouseEventKind::Scroll, MouseButton::None);
        }
    }
);

fn modifiers_of(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        cmd: flags.contains(NSEventModifierFlags::Command),
        caps_lock: flags.contains(NSEventModifierFlags::CapsLock),
        function: flags.contains(NSEventModifierFlags::Function),
        raw: flags.0 as u64,
    }
}

impl BarView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(BarViewIvars {
            handler: RefCell::new(None),
        });
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect
            | NSTrackingAreaOptions::EnabledDuringMouseDrag;
        let owner: &AnyObject = &view;
        // SAFETY: the owner (the view) outlives the tracking area, which the view itself
        // retains; tracking areas do not retain their owner. `InVisibleRect` makes the
        // rect argument irrelevant.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(owner),
                None,
            )
        };
        view.addTrackingArea(&area);
        view
    }

    fn set_handler(&self, handler: Option<MouseHandler>) {
        *self.ivars().handler.borrow_mut() = handler;
    }

    fn forward(&self, event: &NSEvent, kind: MouseEventKind, button: MouseButton) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let (scroll_delta, precise_scroll) = if kind == MouseEventKind::Scroll {
            (
                (
                    event.scrollingDeltaX() as f32,
                    event.scrollingDeltaY() as f32,
                ),
                event.hasPreciseScrollingDeltas(),
            )
        } else {
            ((0.0, 0.0), false)
        };
        let click_count = match kind {
            MouseEventKind::Down | MouseEventKind::Up | MouseEventKind::Dragged => {
                event.clickCount() as i32
            }
            _ => 0,
        };
        let ev = MouseEvent {
            kind,
            button,
            // The view is flipped, so view coordinates are already top-left based.
            location: Point::new(p.x as f32, p.y as f32),
            modifiers: modifiers_of(event.modifierFlags()),
            scroll_delta,
            precise_scroll,
            click_count,
        };
        // Take the handler out while calling it, so re-entrant events (nested run loops,
        // e.g. while a menu is open) cannot double-borrow it.
        let taken = self.ivars().handler.borrow_mut().take();
        if let Some(mut handler) = taken {
            handler(ev);
            let mut slot = self.ivars().handler.borrow_mut();
            if slot.is_none() {
                *slot = Some(handler);
            }
        }
    }
}

/// Height of the primary screen (the one at AppKit origin), for coordinate flipping.
fn primary_screen_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map(|s| s.frame().size.height)
        .unwrap_or(0.0)
}

/// A bar or popup window.
pub struct BarWindow {
    mtm: MainThreadMarker,
    window: Retained<NSPanel>,
    view: Retained<BarView>,
    layer: Retained<CAMetalLayer>,
    frame: Rect,
    level: isize,
    sticky: bool,
    shadow: bool,
    blur_radius: u32,
    corner_radius: f32,
    /// `(drawable w, drawable h, scale bits)` applied to the layer.
    layer_state: (u32, u32, u32),
    rendered_scale: f32,
}

impl BarWindow {
    /// Create a hidden window at `frame` (global top-left points) with the default bar
    /// configuration: backstop-menu level, sticky, no shadow, no blur.
    pub fn new(mtm: MainThreadMarker, renderer: &Renderer, frame: Rect) -> Self {
        let (x, y, w, h) = util::top_left_to_appkit(frame, primary_screen_height(mtm));
        let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)));
        let window = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: we own the window through `Retained`; AppKit must not release it again
        // when it is closed.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setHidesOnDeactivate(false);
        window.setCanHide(false);
        window.setMovable(false);
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        window.setAcceptsMouseMovedEvents(true);
        window.setBecomesKeyOnlyIfNeeded(true);

        let content = NSRect::new(NSPoint::new(0.0, 0.0), rect.size);
        let view = BarView::new(mtm, content);
        let layer = CAMetalLayer::new();
        renderer.configure_layer(&layer);
        layer.setNeedsDisplayOnBoundsChange(false);
        // SAFETY: `kCAGravityTopLeft` is a constant exported by QuartzCore.
        layer.setContentsGravity(unsafe { kCAGravityTopLeft });
        // Layer-hosting view: set the layer first, then enable layer backing.
        view.setLayer(Some(&layer));
        view.setWantsLayer(true);
        window.setContentView(Some(&view));

        let mut this = BarWindow {
            mtm,
            window,
            view,
            layer,
            frame,
            level: level::BACKSTOP_MENU,
            sticky: true,
            shadow: false,
            blur_radius: 0,
            corner_radius: 0.0,
            layer_state: (0, 0, 0),
            rendered_scale: 0.0,
        };
        this.window.setLevel(level::BACKSTOP_MENU);
        this.apply_collection_behavior();
        this.sync_layer();
        this
    }

    /// The underlying AppKit window.
    pub fn ns_window(&self) -> &NSWindow {
        &self.window
    }

    /// `windowNumber` (the WindowServer window id used by SkyLight calls).
    pub fn window_number(&self) -> isize {
        self.window.windowNumber()
    }

    /// Current frame in global top-left points.
    pub fn frame(&self) -> Rect {
        self.frame
    }

    /// Move/resize (global top-left points). No-op if unchanged.
    pub fn set_frame(&mut self, frame: Rect) {
        if frame == self.frame {
            return;
        }
        self.frame = frame;
        let (x, y, w, h) = util::top_left_to_appkit(frame, primary_screen_height(self.mtm));
        let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)));
        self.window.setFrame_display(rect, false);
        self.sync_layer();
    }

    /// Applies the current frame again (after SkyLight moved the window, e.g. to another
    /// space).
    pub fn reapply_frame(&self) {
        let (x, y, w, h) = util::top_left_to_appkit(self.frame, primary_screen_height(self.mtm));
        let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)));
        self.window.setFrame_display(rect, false);
    }

    /// Window level (see [`level`]).
    pub fn set_level(&mut self, level: isize) {
        if self.level != level {
            self.level = level;
            self.window.setLevel(level);
        }
    }

    pub fn level(&self) -> isize {
        self.level
    }

    /// `sticky`: show on all spaces (`canJoinAllSpaces | stationary | ignoresCycle |
    /// fullScreenAuxiliary`); otherwise the window stays on the space it was put on.
    pub fn set_sticky(&mut self, sticky: bool) {
        if self.sticky != sticky {
            self.sticky = sticky;
            self.apply_collection_behavior();
        }
    }

    /// Whether the window joins all spaces (see [`set_sticky`](Self::set_sticky)).
    pub fn is_sticky(&self) -> bool {
        self.sticky
    }

    fn apply_collection_behavior(&self) {
        let base = NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let behavior = if self.sticky {
            base | NSWindowCollectionBehavior::CanJoinAllSpaces
        } else {
            base
        };
        self.window.setCollectionBehavior(behavior);
    }

    /// Window background blur through SkyLight. Returns `true` on success.
    pub fn set_blur(&mut self, radius: u32) -> bool {
        self.blur_radius = radius;
        let wid = self.window.windowNumber();
        if wid <= 0 {
            return false;
        }
        // SAFETY: private SkyLight calls with the process' main connection and a window
        // id owned by this process; both only take plain integers.
        let err =
            unsafe { SLSSetWindowBackgroundBlurRadius(SLSMainConnectionID(), wid as u32, radius) };
        err == 0
    }

    pub fn blur_radius(&self) -> u32 {
        self.blur_radius
    }

    /// Round the window content (layer corner mask).
    pub fn set_corner_radius(&mut self, radius: f32) {
        let radius = radius.max(0.0);
        if self.corner_radius == radius {
            return;
        }
        self.corner_radius = radius;
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.layer.setCornerRadius(radius as f64);
        self.layer.setMasksToBounds(radius > 0.0);
        CATransaction::commit();
    }

    /// System window shadow (`bar.shadow`).
    pub fn set_shadow(&mut self, on: bool) {
        if self.shadow != on {
            self.shadow = on;
            self.window.setHasShadow(on);
        }
    }

    /// Order the window in front (without activating the app).
    pub fn show(&self) {
        self.window.orderFrontRegardless();
    }

    /// Order the window out.
    pub fn hide(&self) {
        self.window.orderOut(None);
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    /// Place this window directly above (`true`) or below (`false`) the window with
    /// `window_number` within its level (0 = front/back of the level).
    pub fn order_relative(&self, above: bool, window_number: isize) {
        let mode = if above {
            NSWindowOrderingMode::Above
        } else {
            NSWindowOrderingMode::Below
        };
        self.window.orderWindow_relativeTo(mode, window_number);
    }

    /// Let clicks pass through the window.
    pub fn set_ignores_mouse(&self, ignore: bool) {
        self.window.setIgnoresMouseEvents(ignore);
    }

    /// Install (or remove) the mouse callback.
    pub fn set_mouse_handler(&self, handler: Option<MouseHandler>) {
        self.view.set_handler(handler);
    }

    /// Backing scale factor of the screen the window is on.
    pub fn backing_scale(&self) -> f32 {
        self.window.backingScaleFactor() as f32
    }

    /// `true` when the backing scale changed since the last render (window moved to a
    /// display with a different density): the content must be re-rendered.
    pub fn needs_redraw(&self) -> bool {
        self.backing_scale() != self.rendered_scale
    }

    /// Apply size/scale changes to the layer (no implicit animations).
    fn sync_layer(&mut self) {
        let scale = self.backing_scale().max(1.0);
        let (pw, ph) = util::drawable_size(self.frame.width, self.frame.height, scale);
        let state = (pw, ph, scale.to_bits());
        if state == self.layer_state {
            return;
        }
        self.layer_state = state;
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.layer.setContentsScale(scale as f64);
        self.layer.setFrame(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(
                self.frame.width.max(1.0) as f64,
                self.frame.height.max(1.0) as f64,
            ),
        ));
        self.layer
            .setDrawableSize(CGSize::new(pw as f64, ph as f64));
        CATransaction::commit();
    }

    /// Render `list` and present it. Acquires a drawable only now. Returns `false` if
    /// nothing was presented (e.g. no drawable available).
    pub fn render(
        &mut self,
        list: &DrawList,
        renderer: &mut Renderer,
        text: &mut TextSystem,
        images: &ImageStore,
    ) -> bool {
        self.sync_layer();
        let scale = f32::from_bits(self.layer_state.2);
        // Use the exact drawable extent in points so the pixel grid matches.
        let size = (
            self.layer_state.0 as f32 / scale,
            self.layer_state.1 as f32 / scale,
        );
        let presented = renderer.render(&self.layer, list, size, scale, text, images);
        if presented {
            self.rendered_scale = self.backing_scale();
            if self.shadow {
                self.window.invalidateShadow();
            }
        }
        presented
    }
}

impl Drop for BarWindow {
    fn drop(&mut self) {
        self.view.set_handler(None);
        self.window.orderOut(None);
        self.window.close();
    }
}
