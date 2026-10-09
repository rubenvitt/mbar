//! One border: its own SkyLight connection and window, kept in sync with the target window
//! (`docs/spec/borders.md` §7.1–§7.8, JankyBorders `src/border.c`, `src/misc/window.h`).
//!
//! Deviations from the C code, all main-thread and synchronous:
//! * No per-border mutex and no HIGH-priority queue for moves (BR-DRW-09): every call
//!   happens on the main thread inside the notify proc, so moves are applied in order.
//! * `SLSReenableUpdate` always follows `SLSDisableUpdate`, even when the transaction
//!   cannot be created (BQ15).
//! * The sub-level comes from `SLSGetWindowSubLevel` instead of the raw MIG request
//!   (§7.6, BQ27).
//! * The border window follows its target to another space: when the target's space id
//!   changes, the border window is moved there again with `SLSMoveWindowsToManagedSpace`
//!   (spec §14 open question 3; JankyBorders moves it only at creation, BQ16). Space ids
//!   stay 64-bit (BQ16, BQ17).
//! * A failed `SLSNewWindow` / `CGSNewRegionWithRect` is logged and skipped instead of
//!   asserting (BQ30).

use super::{draw, ffi, space};
use mbar_core::borders::{
    backing_resolution, border_geometry, border_window_tags, draw_plan, inner_radius, move_origin,
    order_value, BorderRect, BorderSettings, WINDOW_TAG_STICKY,
};
use objc2_core_foundation::{CFRetained, CGAffineTransform, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGContext, CGInterpolationQuality};

fn cg_rect(r: BorderRect) -> CGRect {
    CGRect::new(CGPoint::new(r.x, r.y), CGSize::new(r.width, r.height))
}

fn border_rect(r: CGRect) -> BorderRect {
    BorderRect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// The border of one target window (`struct border`).
pub(super) struct Border {
    /// The border's own connection (`SLSNewConnection`), or the main one if that failed.
    cid: i32,
    own_cid: bool,
    /// The border window (0 until the first visible update creates it).
    wid: u32,
    pub(super) target_wid: u32,
    /// The target's space (from the create event or `window_space_id`).
    pub(super) sid: u64,
    /// The space the border window was last sent to.
    window_sid: u64,
    pub(super) focused: bool,
    pub(super) needs_redraw: bool,
    too_small: bool,
    pub(super) sticky: bool,
    /// The target's corner radius; the inner radius is `radius + 1`.
    pub(super) radius: f64,
    origin: CGPoint,
    /// The border window's frame (origin 0,0); `None` before the window exists.
    frame: Option<BorderRect>,
    context: Option<CFRetained<CGContext>>,
}

impl Border {
    /// `border_create`: a new connection for the border (BR-DRW-01); no window yet.
    pub(super) fn new(target_wid: u32) -> Border {
        let (cid, own_cid) = match ffi::new_connection() {
            Some(cid) => (cid, true),
            None => (ffi::main_cid(), false),
        };
        Border {
            cid,
            own_cid,
            wid: 0,
            target_wid,
            sid: 0,
            window_sid: 0,
            focused: false,
            needs_redraw: true,
            too_small: false,
            sticky: false,
            radius: mbar_core::borders::DEFAULT_CORNER_RADIUS,
            origin: CGPoint::new(0.0, 0.0),
            frame: None,
            context: None,
        }
    }

    /// `window_create` + `border_create_window` (BR-DRW-02): the window, its context and
    /// its space. Leaves `wid == 0` when SkyLight refuses.
    fn create_window(&mut self, frame: BorderRect, hidpi: bool) {
        let cid = self.cid;
        let Some(region) = ffi::new_region(cg_rect(frame)) else {
            log::warn!("borders: CGSNewRegionWithRect failed");
            return;
        };
        let wid = ffi::new_window(cid, &region);
        drop(region);
        if wid == 0 {
            log::warn!(
                "borders: SLSNewWindow failed for window {}",
                self.target_wid
            );
            return;
        }
        let (set, clear) = border_window_tags(false);
        ffi::set_resolution(cid, wid, backing_resolution(hidpi));
        ffi::set_tags(cid, wid, set);
        ffi::clear_tags(cid, wid, clear);
        ffi::set_opacity(cid, wid, false);
        ffi::disable_shadow(wid);

        self.wid = wid;
        self.frame = Some(frame);
        self.needs_redraw = true;
        self.context = ffi::window_context(cid, wid);
        match &self.context {
            Some(ctx) => {
                CGContext::set_interpolation_quality(Some(ctx), CGInterpolationQuality::None)
            }
            None => log::warn!("borders: SLWindowContextCreate failed for {wid}"),
        }
        if self.sid == 0 {
            self.sid = space::window_space_id(self.target_wid);
        }
        self.send_to_space();
    }

    /// `window_send_to_space(cid, wid, sid)`.
    fn send_to_space(&mut self) {
        ffi::move_to_space(self.cid, self.wid, self.sid);
        self.window_sid = self.sid;
    }

    /// `border_update_internal` (BR-DRW-04) with the effective `settings`.
    pub(super) fn update(&mut self, settings: &BorderSettings, macos26: bool) {
        let cid = self.cid;
        // Step 2: bounds. A window that vanished has no bounds: hide like too_small.
        let Some(bounds) = ffi::window_bounds(cid, self.target_wid) else {
            self.hide();
            return;
        };
        let geometry = border_geometry(border_rect(bounds), settings, inner_radius(self.radius));
        self.too_small = geometry.too_small;
        if geometry.too_small {
            self.hide();
            return;
        }
        self.origin = CGPoint::new(geometry.origin.x, geometry.origin.y);

        // Step 3: sticky / visible space. Tags and level come from one query.
        let (tags, level) = space::window_tags_and_level(cid, self.target_wid);
        self.sticky = tags & WINDOW_TAG_STICKY != 0;
        // The target changed spaces (a create event with a new space id, or the space pass
        // refreshed it): send the border window after it **before** the visibility check,
        // so a border whose target left for a hidden space does not stay behind on the
        // visible one (spec §14 open question 3).
        if self.wid != 0 && !self.sticky && self.sid != 0 && self.sid != self.window_sid {
            self.send_to_space();
        }
        if !self.sticky && !space::is_space_visible(self.sid) {
            return;
        }

        // Step 4: minimized / hidden windows are ordered out.
        if !ffi::is_ordered_in(cid, self.target_wid) {
            self.hide();
            return;
        }

        // Step 5.
        let sub_level = ffi::window_sub_level(ffi::main_cid(), self.target_wid);

        // Step 6: lazy creation.
        if self.wid == 0 {
            self.create_window(geometry.frame, settings.hidpi);
            if self.wid == 0 {
                return;
            }
        }
        let wid = self.wid;

        // Step 7: reshape when the size changed.
        let mut disabled_update = false;
        if self.frame != Some(geometry.frame) {
            disabled_update = true;
            ffi::disable_update(cid);
            ffi::freeze(cid, wid);
            match ffi::new_region(cg_rect(geometry.frame)) {
                Some(region) => ffi::set_shape(
                    cid,
                    wid,
                    self.origin.x as f32,
                    self.origin.y as f32,
                    &region,
                ),
                None => log::warn!("borders: CGSNewRegionWithRect failed"),
            }
            self.needs_redraw = true;
            self.frame = Some(geometry.frame);
        }

        // Step 8.
        if self.needs_redraw {
            self.needs_redraw = false;
            let plan = draw_plan(&geometry, self.radius, settings, self.focused, macos26);
            match &self.context {
                Some(ctx) => draw::draw(ctx, cid, wid, &plan),
                None => {
                    ffi::flush_window(cid, wid);
                    ffi::thaw(cid, wid);
                }
            }
        }

        // Step 9: one transaction.
        if let Some(t) = ffi::Transaction::new(cid) {
            t.move_with_group(wid, self.origin);
            t.set_transform(
                wid,
                CGAffineTransform {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    tx: -self.origin.x,
                    ty: -self.origin.y,
                },
            );
            t.set_level(wid, level);
            t.set_sub_level(wid, sub_level);
            t.order(wid, order_value(settings.order), self.target_wid);
            t.commit();

            // Step 10.
            let (set, clear) = border_window_tags(self.sticky);
            ffi::set_tags(cid, wid, set);
            ffi::clear_tags(cid, wid, clear);
        }

        // Step 11 (always, BQ15).
        if disabled_update {
            ffi::reenable_update(cid);
        }
    }

    /// `border_move` (BR-DRW-09): translate the existing bitmap, nothing else.
    pub(super) fn move_to_target(&mut self, settings: &BorderSettings) {
        let Some(bounds) = ffi::window_bounds(self.cid, self.target_wid) else {
            return;
        };
        let origin = move_origin(border_rect(bounds), settings.width);
        let origin = CGPoint::new(origin.x, origin.y);
        if self.wid != 0 {
            if let Some(t) = ffi::Transaction::new(self.cid) {
                t.move_with_group(self.wid, origin);
                t.commit();
            }
        }
        self.origin = origin;
    }

    /// `border_hide`: order the border window out.
    pub(super) fn hide(&self) {
        if self.wid == 0 {
            return;
        }
        if let Some(t) = ffi::Transaction::new(self.cid) {
            t.order(self.wid, 0, self.target_wid);
            t.commit();
        }
    }

    /// `border_unhide`: order it back in (no redraw, no reposition).
    pub(super) fn unhide(&self, settings: &BorderSettings) {
        if self.too_small || (!self.sticky && !space::is_space_visible(self.sid)) {
            return;
        }
        if self.wid == 0 {
            return;
        }
        if let Some(t) = ffi::Transaction::new(self.cid) {
            t.order(self.wid, order_value(settings.order), self.target_wid);
            t.commit();
        }
    }
}

impl Drop for Border {
    /// `border_destroy`: hide, release the context, the window and the own connection.
    fn drop(&mut self) {
        self.hide();
        self.context = None;
        if self.wid != 0 {
            ffi::release_window(self.cid, self.wid);
            self.wid = 0;
        }
        if self.own_cid {
            ffi::release_connection(self.cid);
        }
    }
}
