//! Focused-window detection (`docs/spec/borders.md` §6.3, §6.4; JankyBorders
//! `src/misc/window.h:get_front_window`, `src/misc/space.h:get_active_space_id`,
//! `src/misc/ax.h:ax_get_front_window`), and the coalesced delayed re-checks (BQ25).

use super::ffi;
use crate::sys::util::{self, owned};
use crate::sys::{displays, skylight as sls, spaces};
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFRetained, CFString, CFType};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

/// Messaging timeout for the AX focus query (seconds): a hung front app must not stall
/// the main thread for AX' default 6 s (JankyBorders keeps the default).
const AX_TIMEOUT: f32 = 1.0;

/// `get_active_space_id`: the current space of the only display, or of the display that
/// shows the active menu bar.
fn active_space_id() -> u64 {
    let ids = displays::active_display_ids();
    let uuid = if ids.len() == 1 {
        displays::display_uuid(ids[0])
    } else {
        sls::SLSCopyActiveMenuBarDisplayIdentifier().and_then(|f| {
            // SAFETY: valid connection id; Copy rule.
            let s: Option<CFRetained<CFString>> = unsafe { owned(f(ffi::main_cid())) };
            s.map(|s| s.to_string())
        })
    };
    match uuid {
        Some(uuid) => spaces::current_space(&uuid),
        None => {
            log::debug!("borders: no active display");
            0
        }
    }
}

/// `get_front_window` (BR-FOC-03): the first suitable window of the front process on the
/// active space, front to back; 0 if none.
pub(super) fn skylight_front_window() -> u32 {
    let cid = ffi::main_cid();
    let sid = active_space_id();
    let Some(target) = ffi::front_process_cid(cid) else {
        return 0;
    };
    let Some(list) = ffi::windows_on_spaces(cid, target, &[sid]) else {
        return 0;
    };
    if list.is_empty() {
        return 0;
    }
    let mut wid = 0;
    ffi::query_windows(cid, &list, |it| {
        if it.suitable() {
            wid = it.wid();
            false
        } else {
            true
        }
    });
    wid
}

/// `ax_get_front_window` (BR-FOC-04) without the trust check (the caller decides): the
/// `AXFocusedWindow` of the front process; 0 if none.
pub(super) fn ax_front_window() -> u32 {
    let cid = ffi::main_cid();
    let Some(target) = ffi::front_process_cid(cid) else {
        return 0;
    };
    let pid = ffi::connection_pid(target);
    if pid <= 0 {
        return 0;
    }
    // SAFETY: any pid is accepted; invalid ones fail on use.
    let app = unsafe { AXUIElement::new_application(pid) };
    // SAFETY: plain value argument.
    unsafe { app.set_messaging_timeout(AX_TIMEOUT) };
    let mut out: *const CFType = std::ptr::null();
    let attribute = util::cfstr("AXFocusedWindow");
    // SAFETY: valid attribute string and out pointer; the result follows the Copy rule.
    let err = unsafe { app.copy_attribute_value(&attribute, NonNull::from(&mut out)) };
    // SAFETY: on success `out` is an owned CF object (or null); released either way.
    let window: Option<CFRetained<CFType>> = unsafe { owned(out) };
    if err != AXError::Success {
        return 0;
    }
    window
        .and_then(util::downcast::<AXUIElement>)
        .and_then(|w| ffi::ax_window_id(&w))
        .unwrap_or(0)
}

/// One coalesced delayed job (BQ25): at most one dispatch is outstanding; a request whose
/// latency floor ends after the pending run is remembered and re-armed when it fires, so
/// every trigger is still followed by a run at least its delay later, and a burst of
/// triggers costs one run per delay instead of one per trigger.
#[derive(Debug, Default)]
pub(super) struct Delayed {
    due: Option<Instant>,
    again: Option<Instant>,
}

impl Delayed {
    /// Requests a run `delay` from `now`; returns the delay to dispatch with, or `None` when
    /// the pending dispatch (or the remembered re-arm) covers it.
    pub(super) fn request(&mut self, now: Instant, delay: Duration) -> Option<Duration> {
        let want = now + delay;
        match self.due {
            None => {
                self.due = Some(want);
                Some(delay)
            }
            Some(due) if due >= want => None,
            Some(_) => {
                self.again = Some(self.again.map_or(want, |a| a.max(want)));
                None
            }
        }
    }

    /// The pending dispatch fired (the job runs right after this). Returns the delay of
    /// the follow-up dispatch when a later request is still uncovered.
    pub(super) fn fired(&mut self, now: Instant) -> Option<Duration> {
        self.due = None;
        let again = self.again.take()?;
        if again <= now {
            return None;
        }
        self.due = Some(again);
        Some(again - now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_coalesces_and_keeps_the_floor() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut d = Delayed::default();
        assert_eq!(d.request(t0, ms(50)), Some(ms(50)));
        // Covered by the pending run.
        assert_eq!(d.request(t0 + ms(10), ms(10)), None);
        // Ends later: remembered.
        assert_eq!(d.request(t0 + ms(30), ms(50)), None);
        assert_eq!(d.request(t0 + ms(40), ms(50)), None);
        // Fires at 50: re-armed for 90.
        assert_eq!(d.fired(t0 + ms(50)), Some(ms(40)));
        assert_eq!(d.fired(t0 + ms(90)), None);
        // Nothing pending: a new request dispatches again.
        assert_eq!(d.request(t0 + ms(100), ms(20)), Some(ms(20)));
        // A late fire covers a remembered request whose floor already passed.
        assert_eq!(d.request(t0 + ms(110), ms(20)), None);
        assert_eq!(d.fired(t0 + ms(140)), None);
    }
}
