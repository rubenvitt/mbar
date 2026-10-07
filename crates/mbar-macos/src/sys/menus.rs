//! Accessibility-based menu-bar integration (`docs/EXTENSIONS.md`: `app_menu`, `--menu`,
//! `--query menus`, `menus_change`, `--menubar`, `hide_menubar`).
//!
//! * Reading titles: `AXMenuBar` → `AXChildren` → `AXTitle` of the menu-bar owning app.
//!   Index 0 is the Apple menu; [`menu_titles`] skips it unless asked.
//! * Opening: `AXPress` on the top-level item (falls back to `AXShowMenu`), performed on a
//!   background thread because the call blocks while the menu is tracking.
//! * Observing: [`MenuObserver`] follows app activation and attaches an `AXObserver` to the
//!   new app (focused/main window changes, menu-bar title changes / destruction); titles are
//!   re-read off the main thread and posted as [`SysEvent::MenusChanged`] when they changed.
//! * Auto-hiding the native menu bar: `_HIHideMenuBar` in the global domain + the
//!   `AppleInterfaceMenuBarHidingChangedNotification` distributed notification; the original
//!   value is remembered and restored by [`restore_menubar_autohide`].
//!
//! Requires the Accessibility permission (except for the auto-hide setting).

use super::util::{self, cfstr, owned};
use super::{events::Observer, Sink, SysEvent};
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceDidActivateApplicationNotification};
use objc2_application_services::{AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions, AXObserver, AXUIElement};
use objc2_core_foundation::{
    kCFPreferencesAnyApplication, kCFRunLoopDefaultMode, CFArray, CFBoolean, CFDictionary,
    CFPreferencesAppSynchronize, CFPreferencesCopyAppValue, CFPreferencesSetAppValue, CFRetained,
    CFRunLoop, CFString, CFType,
};
use objc2_foundation::{NSDistributedNotificationCenter, NSString};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Messaging timeout for AX calls (seconds); a hung app must not stall us for the default 6 s.
const AX_TIMEOUT: f32 = 1.0;

/// `AXIsProcessTrustedWithOptions({AXTrustedCheckOptionPrompt: prompt})`.
pub fn is_trusted(prompt: bool) -> bool {
    if !prompt {
        // SAFETY: no preconditions.
        return unsafe { AXIsProcessTrusted() };
    }
    // SAFETY: reading an immutable CF constant.
    let key: &CFString = unsafe { objc2_application_services::kAXTrustedCheckOptionPrompt };
    let value: &CFType = CFBoolean::new(true);
    let dict = CFDictionary::<CFString, CFType>::from_slices(&[key], &[value]);
    // SAFETY: a valid options dictionary.
    unsafe { AXIsProcessTrustedWithOptions(Some(dict.as_opaque())) }
}

fn attr(elem: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    let mut out: *const CFType = std::ptr::null();
    // SAFETY: valid attribute string and out pointer; the result follows the Copy rule.
    let err = unsafe { elem.copy_attribute_value(&cfstr(name), NonNull::from(&mut out)) };
    if err != AXError::Success {
        return None;
    }
    // SAFETY: on success `out` is an owned CF object (or null).
    unsafe { owned(out) }
}

fn menu_bar_items(pid: i32) -> Option<Vec<CFRetained<AXUIElement>>> {
    // SAFETY: any pid is accepted; invalid ones fail on use.
    let app = unsafe { AXUIElement::new_application(pid) };
    // SAFETY: plain value argument.
    unsafe { app.set_messaging_timeout(AX_TIMEOUT) };
    let bar = attr(&app, "AXMenuBar").and_then(util::downcast::<AXUIElement>)?;
    let children = attr(&bar, "AXChildren").and_then(util::downcast::<CFArray>)?;
    Some(
        util::array_items(&children)
            .into_iter()
            .filter_map(util::downcast::<AXUIElement>)
            .collect(),
    )
}

/// All top-level menu titles of `pid` (index 0 = Apple menu).
pub fn all_menu_titles(pid: i32) -> Option<Vec<String>> {
    Some(
        menu_bar_items(pid)?
            .iter()
            .map(|e| attr(e, "AXTitle").and_then(|t| util::cf_string(&t)).unwrap_or_default())
            .collect(),
    )
}

/// Pure: drops the Apple menu (index 0) unless `include_apple`.
pub fn select_titles(all: Vec<String>, include_apple: bool) -> Vec<String> {
    if include_apple {
        all
    } else {
        all.into_iter().skip(1).collect()
    }
}

/// Top-level menu titles of `pid`, without the Apple menu unless `include_apple`.
pub fn menu_titles(pid: i32, include_apple: bool) -> Option<Vec<String>> {
    all_menu_titles(pid).map(|t| select_titles(t, include_apple))
}

/// The application owning the menu bar (falls back to the frontmost application):
/// `(localized name, pid)`.
pub fn menu_bar_owner() -> Option<(String, i32)> {
    let ws = NSWorkspace::sharedWorkspace();
    let app: Retained<NSRunningApplication> = ws.menuBarOwningApplication().or_else(|| ws.frontmostApplication())?;
    Some((
        app.localizedName().map(|s| s.to_string()).unwrap_or_default(),
        app.processIdentifier(),
    ))
}

/// `--query menus`: `(app, pid, titles)` of the menu-bar owner. Blocks up to the AX timeout.
pub fn front_app_menus(include_apple: bool) -> Option<(String, i32, Vec<String>)> {
    let (app, pid) = menu_bar_owner()?;
    let titles = menu_titles(pid, include_apple)?;
    Some((app, pid, titles))
}

/// Pure: resolves `--menu <index|title>` against all titles (index 0 = Apple menu).
pub fn resolve_menu_target(all: &[String], target: &str) -> Option<usize> {
    if let Ok(i) = target.parse::<usize>() {
        return (i < all.len()).then_some(i);
    }
    all.iter().position(|t| t == target)
}

fn press(pid: i32, target: MenuTarget) {
    let Some(items) = menu_bar_items(pid) else {
        log::warn!("open_menu: no menu bar (Accessibility permission?)");
        return;
    };
    let index = match target {
        MenuTarget::Index(i) => Some(i),
        MenuTarget::Title(t) => {
            let titles: Vec<String> = items
                .iter()
                .map(|e| attr(e, "AXTitle").and_then(|t| util::cf_string(&t)).unwrap_or_default())
                .collect();
            resolve_menu_target(&titles, &t)
        }
    };
    let Some(item) = index.and_then(|i| items.get(i)) else {
        return;
    };
    // SAFETY: valid element and action names.
    let err = unsafe { item.perform_action(&cfstr("AXPress")) };
    if err == AXError::ActionUnsupported {
        // SAFETY: as above.
        unsafe { item.perform_action(&cfstr("AXShowMenu")) };
    }
}

enum MenuTarget {
    Index(usize),
    Title(String),
}

/// Opens top-level menu `index` (0 = Apple menu) of the menu-bar owner, asynchronously.
pub fn open_menu(index: usize) {
    open(MenuTarget::Index(index));
}

/// Opens the top-level menu titled `title` (or a numeric index given as text).
pub fn open_menu_titled(title: &str) {
    match title.parse::<usize>() {
        Ok(i) => open(MenuTarget::Index(i)),
        Err(_) => open(MenuTarget::Title(title.to_string())),
    }
}

fn open(target: MenuTarget) {
    let Some((_, pid)) = menu_bar_owner() else { return };
    let _ = std::thread::Builder::new()
        .name("mbar-open-menu".into())
        .spawn(move || press(pid, target));
}

// ---------------------------------------------------------------------------------------
// Observation
// ---------------------------------------------------------------------------------------

struct Shared {
    sink: Option<Sink>,
    last: Option<(i32, Vec<String>)>,
    include_apple: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    sink: None,
    last: None,
    include_apple: true,
});
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Re-reads the owner's titles on a background thread and posts them if they changed.
fn refresh_async() {
    let Some((app, pid)) = menu_bar_owner() else { return };
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = std::thread::Builder::new().name("mbar-menus".into()).spawn(move || {
        let include_apple = SHARED.lock().unwrap_or_else(|e| e.into_inner()).include_apple;
        let Some(titles) = menu_titles(pid, include_apple) else { return };
        if GENERATION.load(Ordering::SeqCst) != generation {
            return; // superseded by a newer refresh
        }
        let sink = {
            let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
            let entry = (pid, titles.clone());
            if s.last.as_ref() == Some(&entry) {
                None
            } else {
                s.last = Some(entry);
                s.sink.clone()
            }
        };
        if let Some(sink) = sink {
            sink(SysEvent::MenusChanged { app, pid, titles });
        }
    });
}

unsafe extern "C-unwind" fn ax_callback(
    _observer: NonNull<AXObserver>,
    _element: NonNull<AXUIElement>,
    _notification: NonNull<CFString>,
    _refcon: *mut c_void,
) {
    refresh_async();
}

struct Attached {
    pid: i32,
    observer: CFRetained<AXObserver>,
}

/// Follows the menu-bar owner and posts [`SysEvent::MenusChanged`]. Main thread only;
/// dropping it stops observation.
pub struct MenuObserver {
    _activation: Observer,
    attached: std::rc::Rc<std::cell::RefCell<Option<Attached>>>,
}

fn detach(slot: &mut Option<Attached>) {
    if let Some(a) = slot.take() {
        // SAFETY: the observer's run-loop source is valid while the observer lives.
        let source = unsafe { a.observer.run_loop_source() };
        if let Some(main) = CFRunLoop::main() {
            // SAFETY: reading an immutable CF constant.
            main.remove_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
        }
    }
}

fn attach(slot: &mut Option<Attached>, pid: i32) {
    if slot.as_ref().map(|a| a.pid) == Some(pid) {
        return;
    }
    detach(slot);
    let mut raw: *mut AXObserver = std::ptr::null_mut();
    // SAFETY: static callback; valid out pointer.
    let err = unsafe { AXObserver::create(pid, Some(ax_callback), NonNull::from(&mut raw)) };
    if err != AXError::Success {
        return;
    }
    // SAFETY: on success `raw` is an owned AXObserver.
    let Some(observer) = (unsafe { owned(raw as *const AXObserver) }) else {
        return;
    };
    // SAFETY: any pid is accepted.
    let app = unsafe { AXUIElement::new_application(pid) };
    // SAFETY: valid observer/element/notification names; no refcon.
    unsafe {
        for n in ["AXFocusedWindowChanged", "AXMainWindowChanged"] {
            observer.add_notification(&app, &cfstr(n), std::ptr::null_mut());
        }
        if let Some(bar) = attr(&app, "AXMenuBar").and_then(util::downcast::<AXUIElement>) {
            for n in ["AXTitleChanged", "AXUIElementDestroyed"] {
                observer.add_notification(&bar, &cfstr(n), std::ptr::null_mut());
            }
        }
        let source = observer.run_loop_source();
        if let Some(main) = CFRunLoop::main() {
            main.add_source(Some(&source), kCFRunLoopDefaultMode);
        }
    }
    *slot = Some(Attached { pid, observer });
}

impl MenuObserver {
    /// Starts observing (main thread). `include_apple` controls whether posted titles keep
    /// the Apple menu at index 0.
    pub fn start(sink: Sink, include_apple: bool, _mtm: MainThreadMarker) -> MenuObserver {
        {
            let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
            s.sink = Some(sink);
            s.include_apple = include_apple;
            s.last = None;
        }
        let attached = std::rc::Rc::new(std::cell::RefCell::new(None));
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let slot = attached.clone();
        // SAFETY: AppKit's notification name constant is a valid static NSString.
        let name: &NSString = unsafe { NSWorkspaceDidActivateApplicationNotification };
        let activation = Observer::new(&center, Some(name), move |_| {
            if let Some((_, pid)) = menu_bar_owner() {
                attach(&mut slot.borrow_mut(), pid);
            }
            refresh_async();
        });
        if let Some((_, pid)) = menu_bar_owner() {
            attach(&mut attached.borrow_mut(), pid);
        }
        refresh_async();
        MenuObserver {
            _activation: activation,
            attached,
        }
    }

    /// Forces a re-read (posts only if titles changed).
    pub fn refresh(&self) {
        refresh_async();
    }
}

impl Drop for MenuObserver {
    fn drop(&mut self) {
        detach(&mut self.attached.borrow_mut());
        SHARED.lock().unwrap_or_else(|e| e.into_inner()).sink = None;
    }
}

// ---------------------------------------------------------------------------------------
// Native menu bar auto-hide
// ---------------------------------------------------------------------------------------

const HIDE_KEY: &str = "_HIHideMenuBar";
static ORIGINAL: Mutex<Option<bool>> = Mutex::new(None);

/// Current `_HIHideMenuBar` (global domain); `false` when unset.
pub fn menubar_autohide() -> bool {
    // SAFETY: reading an immutable CF constant.
    let any = unsafe { kCFPreferencesAnyApplication };
    CFPreferencesCopyAppValue(&cfstr(HIDE_KEY), any)
        .and_then(|v| util::cf_bool(&v))
        .unwrap_or(false)
}

fn write_autohide(hidden: bool) {
    // SAFETY: reading an immutable CF constant.
    let any = unsafe { kCFPreferencesAnyApplication };
    let value: &CFType = CFBoolean::new(hidden);
    // SAFETY: valid key, property-list value and application id.
    unsafe { CFPreferencesSetAppValue(&cfstr(HIDE_KEY), Some(value), any) };
    CFPreferencesAppSynchronize(any);
    let name = NSString::from_str("AppleInterfaceMenuBarHidingChangedNotification");
    // SAFETY: valid notification name; no object/userInfo.
    unsafe {
        NSDistributedNotificationCenter::defaultCenter()
            .postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true)
    };
}

/// `--menubar hide|show` / `hide_menubar=`: sets "Automatically hide and show the menu bar".
/// The value found before the first change is remembered for [`restore_menubar_autohide`].
pub fn set_menubar_autohide(hidden: bool) {
    {
        let mut orig = ORIGINAL.lock().unwrap_or_else(|e| e.into_inner());
        if orig.is_none() {
            *orig = Some(menubar_autohide());
        }
    }
    if menubar_autohide() != hidden {
        write_autohide(hidden);
    }
}

/// `--menubar toggle`. Returns the new state.
pub fn toggle_menubar_autohide() -> bool {
    let new = !menubar_autohide();
    set_menubar_autohide(new);
    new
}

/// Restores the value from before mbar's first change (call on exit). No-op if mbar never
/// changed it.
pub fn restore_menubar_autohide() {
    let orig = ORIGINAL.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(orig) = orig {
        if menubar_autohide() != orig {
            write_autohide(orig);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_selection() {
        let all = vec!["Apple".to_string(), "Safari".to_string(), "File".to_string()];
        assert_eq!(select_titles(all.clone(), false), vec!["Safari", "File"]);
        assert_eq!(select_titles(all.clone(), true).len(), 3);
        assert_eq!(resolve_menu_target(&all, "0"), Some(0));
        assert_eq!(resolve_menu_target(&all, "File"), Some(2));
        assert_eq!(resolve_menu_target(&all, "9"), None);
        assert_eq!(resolve_menu_target(&all, "Edit"), None);
    }
}
