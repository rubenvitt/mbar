//! The shared SkyLight notification plumbing (`docs/superpowers/specs/2026-10-09-borders-design.md`,
//! "Platform"; `docs/spec/borders.md` §5.1, BR-EV-03).
//!
//! * **One notify proc.** Every SkyLight notify id is registered once
//!   (`SLSRegisterNotifyProc`), with one proc that fans each event out to every consumer
//!   ([`super::spaces`], [`super::borders`]). Ids both need (815, 816, 1322, 1325, 1326,
//!   1401, 1508) are therefore never registered twice, and both handlers see every event;
//!   each ignores the ids it does not use. Note that 1322 means capture gating to spaces
//!   and a focus trigger to borders: both run.
//! * **One window-notification set.** `SLSRequestNotificationsForWindows` replaces the
//!   whole set of windows the connection gets per-window events (804–816) for. Every owner
//!   publishes its own set with [`request_windows`]; SkyLight receives the union.
//!
//! The procs are called on the main thread (the main run loop drains SkyLight's event
//! port); nothing here unwinds into SkyLight.

use super::skylight as sls;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::Mutex;

/// Who asked for per-window notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    /// `space_windows_change` tracking ([`super::spaces`]).
    Spaces,
    /// Window borders ([`super::borders`]).
    Borders,
}

#[derive(Default)]
struct State {
    registered: HashSet<u32>,
    spaces: Vec<u32>,
    borders: Vec<u32>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(State::default))
}

/// Registers the shared notify proc for every id in `ids` that is not registered yet.
/// Returns `false` when SkyLight lacks `SLSRegisterNotifyProc`.
pub(crate) fn register(ids: &[u32]) -> bool {
    let Some(reg) = sls::SLSRegisterNotifyProc() else {
        log::warn!("SLSRegisterNotifyProc unavailable");
        return false;
    };
    let todo: Vec<u32> = with_state(|s| {
        ids.iter()
            .copied()
            .filter(|id| s.registered.insert(*id))
            .collect()
    });
    for id in todo {
        // SAFETY: `notify_proc` matches SkyLight's handler signature and lives forever.
        let err = unsafe { reg(notify_proc, id, std::ptr::null_mut()) };
        if err != 0 {
            log::warn!("SLSRegisterNotifyProc({id}) failed: {err}");
        }
    }
    true
}

/// Union of the owners' window sets, in owner order, without duplicates and without 0.
fn union(spaces: &[u32], borders: &[u32]) -> Vec<u32> {
    let mut seen = HashSet::new();
    spaces
        .iter()
        .chain(borders.iter())
        .copied()
        .filter(|w| *w != 0 && seen.insert(*w))
        .collect()
}

/// Replaces `owner`'s window set and asks SkyLight for per-window notifications of the
/// union of all owners' sets (`SLSRequestNotificationsForWindows` on the main connection,
/// called on every request, as both consumers did before sharing it).
pub(crate) fn request_windows(owner: Owner, wids: &[u32]) {
    let all = with_state(|s| {
        match owner {
            Owner::Spaces => s.spaces = wids.to_vec(),
            Owner::Borders => s.borders = wids.to_vec(),
        }
        union(&s.spaces, &s.borders)
    });
    if let Some(f) = sls::SLSRequestNotificationsForWindows() {
        // SAFETY: pointer/count describe a valid u32 slice that outlives the call.
        unsafe { f(sls::cid(), all.as_ptr(), all.len() as i32) };
    }
}

unsafe extern "C" fn notify_proc(event: u32, data: *mut c_void, len: usize, _ctx: *mut c_void) {
    let data = data as *const u8;
    // Never unwind into SkyLight; one consumer's panic must not starve the other.
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `data` points to `len` bytes provided by SkyLight for this event.
        unsafe { super::spaces::handle_notify(event, data, len) }
    });
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: as above.
        unsafe { super::borders::handle_notify(event, data, len) }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_keeps_owner_order_and_drops_duplicates() {
        assert_eq!(union(&[3, 1, 0], &[1, 7, 3, 9]), vec![3, 1, 7, 9]);
        assert_eq!(union(&[], &[5]), vec![5]);
        assert!(union(&[], &[]).is_empty());
    }
}
