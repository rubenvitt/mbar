//! Mission Control spaces via the private SkyLight API (`docs/spec/bar.md` §6.4, §7;
//! `docs/spec/events.md` §5.2, §5.11): spaces per display, mission-control indices,
//! `space_change` / `space_windows_change` INFO payloads, per-space window tracking, space
//! capture (`space.<n>` images) and the SkyLight notify procs (including alias capture
//! gating).

use super::skylight as sls;
use super::util::{self, owned};
use super::{Sink, SysEvent};
use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::{CFArray, CFNumber, CFRetained, CFString, CFType};
use objc2_core_graphics::CGImage;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// SkyLight space type of a native fullscreen space.
pub const SPACE_TYPE_FULLSCREEN: i32 = 4;

/// SkyLight notify ids (`bar.md` §10).
pub mod notify {
    pub const WINDOW_UNHIDDEN: u32 = 815;
    pub const WINDOW_HIDDEN: u32 = 816;
    pub const CAPTURE_ENABLE_904: u32 = 904;
    pub const CAPTURE_DISABLE_INDEFINITELY: u32 = 905;
    pub const CAPTURE_DISABLE_TEMPORARILY: u32 = 1322;
    pub const WINDOW_CREATED: u32 = 1325;
    pub const WINDOW_DESTROYED: u32 = 1326;
    pub const SPACE_CHANGED_1327: u32 = 1327;
    pub const SPACE_CHANGED_1328: u32 = 1328;
    pub const SPACES_REFRESH_1401: u32 = 1401;
    pub const CAPTURE_ENABLE_1508: u32 = 1508;
}

/// The spaces of one managed display, in `SLSCopyManagedDisplaySpaces` order.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplaySpaces {
    /// `"Display Identifier"` (a UUID, or `"Main"` when separate spaces are off).
    pub display: String,
    /// `"Spaces"[i]["id64"]`.
    pub spaces: Vec<u64>,
}

/// One space with its derived ids.
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceEntry {
    /// SkyLight space id (`dsid`).
    pub id: u64,
    /// Mission-control index (1-based over all displays, fullscreen spaces included).
    pub index: u32,
    /// Display identifier owning the space.
    pub display: String,
    /// Arrangement id of that display (0 when unknown).
    pub adid: u32,
    /// `SLSSpaceGetType == 4`.
    pub fullscreen: bool,
}

/// `SLSCopyManagedDisplaySpaces(cid)` reduced to ids.
pub fn managed_display_spaces() -> Vec<DisplaySpaces> {
    let Some(f) = sls::SLSCopyManagedDisplaySpaces() else {
        return Vec::new();
    };
    // SAFETY: valid connection id; Copy rule.
    let Some(arr): Option<CFRetained<CFArray>> = (unsafe { owned(f(sls::cid())) }) else {
        return Vec::new();
    };
    util::array_items(&arr)
        .into_iter()
        .filter_map(util::downcast::<objc2_core_foundation::CFDictionary>)
        .map(|d| {
            let display = util::dict_string(&d, "Display Identifier").unwrap_or_default();
            let spaces = util::dict_array(&d, "Spaces")
                .map(|a| {
                    util::array_items(&a)
                        .into_iter()
                        .filter_map(util::downcast::<objc2_core_foundation::CFDictionary>)
                        .filter_map(|s| util::dict_i64(&s, "id64").map(|v| v as u64))
                        .collect()
                })
                .unwrap_or_default();
            DisplaySpaces { display, spaces }
        })
        .collect()
}

/// `mission_control_index(dsid)`: 1-based position of `dsid` over all displays' spaces, 0
/// if not found.
pub fn mission_control_index_in(list: &[DisplaySpaces], dsid: u64) -> u32 {
    list.iter()
        .flat_map(|d| d.spaces.iter())
        .position(|s| *s == dsid)
        .map(|i| i as u32 + 1)
        .unwrap_or(0)
}

/// The `n`-th space overall (1-based), 0 if out of range (`dsid_from_sid`).
pub fn space_at_index_in(list: &[DisplaySpaces], n: u32) -> u64 {
    if n == 0 {
        return 0;
    }
    list.iter()
        .flat_map(|d| d.spaces.iter())
        .nth(n as usize - 1)
        .copied()
        .unwrap_or(0)
}

/// [`mission_control_index_in`] on the live list.
pub fn mission_control_index(dsid: u64) -> u32 {
    mission_control_index_in(&managed_display_spaces(), dsid)
}

/// [`space_at_index_in`] on the live list.
pub fn space_at_index(n: u32) -> u64 {
    space_at_index_in(&managed_display_spaces(), n)
}

/// `SLSManagedDisplayGetCurrentSpace(cid, uuid)`.
pub fn current_space(display_uuid: &str) -> u64 {
    let Some(f) = sls::SLSManagedDisplayGetCurrentSpace() else {
        return 0;
    };
    let s = util::cfstr(display_uuid);
    // SAFETY: valid connection id and CFString.
    unsafe { f(sls::cid(), CFRetained::as_ptr(&s).as_ptr()) }
}

/// `SLSGetActiveSpace(cid)` (space of the active display).
pub fn active_space() -> u64 {
    match sls::SLSGetActiveSpace() {
        // SAFETY: valid connection id.
        Some(f) => unsafe { f(sls::cid()) },
        None => 0,
    }
}

/// `SLSSpaceGetType(cid, dsid)` (4 = fullscreen).
pub fn space_type(dsid: u64) -> i32 {
    match sls::SLSSpaceGetType() {
        // SAFETY: valid connection id; unknown ids return a default type.
        Some(f) => unsafe { f(sls::cid(), dsid) },
        None => 0,
    }
}

/// UUID of the display owning a space (`SLSCopyManagedDisplayForSpace`).
pub fn display_for_space(dsid: u64) -> Option<String> {
    let f = sls::SLSCopyManagedDisplayForSpace()?;
    // SAFETY: valid connection id; Copy rule.
    let s: CFRetained<CFString> = unsafe { owned(f(sls::cid(), dsid)) }?;
    Some(s.to_string())
}

/// `display_space_list(did)`: spaces of the display with this UUID.
pub fn spaces_of_display(display_uuid: &str) -> Vec<u64> {
    managed_display_spaces()
        .into_iter()
        .find(|d| d.display == display_uuid)
        .map(|d| d.spaces)
        .unwrap_or_default()
}

/// `display_id_for_space(n)`: CG display id owning the `n`-th space (0 if unknown).
pub fn display_id_for_space_index(n: u32) -> u32 {
    let dsid = space_at_index(n);
    if dsid == 0 {
        return 0;
    }
    display_for_space(dsid)
        .map(|u| super::displays::display_id_for_uuid(&u))
        .unwrap_or(0)
}

/// Every space in mission-control order with its display and arrangement id.
pub fn all_spaces() -> Vec<SpaceEntry> {
    let list = managed_display_spaces();
    let uuids = super::displays::managed_display_uuids();
    let single = super::displays::active_display_ids().len() == 1;
    let mut index = 0;
    let mut out = Vec::new();
    for d in &list {
        let adid = if single {
            1
        } else {
            super::displays::arrangement_index(&uuids, &d.display)
        };
        for &id in &d.spaces {
            index += 1;
            out.push(SpaceEntry {
                id,
                index,
                display: d.display.clone(),
                adid,
                fullscreen: space_type(id) == SPACE_TYPE_FULLSCREEN,
            });
        }
    }
    out
}

/// `space_change` INFO (`events.md` §5.2), exact bytes, no trailing newline:
/// `{\n\t"display-<adid>": <sid>,\n…}`; `"{\n}"` for no entries.
pub fn space_change_info(entries: &[(u32, u32)]) -> String {
    let mut s = String::from("{\n");
    for (i, (adid, sid)) in entries.iter().enumerate() {
        s.push_str(&format!("\t\"display-{adid}\": {sid}"));
        if i + 1 < entries.len() {
            s.push(',');
        }
        s.push('\n');
    }
    s.push('}');
    s
}

/// `(adid, mission-control index of the current space)` for every active display with a
/// managed UUID, in arrangement order.
pub fn current_display_spaces() -> Vec<(u32, u32)> {
    let list = managed_display_spaces();
    let uuids = super::displays::managed_display_uuids();
    let ids = super::displays::active_display_ids();
    let mut out: Vec<(u32, u32)> = ids
        .iter()
        .filter_map(|&did| {
            let uuid = super::displays::display_uuid(did)?;
            let adid = if ids.len() == 1 {
                1
            } else {
                super::displays::arrangement_index(&uuids, &uuid)
            };
            if adid == 0 {
                return None;
            }
            let dsid = current_space(&uuid);
            Some((adid, mission_control_index_in(&list, dsid)))
        })
        .collect();
    out.sort_by_key(|e| e.0);
    out
}

/// `space_windows_change` INFO (`events.md` §5.11): exact bytes incl. trailing newline;
/// names are not escaped (SketchyBar behaviour).
pub fn space_windows_info(space_index: u32, apps: &[(String, u32)]) -> String {
    let mut s = format!("{{\n\t\"space\": {space_index},\n\t\"apps\": {{\n");
    let body: Vec<String> = apps
        .iter()
        .map(|(name, count)| format!("\t\t\"{name}\": {count}"))
        .collect();
    s.push_str(&body.join(",\n"));
    s.push_str("\n\t}\n}\n");
    s
}

/// Groups window owner pids (in window order) into `(app name, count)`: grouped by pid in
/// first-seen order, pids without a name skipped, identical names merged into the first.
pub fn group_app_windows(
    pids: &[i32],
    name_of: impl Fn(i32) -> Option<String>,
) -> Vec<(String, u32)> {
    let mut by_pid: Vec<(i32, u32)> = Vec::new();
    for &pid in pids {
        match by_pid.iter_mut().find(|(p, _)| *p == pid) {
            Some(e) => e.1 += 1,
            None => by_pid.push((pid, 1)),
        }
    }
    let mut out: Vec<(String, u32)> = Vec::new();
    for (pid, count) in by_pid {
        let Some(name) = name_of(pid) else { continue };
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some(e) => e.1 += count,
            None => out.push((name, count)),
        }
    }
    out
}

/// `iterator_window_suitable` (`bar.md` §7).
pub fn window_suitable(parent_wid: u32, attributes: u64, tags: u64) -> bool {
    parent_wid == 0
        && ((attributes & 0x2) != 0 || (tags & 0x0400_0000_0000_0000) != 0)
        && ((tags & 0x1) != 0 || ((tags & 0x2) != 0 && (tags & 0x8000_0000) != 0))
}

fn localized_app_name(pid: i32) -> Option<String> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|a| a.localizedName())
        .map(|n| n.to_string())
}

fn owner_pid(wid: u32) -> Option<i32> {
    let get_owner = sls::SLSGetWindowOwner()?;
    let get_pid = sls::SLSConnectionGetPID()?;
    let mut owner_cid = 0;
    let mut pid: libc::pid_t = 0;
    // SAFETY: valid connection id and out pointers.
    unsafe {
        get_owner(sls::cid(), wid, &mut owner_cid);
        get_pid(owner_cid, &mut pid);
    }
    (pid != 0).then_some(pid)
}

/// Iterates a SkyLight window query over `windows` and returns the suitable window ids.
fn suitable_windows(windows: &CFArray) -> Vec<u32> {
    let (Some(query), Some(copy), Some(advance), Some(parent), Some(wid), Some(tags), Some(attrs)) = (
        sls::SLSWindowQueryWindows(),
        sls::SLSWindowQueryResultCopyWindows(),
        sls::SLSWindowIteratorAdvance(),
        sls::SLSWindowIteratorGetParentID(),
        sls::SLSWindowIteratorGetWindowID(),
        sls::SLSWindowIteratorGetTags(),
        sls::SLSWindowIteratorGetAttributes(),
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // SAFETY: SkyLight query objects are CF objects following the Create/Copy rule; the
    // iterator is only used while `iter` is alive.
    unsafe {
        let Some(q): Option<CFRetained<CFType>> = owned(query(sls::cid(), windows, 0)) else {
            return out;
        };
        let Some(iter): Option<CFRetained<CFType>> = owned(copy(CFRetained::as_ptr(&q).as_ptr()))
        else {
            return out;
        };
        let it = CFRetained::as_ptr(&iter).as_ptr() as *const CFType;
        while advance(it) {
            if window_suitable(parent(it), attrs(it), tags(it)) {
                out.push(wid(it));
            }
        }
    }
    out
}

/// Suitable windows on space `dsid` with their owner pid (`app_windows_update_space`).
pub fn windows_on_space(dsid: u64) -> Vec<(u32, i32)> {
    let Some(copy) = sls::SLSCopyWindowsWithOptionsAndTags() else {
        return Vec::new();
    };
    let num = CFNumber::new_i64(dsid as i64);
    let spaces = CFArray::from_retained_objects(&[num]);
    let mut set_tags: u64 = 1;
    let mut clear_tags: u64 = 0;
    // SAFETY: valid connection id, CFArray of CFNumber space ids and tag out pointers;
    // Copy rule for the result.
    let list: Option<CFRetained<CFArray>> = unsafe {
        owned(copy(
            sls::cid(),
            0,
            spaces.as_opaque(),
            0x2,
            &mut set_tags,
            &mut clear_tags,
        ))
    };
    let Some(list) = list else {
        return Vec::new();
    };
    if list.is_empty() {
        return Vec::new();
    }
    suitable_windows(&list)
        .into_iter()
        .filter_map(|wid| owner_pid(wid).map(|pid| (wid, pid)))
        .collect()
}

/// `app_window_suitable(wid)`: the predicate on a single-window query.
pub fn window_is_suitable(wid: u32) -> bool {
    let num = CFNumber::new_i64(wid as i64);
    let arr = CFArray::from_retained_objects(&[num]);
    !suitable_windows(arr.as_opaque()).is_empty()
}

/// `window_send_to_space` (`bar.md` §6.4, non-sticky bars on a space change):
/// `SLSMoveWindowsToManagedSpace(cid, [wid…] (SInt32), dsid)`. Returns `false` when
/// there is nothing to do or SkyLight lacks the symbol.
pub fn move_windows_to_space(wids: &[u32], dsid: u64) -> bool {
    if wids.is_empty() || dsid == 0 {
        return false;
    }
    let Some(f) = sls::SLSMoveWindowsToManagedSpace() else {
        return false;
    };
    let nums: Vec<CFRetained<CFNumber>> =
        wids.iter().map(|w| CFNumber::new_i32(*w as i32)).collect();
    let arr = CFArray::from_retained_objects(&nums);
    // SAFETY: valid connection id, a CFArray of CFNumber window ids owned by this process
    // and a plain space id; the call retains nothing we pass.
    unsafe { f(sls::cid(), arr.as_opaque(), dsid) };
    true
}

/// `space.<n>` image: `SLSHWCaptureSpace(cid, dsid_of(n), 0)[0]` (one-shot snapshot).
pub fn capture_space(n: u32) -> Option<CFRetained<CGImage>> {
    let dsid = space_at_index(n);
    if dsid == 0 {
        return None;
    }
    let f = sls::SLSHWCaptureSpace()?;
    // SAFETY: valid connection/space ids; Copy rule for the array.
    let arr: CFRetained<CFArray> = unsafe { owned(f(sls::cid() as i64, dsid as i64, 0)) }?;
    let first = util::array_items(&arr).into_iter().next()?;
    util::downcast::<CGImage>(first)
}

// ---------------------------------------------------------------------------------------
// SkyLight notifications
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
struct TrackedWindow {
    wid: u32,
    sid: u64,
    pid: i32,
}

#[derive(Default)]
struct NotifyState {
    registered: HashSet<u32>,
    space_sink: Option<Sink>,
    windows_sink: Option<Sink>,
    windows: Vec<TrackedWindow>,
    hidden: Vec<TrackedWindow>,
}

static STATE: Mutex<Option<NotifyState>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut NotifyState) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(NotifyState::default))
}

/// Capture gating (`window.c:window_capture`): 0 = enabled, -1 = disabled indefinitely,
/// otherwise the monotonic ns timestamp of the last 1322 event.
static CAPTURE_GATE: AtomicI64 = AtomicI64::new(0);

fn monotonic_ns() -> i64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    // +1 so that a timestamp is never 0 (= enabled).
    START.get_or_init(Instant::now).elapsed().as_nanos() as i64 + 1
}

/// Pure gating rule: `gate` as stored, `now` monotonic ns.
pub fn capture_disabled_at(gate: i64, now: i64) -> bool {
    match gate {
        0 => false,
        -1 => true,
        t => now - t <= (1i64 << 30),
    }
}

/// Whether alias capture is currently suspended by WindowServer notifications.
pub fn capture_disabled() -> bool {
    capture_disabled_at(CAPTURE_GATE.load(Ordering::Relaxed), monotonic_ns())
}

fn register(ids: &[u32]) {
    let Some(reg) = sls::SLSRegisterNotifyProc() else {
        log::warn!("SLSRegisterNotifyProc unavailable");
        return;
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
}

/// Starts space notifications (`sketchybar.c:system_events/space_events`): SkyLight
/// 1327/1328 (macOS ≥ 13) → [`SysEvent::SpaceChange`], and capture gating ids 904, 905,
/// 1401, 1508, 1322 → [`SysEvent::CaptureGating`]. Idempotent; call on the main thread.
pub fn start_space_events(sink: Sink) {
    with_state(|s| s.space_sink = Some(sink));
    let mut ids = vec![
        notify::CAPTURE_ENABLE_904,
        notify::CAPTURE_DISABLE_INDEFINITELY,
        notify::SPACES_REFRESH_1401,
        notify::CAPTURE_ENABLE_1508,
        notify::CAPTURE_DISABLE_TEMPORARILY,
    ];
    if util::os_at_least(13, 0) {
        ids.push(notify::SPACE_CHANGED_1327);
        ids.push(notify::SPACE_CHANGED_1328);
    }
    register(&ids);
}

/// `begin_receiving_space_window_events` (first `space_windows_change` subscription):
/// registers 1325, 1326, 815, 816, 1401 (+1327/1328 on macOS ≥ 13) and silently scans all
/// spaces. Idempotent; call on the main thread.
pub fn start_space_window_events(sink: Sink) {
    let first = with_state(|s| {
        let first = s.windows_sink.is_none();
        s.windows_sink = Some(sink);
        first
    });
    if !first {
        return;
    }
    let mut ids = vec![
        notify::WINDOW_CREATED,
        notify::WINDOW_DESTROYED,
        notify::WINDOW_UNHIDDEN,
        notify::WINDOW_HIDDEN,
        notify::SPACES_REFRESH_1401,
    ];
    if util::os_at_least(13, 0) {
        ids.push(notify::SPACE_CHANGED_1327);
        ids.push(notify::SPACE_CHANGED_1328);
    }
    register(&ids);
    update_all_spaces(true);
}

/// Whether window tracking was started.
pub fn space_window_events_started() -> bool {
    with_state(|s| s.windows_sink.is_some())
}

/// `forced_space_windows_event()`: if tracking started, rescans every space of every
/// display, posts one [`SysEvent::SpaceWindowsChange`] per space and returns the payloads.
pub fn forced_space_windows() -> Vec<String> {
    if !space_window_events_started() {
        return Vec::new();
    }
    update_all_spaces(false)
}

/// `app_windows_update_space` for every space of every active display.
fn update_all_spaces(silent: bool) -> Vec<String> {
    let mut out = Vec::new();
    for did in super::displays::active_display_ids() {
        let Some(uuid) = super::displays::display_uuid(did) else {
            continue;
        };
        for sid in spaces_of_display(&uuid) {
            if let Some(info) = update_space(sid, silent) {
                out.push(info);
            }
        }
    }
    out
}

/// Rescans one space; posts (unless `silent`) and returns its INFO payload.
fn update_space(sid: u64, silent: bool) -> Option<String> {
    let found = windows_on_space(sid);
    let (sink, all_wids, pids) = with_state(|s| {
        s.windows.retain(|w| w.sid != sid);
        for &(wid, pid) in &found {
            s.windows.push(TrackedWindow { wid, sid, pid });
        }
        let pids: Vec<i32> = s
            .windows
            .iter()
            .filter(|w| w.sid == sid)
            .map(|w| w.pid)
            .collect();
        let wids: Vec<u32> = s
            .windows
            .iter()
            .chain(s.hidden.iter())
            .map(|w| w.wid)
            .filter(|w| *w != 0)
            .collect();
        (s.windows_sink.clone(), wids, pids)
    });
    let mut result = None;
    if !silent {
        let index = mission_control_index(sid);
        let apps = group_app_windows(&pids, localized_app_name);
        let info = space_windows_info(index, &apps);
        if let Some(sink) = &sink {
            sink(SysEvent::SpaceWindowsChange {
                space: index,
                info_json: info.clone(),
            });
        }
        result = Some(info);
    }
    request_window_notifications(&all_wids);
    result
}

fn request_window_notifications(wids: &[u32]) {
    if let Some(f) = sls::SLSRequestNotificationsForWindows() {
        // SAFETY: pointer/count describe a valid u32 slice.
        unsafe { f(sls::cid(), wids.as_ptr(), wids.len() as i32) };
    }
}

fn post_space_change() {
    let sink = with_state(|s| s.space_sink.clone());
    if let Some(sink) = sink {
        sink(SysEvent::SpaceChange {
            info_json: space_change_info(&current_display_spaces()),
        });
    }
}

/// Posts a [`SysEvent::SpaceChange`] now (used for the NSWorkspace notification).
pub(crate) fn notify_space_changed() {
    post_space_change();
}

fn set_gate(value: i64) {
    let was = capture_disabled();
    CAPTURE_GATE.store(value, Ordering::Relaxed);
    let now = capture_disabled();
    if was != now || value == -1 || value > 0 {
        if let Some(sink) = with_state(|s| s.space_sink.clone()) {
            sink(SysEvent::CaptureGating { disabled: now });
        }
    }
}

unsafe extern "C" fn notify_proc(event: u32, data: *mut c_void, len: usize, _ctx: *mut c_void) {
    // Never unwind into SkyLight.
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `data` points to `len` bytes provided by SkyLight for this event.
        unsafe { handle_notify(event, data as *const u8, len) }
    });
}

unsafe fn handle_notify(event: u32, data: *const u8, len: usize) {
    let windows_started = space_window_events_started();
    match event {
        notify::CAPTURE_DISABLE_TEMPORARILY => set_gate(monotonic_ns()),
        notify::CAPTURE_DISABLE_INDEFINITELY => set_gate(-1),
        notify::CAPTURE_ENABLE_904 | notify::CAPTURE_ENABLE_1508 => set_gate(0),
        notify::SPACES_REFRESH_1401 => {
            set_gate(0);
            if windows_started {
                update_all_spaces(true);
            }
        }
        notify::SPACE_CHANGED_1327 | notify::SPACE_CHANGED_1328 => {
            post_space_change();
            if windows_started {
                update_all_spaces(false);
            }
        }
        notify::WINDOW_CREATED | notify::WINDOW_DESTROYED if windows_started => {
            if data.is_null() || len < 12 {
                return;
            }
            // SAFETY: payload is `{ u64 sid; u32 wid; }` (≥ 12 bytes, checked above).
            let (sid, wid) = unsafe {
                (
                    std::ptr::read_unaligned(data as *const u64),
                    std::ptr::read_unaligned(data.add(8) as *const u32),
                )
            };
            if wid == 0 || sid == 0 {
                return;
            }
            if event == notify::WINDOW_CREATED {
                if window_is_suitable(wid) {
                    update_space(sid, false);
                }
            } else {
                let known = with_state(|s| s.windows.iter().any(|w| w.wid == wid && w.sid == sid));
                if known {
                    update_space(sid, false);
                    with_state(|s| s.hidden.retain(|w| w.wid != wid));
                }
            }
        }
        notify::WINDOW_HIDDEN | notify::WINDOW_UNHIDDEN if windows_started => {
            if data.is_null() || len < 4 {
                return;
            }
            // SAFETY: payload is a `u32` window id (≥ 4 bytes, checked above).
            let wid = unsafe { std::ptr::read_unaligned(data as *const u32) };
            if event == notify::WINDOW_HIDDEN {
                let entry = with_state(|s| {
                    let e = s.windows.iter().find(|w| w.wid == wid).copied();
                    if let Some(e) = e {
                        if !s.hidden.iter().any(|h| h.wid == wid) {
                            s.hidden.push(e);
                        }
                    }
                    e
                });
                if let Some(e) = entry {
                    update_space(e.sid, false);
                }
            } else {
                let entry = with_state(|s| s.hidden.iter().find(|w| w.wid == wid).copied());
                if let Some(e) = entry {
                    update_space(e.sid, false);
                    let wids = with_state(|s| {
                        s.hidden.retain(|w| w.wid != wid);
                        s.windows
                            .iter()
                            .chain(s.hidden.iter())
                            .map(|w| w.wid)
                            .collect::<Vec<_>>()
                    });
                    request_window_notifications(&wids);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<DisplaySpaces> {
        vec![
            DisplaySpaces {
                display: "A".into(),
                spaces: vec![10, 11, 12],
            },
            DisplaySpaces {
                display: "B".into(),
                spaces: vec![20, 21],
            },
        ]
    }

    #[test]
    fn mission_control_indices() {
        let l = sample();
        assert_eq!(mission_control_index_in(&l, 10), 1);
        assert_eq!(mission_control_index_in(&l, 20), 4);
        assert_eq!(mission_control_index_in(&l, 99), 0);
        assert_eq!(space_at_index_in(&l, 5), 21);
        assert_eq!(space_at_index_in(&l, 6), 0);
        assert_eq!(space_at_index_in(&l, 0), 0);
    }

    #[test]
    fn space_change_payload() {
        assert_eq!(
            space_change_info(&[(1, 2), (2, 5)]),
            "{\n\t\"display-1\": 2,\n\t\"display-2\": 5\n}"
        );
        assert_eq!(space_change_info(&[]), "{\n}");
        assert_eq!(
            space_change_info(&[(12, 345)]),
            "{\n\t\"display-12\": 345\n}"
        );
    }

    #[test]
    fn space_windows_payload() {
        assert_eq!(
            space_windows_info(3, &[]),
            "{\n\t\"space\": 3,\n\t\"apps\": {\n\n\t}\n}\n"
        );
        assert_eq!(
            space_windows_info(1, &[("Safari".into(), 2), ("Mail".into(), 1)]),
            "{\n\t\"space\": 1,\n\t\"apps\": {\n\t\t\"Safari\": 2,\n\t\t\"Mail\": 1\n\t}\n}\n"
        );
    }

    #[test]
    fn grouping() {
        let names = |pid: i32| match pid {
            1 => Some("Safari".to_string()),
            2 => Some("Mail".to_string()),
            3 => Some("Safari".to_string()),
            _ => None,
        };
        let g = group_app_windows(&[2, 1, 4, 1, 3, 2], names);
        assert_eq!(g, vec![("Mail".to_string(), 2), ("Safari".to_string(), 3)]);
    }

    #[test]
    fn suitability() {
        assert!(window_suitable(0, 0x2, 0x1));
        assert!(window_suitable(0, 0, 0x0400_0000_0000_0001));
        assert!(window_suitable(0, 0x2, 0x2 | 0x8000_0000));
        assert!(!window_suitable(5, 0x2, 0x1));
        assert!(!window_suitable(0, 0, 0x1));
        assert!(!window_suitable(0, 0x2, 0x2));
    }

    #[test]
    fn gating() {
        assert!(!capture_disabled_at(0, 100));
        assert!(capture_disabled_at(-1, 100));
        assert!(capture_disabled_at(100, 100 + (1 << 30)));
        assert!(!capture_disabled_at(100, 101 + (1 << 30)));
    }
}
