//! Window borders: JankyBorders inside mbar (`docs/spec/borders.md`, design in
//! `docs/superpowers/specs/2026-10-09-borders-design.md`, "Platform").
//!
//! The core sends the configuration ([`BordersUpdate`], `PlatformRequest::SetBorders`);
//! this module owns the live table `target wid → Border` and keeps one SkyLight window per
//! suitable window in sync with it. Everything runs on the **main thread**: the public
//! entry points are called by `platform::services`, window events arrive through the
//! shared SkyLight notify proc ([`super::notify`]) and are applied directly in
//! [`handle_notify`], never through the Runtime.
//!
//! * [`ffi`]: the private symbols (missing required symbol → borders stay off, one warning).
//! * [`border`]: create / update / move / hide / destroy of one border window (§7).
//! * [`draw`]: CoreGraphics drawing of a [`mbar_core::borders::DrawPlan`] (§7.4).
//! * [`focus`]: focused-window detection (§6) and coalesced delayed re-checks (BQ25).
//!
//! Deviations (besides those in [`border`]): focus detection runs once after the borders
//! are turned on and after every recreate (BQ1: JankyBorders leaves every border inactive
//! until the next focus event); `ax_focus=on` without the Accessibility permission logs a
//! warning and uses the SkyLight path (BQ13: JankyBorders exits); `SLSGetEventPort` is not
//! drained (AppKit owns it); no yabai proxy integration (§10); displays reconfigured and
//! wake recreate every border.

mod border;
mod draw;
mod ffi;
mod focus;

use super::notify::{self, Owner};
use border::Border;
use focus::Delayed;
use mbar_core::borders::{corner_radius, BorderSettings, BordersUpdate, UpdateMask};
use objc2::MainThreadMarker;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// SkyLight notify ids the borders handle (spec §5.1, JankyBorders `src/events.h`).
mod event {
    pub const WINDOW_UPDATE: u32 = 723;
    pub const WINDOW_CLOSE: u32 = 804;
    pub const WINDOW_MOVE: u32 = 806;
    pub const WINDOW_RESIZE: u32 = 807;
    pub const WINDOW_REORDER: u32 = 808;
    pub const WINDOW_LEVEL: u32 = 811;
    pub const WINDOW_UNHIDE: u32 = 815;
    pub const WINDOW_HIDE: u32 = 816;
    pub const WINDOW_TITLE: u32 = 1322;
    pub const WINDOW_CREATE: u32 = 1325;
    pub const WINDOW_DESTROY: u32 = 1326;
    pub const SPACE_CHANGE: u32 = 1401;
    pub const FRONT_CHANGE: u32 = 1508;

    pub const ALL: [u32; 13] = [
        WINDOW_UPDATE,
        WINDOW_CLOSE,
        WINDOW_MOVE,
        WINDOW_RESIZE,
        WINDOW_REORDER,
        WINDOW_LEVEL,
        WINDOW_UNHIDE,
        WINDOW_HIDE,
        WINDOW_TITLE,
        WINDOW_CREATE,
        WINDOW_DESTROY,
        SPACE_CHANGE,
        FRONT_CHANGE,
    ];
}

/// Delays of JankyBorders' `DELAY_ASYNC_EXEC_ON_MAIN_THREAD` (BR-EV-02).
const FOCUS_DELAY: Duration = Duration::from_millis(50);
const REORDER_FOCUS_DELAY: Duration = Duration::from_millis(10);
const SPACE_DELAY: Duration = Duration::from_millis(20);
/// Displays reconfigured / wake: let WindowServer settle, coalesce the callback burst.
const RECREATE_DELAY: Duration = Duration::from_millis(100);

/// Space and window-tag helpers shared by the manager and [`border`].
mod space {
    use super::ffi;
    use crate::sys::{displays, spaces};

    /// The current space of every managed display (`SLSCopyManagedDisplays` +
    /// `SLSManagedDisplayGetCurrentSpace`; no displays → empty, BQ31).
    pub(super) fn current_spaces() -> Vec<u64> {
        displays::managed_display_uuids()
            .iter()
            .map(|uuid| spaces::current_space(uuid))
            .collect()
    }

    /// `is_space_visible` (BR-WIN-09).
    pub(super) fn is_space_visible(sid: u64) -> bool {
        current_spaces().contains(&sid)
    }

    /// `window_space_id` (BR-WIN-08).
    pub(super) fn window_space_id(wid: u32) -> u64 {
        let cid = ffi::main_cid();
        let sid = ffi::first_space_of_window(cid, wid);
        if sid != 0 {
            return sid;
        }
        ffi::managed_display_for_window(cid, wid)
            .map(|uuid| spaces::current_space(&uuid))
            .unwrap_or(0)
    }

    /// `window_tags` and `window_level` of one window from a single query (0, 0 if the
    /// query fails).
    pub(super) fn window_tags_and_level(cid: i32, wid: u32) -> (u64, i32) {
        let list = ffi::wid_array(wid);
        let mut out = (0, 0);
        ffi::query_windows(cid, list.as_opaque(), |it| {
            out = (it.tags(), it.level());
            false
        });
        out
    }
}

/// The BSD process name (`proc_name`, BR-WIN-03 step 2); empty when it fails (BQ19).
fn process_name(pid: i32) -> String {
    let mut buf = [0u8; 4096]; // PROC_PIDPATHINFO_MAXSIZE
                               // SAFETY: `buf` is writable for its full length.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return String::new();
    }
    std::ffi::CStr::from_bytes_until_nul(&buf)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `app_allowed` (BR-WIN-03 step 3): whitelist first, then blacklist; exact,
/// case-sensitive. Same rule as the core's `BorderSettings::admits`.
fn app_allowed(settings: &BorderSettings, name: &str) -> bool {
    if !settings.whitelist.is_empty() && !settings.whitelist.iter().any(|n| n == name) {
        return false;
    }
    !(!settings.blacklist.is_empty() && settings.blacklist.iter().any(|n| n == name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    Focus,
    Spaces,
    Recreate,
}

struct Manager {
    settings: BorderSettings,
    overrides: HashMap<u32, BorderSettings>,
    /// target wid → border (ordered, so updates and the notification list are stable).
    borders: BTreeMap<u32, Border>,
    pid: i32,
    macos26: bool,
    /// Identifies this manager in delayed jobs (a job of a previous manager is dropped).
    generation: u64,
    focus: Delayed,
    spaces: Delayed,
    recreate: Delayed,
    ax_warned: bool,
}

thread_local! {
    static MANAGER: RefCell<Option<Manager>> = const { RefCell::new(None) };
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

/// Runs `f` on the live manager. `None` when borders are off, off the main thread, or
/// when the manager is already borrowed (a re-entrant callback is dropped, never panics).
fn with_manager<R>(f: impl FnOnce(&mut Manager) -> R) -> Option<R> {
    MainThreadMarker::new()?;
    MANAGER.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => slot.as_mut().map(f),
        Err(_) => {
            log::debug!("borders: re-entrant call dropped");
            None
        }
    })
}

fn overrides_map(list: &[(u32, BorderSettings)]) -> HashMap<u32, BorderSettings> {
    list.iter().cloned().collect()
}

/// Applies a configuration from the core (`PlatformRequest::SetBorders`). Turning borders
/// on scans every window (§5.3); `drawing=off` destroys every border window.
pub fn configure(update: BordersUpdate) {
    if MainThreadMarker::new().is_none() {
        log::warn!("borders::configure called off the main thread");
        return;
    }
    let installed = MANAGER.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            log::warn!("borders: configuration dropped (re-entrant call)");
            return false;
        };
        if !update.drawing {
            // Dropping the manager destroys every border window.
            *slot = None;
            return false;
        }
        match slot.as_mut() {
            Some(m) => {
                m.reconfigure(update);
                false
            }
            None => {
                if !ffi::available() {
                    return false;
                }
                if !notify::register(&event::ALL) {
                    return false;
                }
                *slot = Some(Manager::new(update));
                true
            }
        }
    });
    if installed {
        with_manager(|m| {
            m.add_existing_windows();
            // Deviation from BQ1: determine the focused window right away.
            m.determine_and_focus();
        });
    }
}

/// Destroys every border window (daemon exit).
pub fn shutdown() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    MANAGER.with(|cell| {
        if let Ok(mut slot) = cell.try_borrow_mut() {
            *slot = None;
        }
    });
}

/// Displays were reconfigured: recreate every border (coalesced).
pub fn on_displays_changed() {
    with_manager(|m| m.schedule(Job::Recreate, RECREATE_DELAY));
}

/// The system woke: recreate every border (coalesced).
pub fn on_system_woke() {
    with_manager(|m| m.schedule(Job::Recreate, RECREATE_DELAY));
}

/// The active space changed (NSWorkspace / SkyLight 1327/1328): the same consistency pass
/// as SLS 1401 (coalesced with it), in case 1401 does not fire on this macOS.
pub fn on_space_changed() {
    with_manager(|m| m.schedule(Job::Spaces, SPACE_DELAY));
}

/// The front application changed (NSWorkspace): re-check focus like SLS 1508.
pub fn on_front_app_switched() {
    with_manager(|m| m.schedule(Job::Focus, FOCUS_DELAY));
}

/// Borders' part of the shared SkyLight notify proc ([`super::notify`]).
///
/// # Safety
/// `data` must be null or point to `len` readable bytes (the SkyLight payload).
pub(crate) unsafe fn handle_notify(event: u32, data: *const u8, len: usize) {
    if !event::ALL.contains(&event) {
        return;
    }
    let payload = match event {
        event::WINDOW_CREATE | event::WINDOW_DESTROY => {
            if data.is_null() || len < 12 {
                return;
            }
            // SAFETY: payload is `{ u64 sid; u32 wid; }` (≥ 12 bytes, checked above).
            unsafe {
                (
                    std::ptr::read_unaligned(data as *const u64),
                    std::ptr::read_unaligned(data.add(8) as *const u32),
                )
            }
        }
        event::SPACE_CHANGE | event::FRONT_CHANGE => (0, 0),
        _ => {
            if data.is_null() || len < 4 {
                return;
            }
            // SAFETY: payload starts with a `u32` window id (≥ 4 bytes, checked above).
            (0, unsafe { std::ptr::read_unaligned(data as *const u32) })
        }
    };
    with_manager(|m| m.on_event(event, payload.0, payload.1));
}

fn dispatch(job: Job, delay: Duration, generation: u64) {
    let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
    let queued = dispatch2::DispatchQueue::main().after(when, move || {
        // Never unwind into libdispatch.
        let _ = std::panic::catch_unwind(|| run_job(job, generation));
    });
    if queued.is_err() {
        log::debug!("borders: could not schedule {job:?}");
    }
}

fn run_job(job: Job, generation: u64) {
    with_manager(|m| {
        if m.generation != generation {
            return;
        }
        let now = Instant::now();
        let again = m.slot(job).fired(now);
        match job {
            Job::Focus => m.determine_and_focus(),
            Job::Spaces => m.draw_borders_on_current_spaces(),
            Job::Recreate => {
                m.recreate_all();
                m.determine_and_focus();
            }
        }
        if let Some(delay) = again {
            dispatch(job, delay, m.generation);
        }
    });
}

impl Manager {
    fn new(update: BordersUpdate) -> Manager {
        Manager {
            settings: update.settings,
            overrides: overrides_map(&update.overrides),
            borders: BTreeMap::new(),
            pid: std::process::id() as i32,
            macos26: crate::sys::util::os_at_least(26, 0),
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            focus: Delayed::default(),
            spaces: Delayed::default(),
            recreate: Delayed::default(),
            ax_warned: false,
        }
    }

    fn slot(&mut self, job: Job) -> &mut Delayed {
        match job {
            Job::Focus => &mut self.focus,
            Job::Spaces => &mut self.spaces,
            Job::Recreate => &mut self.recreate,
        }
    }

    fn schedule(&mut self, job: Job, delay: Duration) {
        if let Some(delay) = self.slot(job).request(Instant::now(), delay) {
            dispatch(job, delay, self.generation);
        }
    }

    fn effective<'a>(
        settings: &'a BorderSettings,
        overrides: &'a HashMap<u32, BorderSettings>,
        wid: u32,
    ) -> &'a BorderSettings {
        overrides.get(&wid).unwrap_or(settings)
    }

    /// A new configuration while borders are on (BR-IPC-07 step 5). `RECREATE_ALL`
    /// recreates every border; otherwise every border whose effective settings changed is
    /// redrawn (this covers JankyBorders' ALL / ACTIVE / INACTIVE bulk redraws and the
    /// `apply-to` overrides; a border that did not change is left alone).
    fn reconfigure(&mut self, update: BordersUpdate) {
        let old_settings = std::mem::replace(&mut self.settings, update.settings);
        let old_overrides =
            std::mem::replace(&mut self.overrides, overrides_map(&update.overrides));
        if update.mask.intersects(UpdateMask::RECREATE_ALL) {
            self.recreate_all();
            // Deviation from BQ1: determine the focused window right away.
            self.determine_and_focus();
            return;
        }
        let Manager {
            settings,
            overrides,
            borders,
            macos26,
            ..
        } = self;
        for (wid, border) in borders.iter_mut() {
            let new = Self::effective(settings, overrides, *wid);
            let old = Self::effective(&old_settings, &old_overrides, *wid);
            if new != old {
                border.needs_redraw = true;
                border.update(new, *macos26);
            }
        }
    }

    fn update_border(&mut self, wid: u32) {
        let Manager {
            settings,
            overrides,
            borders,
            macos26,
            ..
        } = self;
        if let Some(border) = borders.get_mut(&wid) {
            border.update(Self::effective(settings, overrides, wid), *macos26);
        }
    }

    /// `windows_update_notifications` (BR-EV-03): our share of the shared window set.
    fn update_notifications(&self) {
        let wids: Vec<u32> = self.borders.keys().copied().collect();
        notify::request_windows(Owner::Borders, &wids);
    }

    fn on_event(&mut self, event: u32, sid: u64, wid: u32) {
        match event {
            event::WINDOW_CREATE | event::WINDOW_DESTROY => {
                // BR-WIN-04.
                if wid == 0 || sid == 0 || self.is_own_window(wid) {
                    return;
                }
                if event == event::WINDOW_CREATE {
                    if self.window_create(wid, sid) {
                        self.determine_and_focus();
                    }
                } else {
                    self.window_destroy(wid, sid);
                    self.determine_and_focus();
                }
            }
            event::SPACE_CHANGE => self.schedule(Job::Spaces, SPACE_DELAY),
            event::FRONT_CHANGE => self.schedule(Job::Focus, FOCUS_DELAY),
            event::WINDOW_UPDATE | event::WINDOW_TITLE => {
                if !self.is_own_window(wid) {
                    self.schedule(Job::Focus, FOCUS_DELAY);
                }
            }
            event::WINDOW_CLOSE => {
                if self.borders.contains_key(&wid) {
                    self.window_destroy(wid, 0);
                }
            }
            event::WINDOW_MOVE => {
                let Manager {
                    settings,
                    overrides,
                    borders,
                    ..
                } = self;
                if let Some(border) = borders.get_mut(&wid) {
                    border.move_to_target(Self::effective(settings, overrides, wid));
                }
            }
            event::WINDOW_RESIZE | event::WINDOW_LEVEL => self.update_border(wid),
            event::WINDOW_REORDER => {
                // Only a tracked window can be updated; own windows are never tracked.
                if self.borders.contains_key(&wid) {
                    self.update_border(wid);
                    self.schedule(Job::Focus, REORDER_FOCUS_DELAY);
                } else if !self.is_own_window(wid) {
                    self.schedule(Job::Focus, REORDER_FOCUS_DELAY);
                }
            }
            event::WINDOW_HIDE => {
                if let Some(border) = self.borders.get(&wid) {
                    border.hide();
                }
            }
            event::WINDOW_UNHIDE => {
                if let Some(border) = self.borders.get(&wid) {
                    border.unhide(Self::effective(&self.settings, &self.overrides, wid));
                }
            }
            _ => {}
        }
    }

    /// BR-EV-01: windows of our own process (bar panels, border windows).
    fn is_own_window(&self, wid: u32) -> bool {
        ffi::window_owner_pid(wid) == self.pid
    }

    /// `windows_window_create` (BR-WIN-03). Returns whether a new border was created.
    fn window_create(&mut self, wid: u32, sid: u64) -> bool {
        let pid = ffi::window_owner_pid(wid);
        if pid == self.pid || !app_allowed(&self.settings, &process_name(pid)) {
            return false;
        }
        let list = ffi::wid_array(wid);
        let mut found: Option<Option<i32>> = None;
        ffi::query_windows(ffi::main_cid(), list.as_opaque(), |it| {
            if it.suitable() {
                found = Some(it.corner_radius());
            }
            false
        });
        let Some(reported) = found else {
            return false;
        };
        let mut created = false;
        let border = self.borders.entry(wid).or_insert_with(|| {
            created = true;
            Border::new(wid)
        });
        border.radius = corner_radius(reported);
        border.target_wid = wid;
        border.sid = sid;
        self.update_border(wid);
        self.update_notifications();
        created
    }

    /// `windows_window_destroy` (BR-WIN-05, space ids compared as u64).
    fn window_destroy(&mut self, wid: u32, sid: u64) -> bool {
        let matches = self
            .borders
            .get(&wid)
            .is_some_and(|b| b.sid == sid || b.sticky || sid == 0);
        if !matches {
            return false;
        }
        // Dropping the border hides and releases its window and connection.
        self.borders.remove(&wid);
        self.update_notifications();
        true
    }

    /// `windows_add_existing_windows` (BR-WIN-02): every suitable window on every space.
    fn add_existing_windows(&mut self) {
        let sids: Vec<u64> = crate::sys::spaces::managed_display_spaces()
            .into_iter()
            .flat_map(|d| d.spaces)
            .collect();
        for wid in self.suitable_windows_on(&sids) {
            let sid = space::window_space_id(wid);
            self.window_create(wid, sid);
        }
        self.update_notifications();
    }

    fn suitable_windows_on(&self, sids: &[u64]) -> Vec<u32> {
        let cid = ffi::main_cid();
        let Some(list) = ffi::windows_on_spaces(cid, 0, sids) else {
            return Vec::new();
        };
        let mut wids = Vec::new();
        ffi::query_windows(cid, &list, |it| {
            if it.suitable() {
                wids.push(it.wid());
            }
            true
        });
        wids
    }

    /// `windows_recreate_all_borders` (BR-WIN-06).
    fn recreate_all(&mut self) {
        self.borders.clear();
        self.update_notifications();
        self.add_existing_windows();
    }

    /// `windows_draw_borders_on_current_spaces` (BR-WIN-07). For a tracked, non-sticky
    /// window the space id is refreshed first, so a window that changed spaces without a
    /// create event still gets its border (and the border window follows it, see
    /// [`border`]; spec §14 open question 3).
    fn draw_borders_on_current_spaces(&mut self) {
        let sids = space::current_spaces();
        if sids.is_empty() {
            return;
        }
        for wid in self.suitable_windows_on(&sids) {
            match self.borders.get_mut(&wid) {
                Some(border) => {
                    if !border.sticky {
                        let sid = space::window_space_id(wid);
                        if sid != 0 {
                            border.sid = sid;
                        }
                    }
                    self.update_border(wid);
                }
                None => {
                    let sid = space::window_space_id(wid);
                    self.window_create(wid, sid);
                }
            }
        }
    }

    /// The front window by the configured path (BR-FOC-02 step 1). `ax_focus` auto means
    /// "AX when trusted"; `ax_focus=on` without trust warns once and uses SkyLight.
    fn front_window(&mut self) -> u32 {
        let use_ax = match self.settings.ax_focus {
            Some(false) => false,
            Some(true) => {
                let trusted = crate::sys::menus::is_trusted(false);
                if !trusted && !self.ax_warned {
                    self.ax_warned = true;
                    log::warn!(
                        "borders: ax_focus=on needs the Accessibility permission; \
                         using the SkyLight focus path"
                    );
                }
                trusted
            }
            None => crate::sys::menus::is_trusted(false),
        };
        if use_ax && ffi::has_ax_window() {
            focus::ax_front_window()
        } else {
            focus::skylight_front_window()
        }
    }

    /// `windows_window_focus` (BR-FOC-01): returns whether a border targets `front`.
    fn apply_focus(&mut self, front: u32) -> bool {
        let Manager {
            settings,
            overrides,
            borders,
            macos26,
            ..
        } = self;
        let mut found = false;
        for (wid, border) in borders.iter_mut() {
            let target = border.target_wid == front;
            if border.focused != target {
                border.focused = target;
                border.needs_redraw = true;
                border.update(Self::effective(settings, overrides, *wid), *macos26);
            }
            found |= target;
        }
        found
    }

    /// `windows_determine_and_focus_active_window` (BR-FOC-02).
    fn determine_and_focus(&mut self) {
        let front = self.front_window();
        if !self.apply_focus(front) && front != 0 {
            let sid = space::window_space_id(front);
            if self.window_create(front, sid) {
                self.apply_focus(front);
            }
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        // Every Border hides and releases its window and connection on drop.
        self.borders.clear();
        notify::request_windows(Owner::Borders, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_filter_by_exact_name() {
        let mut s = BorderSettings::default();
        assert!(app_allowed(&s, "Safari"));
        s.blacklist = vec!["Safari".into()];
        assert!(!app_allowed(&s, "Safari"));
        assert!(app_allowed(&s, "safari"));
        s.whitelist = vec!["kitty".into()];
        assert!(!app_allowed(&s, "Mail"));
        assert!(app_allowed(&s, "kitty"));
        s.blacklist = vec!["kitty".into()];
        assert!(!app_allowed(&s, "kitty"));
    }

    #[test]
    fn notify_ids_are_unique() {
        let mut ids = event::ALL.to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), event::ALL.len());
    }
}
