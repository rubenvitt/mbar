//! [`WindowManager`]: reconciles the core's [`FrameOutput`] with [`BarWindow`]s.
//!
//! * One window per [`WindowKey`]; created on first update, destroyed when listed in
//!   `closed`. Frame, level, sticky, shadow and blur are applied only when they change
//!   (the setters are no-ops otherwise).
//! * Only windows in the frame output are rendered; the [`DrawList`] is reused across
//!   frames. The last scene of each window is kept (moved out of the update, no copy) so
//!   a window can be re-rendered after a backing-scale change without a new scene.
//! * Popups (`order > 0`) are ordered to the front of their level whenever a window was
//!   (re)shown, so they stay above bars sharing the level.
//! * `BlurRegion`s become borderless, click-through child windows (background blur through
//!   SkyLight) ordered directly below their parent.
//! * View mouse events are forwarded to the platform through a [`ViewMouseSink`] with the
//!   window's key.

use super::convert::{self, blur_regions, scene_to_drawlist, BlurSpec};
use super::resources::MacResources;
use crate::gfx::scene::{DrawList, Rect as GRect};
use crate::gfx::window::{BarWindow, MouseEvent};
use mbar_core::geometry::{Point, Rect};
use mbar_core::platform::{FrameOutput, WindowKey, WindowUpdate};
use mbar_core::scene::Scene;
use objc2::MainThreadMarker;
use std::collections::BTreeMap;
use std::rc::Rc;

/// Receives mouse events of bar/popup views (main thread).
pub type ViewMouseSink = Rc<dyn Fn(WindowKey, MouseEvent)>;

fn grect(r: &Rect) -> GRect {
    GRect::new(r.x, r.y, r.width, r.height)
}

struct BlurChild {
    win: BarWindow,
    radius: u32,
}

struct Managed {
    win: BarWindow,
    scene: Scene,
    order: u32,
    shown: bool,
    blur: Option<u32>,
    children: Vec<BlurChild>,
}

/// Owner of every bar/popup window.
pub struct WindowManager {
    mtm: MainThreadMarker,
    windows: BTreeMap<WindowKey, Managed>,
    list: DrawList,
    empty: DrawList,
    blurs: Vec<BlurSpec>,
    mouse: ViewMouseSink,
    displays_changed: bool,
}

impl WindowManager {
    pub fn new(mtm: MainThreadMarker, mouse: ViewMouseSink) -> WindowManager {
        WindowManager {
            mtm,
            windows: BTreeMap::new(),
            list: DrawList::new(),
            empty: DrawList::new(),
            blurs: Vec::new(),
            mouse,
            displays_changed: false,
        }
    }

    /// Displays were reconfigured: re-render windows whose backing scale changed on the
    /// next [`apply`](Self::apply).
    pub fn mark_displays_changed(&mut self) {
        self.displays_changed = true;
    }

    /// Number of open windows (diagnostics).
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Global frame (top-left points) of `key`'s window.
    pub fn frame(&self, key: WindowKey) -> Option<Rect> {
        self.windows.get(&key).map(|m| {
            let f = m.win.frame();
            Rect::new(f.x, f.y, f.width, f.height)
        })
    }

    /// Global origin of `key`'s window (view → global mouse conversion).
    pub fn origin(&self, key: WindowKey) -> Option<Point> {
        self.frame(key).map(|f| Point::new(f.x, f.y))
    }

    /// The key of the window with WindowServer number `n`.
    pub fn key_for_window_number(&self, n: i64) -> Option<WindowKey> {
        if n <= 0 {
            return None;
        }
        self.windows
            .iter()
            .find(|(_, m)| m.win.window_number() as i64 == n)
            .map(|(k, _)| *k)
    }

    /// The topmost of our windows containing global point `p` (popups first).
    pub fn key_at(&self, p: Point) -> Option<WindowKey> {
        let hit = |want_popup: bool| {
            self.windows
                .iter()
                .filter(|(k, _)| matches!(k, WindowKey::Popup(_)) == want_popup)
                .find(|(_, m)| {
                    let f = m.win.frame();
                    p.x >= f.x && p.x < f.x + f.width && p.y >= f.y && p.y < f.y + f.height
                })
                .map(|(k, _)| *k)
        };
        hit(true).or_else(|| hit(false))
    }

    /// Applies one frame: closes, creates/updates and renders windows.
    pub fn apply(&mut self, frame: FrameOutput, res: &mut MacResources) {
        for key in &frame.closed {
            self.windows.remove(key);
        }
        let mut reorder = false;
        for update in frame.windows {
            reorder |= self.update(update, res);
        }
        if std::mem::take(&mut self.displays_changed) {
            self.redraw_stale(res);
            reorder = true;
        }
        if reorder {
            self.reorder();
        }
    }

    /// Creates/updates and renders one window. Returns whether it was newly shown.
    fn update(&mut self, u: WindowUpdate, res: &mut MacResources) -> bool {
        let mtm = self.mtm;
        let key = u.key;
        let m = match self.windows.entry(key) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => {
                let win = BarWindow::new(mtm, &res.renderer, grect(&u.frame));
                let sink = self.mouse.clone();
                win.set_mouse_handler(Some(Box::new(move |ev| sink(key, ev))));
                v.insert(Managed {
                    win,
                    scene: Scene::default(),
                    order: u.order,
                    shown: false,
                    blur: None,
                    children: Vec::new(),
                })
            }
        };
        m.win.set_frame(grect(&u.frame));
        m.win.set_level(convert::window_level(u.level));
        m.win.set_sticky(u.sticky);
        m.win.set_shadow(u.shadow);
        m.order = u.order;
        m.scene = u.scene;
        res.text.system.set_font_smoothing(u.font_smoothing);

        let mut look = res.lookup();
        scene_to_drawlist(&m.scene, &mut self.list, &mut look);
        m.win
            .render(&self.list, &mut res.renderer, &mut res.text.system, &res.images.store);
        let newly_shown = !m.shown;
        if newly_shown {
            m.win.show();
            m.shown = true;
        }
        if m.blur != Some(u.blur_radius) && (m.win.set_blur(u.blur_radius) || u.blur_radius == 0)
        {
            m.blur = Some(u.blur_radius);
        }

        // Blur child windows for the scene's BlurRegions.
        self.blurs.clear();
        self.blurs.extend(blur_regions(&m.scene));
        let parent = m.win.frame();
        let level = m.win.level();
        m.children.truncate(self.blurs.len());
        let mut created = false;
        for (i, spec) in self.blurs.iter().enumerate() {
            let global = GRect::new(
                parent.x + spec.rect.x,
                parent.y + spec.rect.y,
                spec.rect.width,
                spec.rect.height,
            );
            if i == m.children.len() {
                let mut win = BarWindow::new(mtm, &res.renderer, global);
                win.set_ignores_mouse(true);
                win.render(
                    &self.empty,
                    &mut res.renderer,
                    &mut res.text.system,
                    &res.images.store,
                );
                win.show();
                m.children.push(BlurChild { win, radius: 0 });
                created = true;
            }
            let c = &mut m.children[i];
            c.win.set_frame(global);
            c.win.set_level(level);
            c.win.set_sticky(u.sticky);
            c.win.set_corner_radius(spec.corner_radius);
            if c.radius != spec.radius && c.win.set_blur(spec.radius) {
                c.radius = spec.radius;
            }
        }
        if created || newly_shown {
            let n = m.win.window_number();
            for c in &m.children {
                c.win.order_relative(false, n);
            }
        }
        newly_shown || created
    }

    /// Re-renders windows whose backing scale differs from their last render.
    fn redraw_stale(&mut self, res: &mut MacResources) {
        for m in self.windows.values_mut() {
            if !m.win.needs_redraw() {
                continue;
            }
            let mut look = res.lookup();
            scene_to_drawlist(&m.scene, &mut self.list, &mut look);
            m.win
                .render(&self.list, &mut res.renderer, &mut res.text.system, &res.images.store);
            for c in &mut m.children {
                if c.win.needs_redraw() {
                    c.win.render(
                        &self.empty,
                        &mut res.renderer,
                        &mut res.text.system,
                        &res.images.store,
                    );
                }
            }
        }
    }

    /// Popups (and anything with `order > 0`) to the front of their level, in order.
    fn reorder(&self) {
        let mut ordered: Vec<(&u32, &Managed)> = self
            .windows
            .values()
            .filter(|m| m.order > 0 && m.shown)
            .map(|m| (&m.order, m))
            .collect();
        ordered.sort_by_key(|(o, _)| **o);
        for (_, m) in ordered {
            m.win.order_relative(true, 0);
            let n = m.win.window_number();
            for c in &m.children {
                c.win.order_relative(false, n);
            }
        }
    }

    /// Closes every window.
    pub fn close_all(&mut self) {
        self.windows.clear();
    }
}
