//! [`Services`]: the running system sources (workspace/distributed notifications, mouse
//! monitor, AX menu observer, providers, alias captures, hotload watcher, mach server),
//! the stateful part of the `SysEvent → Input` translation and the execution of every
//! [`PlatformRequest`].
//!
//! | PlatformRequest | action |
//! |---|---|
//! | `StartVolumeEvents` | `SystemEvents::start_volume_events` (CoreAudio listeners) |
//! | `StartBrightnessEvents` | `SystemEvents::start_brightness_events` (DisplayServices) |
//! | `StartMediaEvents` | `SystemEvents::start_media_events` (MediaRemote / helper) |
//! | `StartSpaceWindowEvents` | `SystemEvents::start_space_window_events` (SkyLight) |
//! | `RefreshMedia` | `events::media::refresh` (re-query now playing, async) |
//! | `ObserveNotification(n)` | `SystemEvents::observe(n)` (distributed center) |
//! | `LoadFont(path)` | `TextSystem::register_font` (process scope) |
//! | `CaptureAlias{item, owner, name, forced}` | `AliasScheduler::set(item, owner, name, 0)` + `refresh_now` (capture off the main thread; reply `Input::AliasImage`) |
//! | `RequestScreenCapture` | `CGRequestScreenCaptureAccess` once per process, if not yet granted |
//! | `StartProvider{item, provider, freq, args}` | `Providers::subscribe(item, …)` |
//! | `StopProvider{item}` | `Providers::unsubscribe(item)` |
//! | `OpenMenu{index}` | `menus::open_menu(index)` (AXPress on a background thread) |
//! | `SetMenuBarHidden(b)` | `menus::set_menubar_autohide(b)` (restored on exit) |
//! | `SetHotload(b)` | `HotloadWatcher::set_enabled(b)` (normally consumed by the driver) |
//! | `MachSend{service, payload}` | `mach_server::send_to_service` on a worker thread |

use super::convert::{self, monitored_kind, sys_event_to_input, window_mouse_input};
use super::resources::{core_rect, MacResources};
use super::windows::WindowManager;
use crate::gfx::text::TextSystem;
use crate::gfx::window::{MouseEvent as ViewMouse, MouseEventKind};
use crate::sys::alias::{self, AliasScheduler};
use crate::sys::events::{media, SystemEvents};
use crate::sys::hotload::HotloadWatcher;
use crate::sys::mach_server::{self, MachServer};
use crate::sys::menus::{self, MenuObserver};
use crate::sys::mouse::{MonitorOptions, MouseKind as SysMouseKind, MouseMonitor};
use crate::sys::providers::Providers;
use crate::sys::{MachReply, Sink, SysEvent};
use mbar_core::geometry::Point;
use mbar_core::item::ItemId;
use mbar_core::platform::{Input, OsEvent, PlatformRequest, WindowKey};
use objc2::MainThreadMarker;
use std::collections::HashMap;
use std::sync::mpsc;

/// Result of translating one [`SysEvent`].
#[derive(Debug)]
pub enum Translated {
    /// Feed into `Runtime::handle`.
    Input(Input),
    /// A mach IPC request: run it and answer through `reply`.
    Request { args: Vec<String>, reply: MachReply },
    /// Nothing for the core.
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClickTag {
    Up,
    Scroll,
}

/// Running system services. Main thread only; dropping it stops every source.
pub struct Services {
    mtm: MainThreadMarker,
    sink: Sink,
    events: SystemEvents,
    _mouse: MouseMonitor,
    menus: Option<MenuObserver>,
    providers: Option<Providers>,
    aliases: Option<AliasScheduler>,
    /// Alias captures requested with `forced` (posted even when unchanged).
    alias_forced: HashMap<u64, bool>,
    /// Last `(window_id, frame)` per alias, for dropping unchanged unforced captures.
    alias_last: HashMap<u64, (u32, mbar_core::geometry::Rect)>,
    hotload: Option<HotloadWatcher>,
    mach: Option<MachServer>,
    mach_tx: Option<mpsc::Sender<(String, Vec<u8>)>>,
    screen_capture_requested: bool,
    /// A release/scroll the NSEvent monitor delivered; the view's copy of the same event
    /// (dispatched right after) is dropped.
    monitor_pending: Option<ClickTag>,
}

impl Services {
    /// Starts the startup sources (`SystemEvents::start`, the local mouse monitor for
    /// releases and scrolls, the AX menu observer if trusted, the hotload watcher on
    /// `watch_dir` and the mach server `dev.rubeen.<bar_name>`). Providers and alias
    /// captures start lazily.
    pub fn start(
        sink: Sink,
        mtm: MainThreadMarker,
        bar_name: &str,
        watch_dir: Option<&str>,
    ) -> Services {
        let events = SystemEvents::start(sink.clone(), mtm);
        let mouse = MouseMonitor::start(
            sink.clone(),
            MonitorOptions {
                global: false,
                local: true,
                motion: false,
                clicks: true,
                scroll: true,
            },
            mtm,
        );
        let menus = menus::is_trusted(false).then(|| MenuObserver::start(sink.clone(), true, mtm));
        let hotload = watch_dir.and_then(|d| HotloadWatcher::start(d, false, sink.clone()));
        let mach = match mach_server::start(bar_name, sink.clone()) {
            Ok(m) => Some(m),
            Err(e) => {
                log::warn!("mach server not started: {e}");
                None
            }
        };
        Services {
            mtm,
            sink,
            events,
            _mouse: mouse,
            menus,
            providers: None,
            aliases: None,
            alias_forced: HashMap::new(),
            alias_last: HashMap::new(),
            hotload,
            mach,
            mach_tx: None,
            screen_capture_requested: false,
            monitor_pending: None,
        }
    }

    /// Mach bootstrap name, if the server runs.
    pub fn mach_name(&self) -> Option<&str> {
        self.mach.as_ref().map(|m| m.name())
    }

    /// Mirrors the driver's `--hotload` flag into the FSEvents watcher.
    pub fn sync_hotload(&self, on: bool) {
        if let Some(h) = &self.hotload {
            if h.enabled() != on {
                h.set_enabled(on);
            }
        }
    }

    fn ensure_menu_observer(&mut self) {
        if self.menus.is_none() && menus::is_trusted(false) {
            self.menus = Some(MenuObserver::start(self.sink.clone(), true, self.mtm));
        }
    }

    /// Translates one system event, updating caches first (display/space state is
    /// refreshed before the core sees the event).
    pub fn translate(
        &mut self,
        ev: SysEvent,
        res: &mut MacResources,
        wm: &mut WindowManager,
    ) -> Translated {
        if let Some(p) = &self.providers {
            p.on_event(&ev);
        }
        match ev {
            SysEvent::DisplaysReconfigured { .. } | SysEvent::SystemWoke { .. } => {
                res.refresh_displays();
                wm.mark_displays_changed();
            }
            SysEvent::DisplayChange { .. } | SysEvent::SpaceChange { .. } => {
                res.refresh_displays();
            }
            SysEvent::MenuBarHidingChanged => res.refresh_menu_bar(),
            SysEvent::WifiChange(ref s) => res.last_ssid = Some(s.clone()),
            SysEvent::FrontAppSwitched { .. } => self.ensure_menu_observer(),
            SysEvent::SpaceWindowsChange { ref info_json, .. } => {
                let sup = &mut res.suppressed_space_windows;
                if let Some(i) = sup.iter().position(|s| s == info_json) {
                    sup.swap_remove(i);
                    return Translated::Nothing;
                }
            }
            SysEvent::MediaArtwork(img) => {
                let info = res.images.set_artwork(img.as_deref());
                return Translated::Input(Input::Event(OsEvent::MediaArtwork(info)));
            }
            SysEvent::AliasUpdate {
                id,
                window_id,
                frame,
                image,
                disabled,
                ..
            } => {
                let forced = self.alias_forced.remove(&id).unwrap_or(false);
                let frame = core_rect(&frame);
                let size = Some((frame.width, frame.height)).filter(|(w, h)| *w > 0.0 && *h > 0.0);
                let (info, changed) = match image.as_deref() {
                    Some(img) => match res.images.set_alias(id, img, size) {
                        Some((i, c)) => (Some(i), c),
                        None => (None, true),
                    },
                    None => (None, true),
                };
                let same_place = self.alias_last.get(&id) == Some(&(window_id, frame));
                self.alias_last.insert(id, (window_id, frame));
                if !forced && !changed && same_place && !disabled {
                    return Translated::Nothing;
                }
                return Translated::Input(Input::AliasImage {
                    item: ItemId(id),
                    window_id,
                    frame,
                    image: info,
                    disabled,
                });
            }
            SysEvent::MachMessage { args, reply, .. } => {
                return Translated::Request { args, reply };
            }
            SysEvent::Mouse(m) => {
                let window = wm.key_for_window_number(m.window_number).or_else(|| {
                    wm.key_at(Point::new(m.location.x as f32, m.location.y as f32))
                });
                self.monitor_pending = match m.kind {
                    SysMouseKind::Up => Some(ClickTag::Up),
                    SysMouseKind::Scrolled { .. } => Some(ClickTag::Scroll),
                    _ => self.monitor_pending,
                };
                return match convert::monitor_mouse_input(&m, window) {
                    Some(i) => Translated::Input(Input::Mouse(i)),
                    None => Translated::Nothing,
                };
            }
            _ => {}
        }
        match sys_event_to_input(ev) {
            Some(i) => Translated::Input(i),
            None => Translated::Nothing,
        }
    }

    /// A bar/popup view mouse event. Releases and scrolls already reported (with exact CG
    /// data) by the NSEvent monitor are dropped; otherwise they are mapped approximately.
    pub fn view_mouse(&mut self, key: WindowKey, ev: &ViewMouse, wm: &WindowManager) -> Option<Input> {
        if monitored_kind(ev.kind) {
            let tag = if ev.kind == MouseEventKind::Up {
                ClickTag::Up
            } else {
                ClickTag::Scroll
            };
            if self.monitor_pending == Some(tag) {
                self.monitor_pending = None;
                return None;
            }
        } else if ev.kind != MouseEventKind::Down {
            self.monitor_pending = None;
        }
        let origin = wm.origin(key)?;
        window_mouse_input(key, origin, ev).map(Input::Mouse)
    }

    /// Executes one platform request (see the module table).
    pub fn execute(&mut self, req: PlatformRequest, _res: &mut MacResources) {
        match req {
            PlatformRequest::StartVolumeEvents => self.events.start_volume_events(),
            PlatformRequest::StartBrightnessEvents => self.events.start_brightness_events(),
            PlatformRequest::StartMediaEvents => self.events.start_media_events(),
            PlatformRequest::StartSpaceWindowEvents => self.events.start_space_window_events(),
            PlatformRequest::RefreshMedia => media::refresh(),
            PlatformRequest::ObserveNotification(name) => self.events.observe(&name),
            PlatformRequest::LoadFont(path) => {
                if !TextSystem::register_font(&path) {
                    log::warn!("could not register font '{path}'");
                }
            }
            PlatformRequest::CaptureAlias {
                item,
                owner,
                name,
                forced,
            } => {
                let sink = self.sink.clone();
                let s = self.aliases.get_or_insert_with(|| AliasScheduler::new(sink));
                s.set(item.0, &owner, name.as_deref(), 0);
                s.refresh_now(item.0);
                let f = self.alias_forced.entry(item.0).or_insert(false);
                *f |= forced;
            }
            PlatformRequest::RequestScreenCapture => {
                if !self.screen_capture_requested && !alias::screen_capture_preflight() {
                    self.screen_capture_requested = true;
                    alias::request_screen_capture();
                }
            }
            PlatformRequest::StartProvider {
                item,
                provider,
                freq,
                args,
            } => {
                let sink = self.sink.clone();
                let p = self.providers.get_or_insert_with(|| Providers::new(sink));
                if let Err(e) = p.subscribe(item.0, &provider, freq, args) {
                    log::debug!("provider for item {}: {e}", item.0);
                }
            }
            PlatformRequest::StopProvider { item } => {
                if let Some(p) = &self.providers {
                    p.unsubscribe(item.0);
                }
            }
            PlatformRequest::OpenMenu { index } => menus::open_menu(index),
            PlatformRequest::SetMenuBarHidden(hidden) => menus::set_menubar_autohide(hidden),
            PlatformRequest::SetHotload(on) => self.sync_hotload(on),
            PlatformRequest::MachSend { service, payload } => self.mach_send(service, payload),
        }
    }

    /// One-way mach send off the main thread (`bootstrap_look_up` + send may block).
    fn mach_send(&mut self, service: String, payload: Vec<u8>) {
        if self.mach_tx.is_none() {
            let (tx, rx) = mpsc::channel::<(String, Vec<u8>)>();
            let spawned = std::thread::Builder::new()
                .name("mbar-mach-send".into())
                .spawn(move || {
                    for (service, payload) in rx {
                        if !mach_server::send_to_service(&service, &payload) {
                            log::debug!("mach_helper '{service}' not reachable");
                        }
                    }
                });
            if spawned.is_err() {
                log::warn!("cannot start mach sender thread");
                return;
            }
            self.mach_tx = Some(tx);
        }
        if let Some(tx) = &self.mach_tx {
            let _ = tx.send((service, payload));
        }
    }

    /// Exit cleanup: restore the menu-bar auto-hide setting, stop the media helper.
    pub fn shutdown(&mut self) {
        menus::restore_menubar_autohide();
        media::stop();
        self.mach_tx = None;
    }
}
