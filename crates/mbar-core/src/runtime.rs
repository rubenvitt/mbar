//! The state machine — **WP-C** (`docs/DESIGN-CORE.md` "Runtime").
//!
//! `Runtime` owns all state, consumes [`Input`]s and emits [`Effect`]s; `frame` produces the
//! windows to redraw. Single-threaded; `handle` never draws, it marks dirty state.
//!
//! Message handling follows `cli.md` §2.5: reset `--animate`, freeze, run every command in
//! order (appending responses), resize/`bar_needs_update` if a `--bar`/`--update`/`--remove`
//! asked for it, `animator.lock_all()`, unfreeze, refresh, reply.
//!
//! Redraw pipeline (mbar's version of `bar_manager_refresh` / `bar_draw`):
//!
//! * every handled input ends with [`Runtime::refresh`], which reproduces SketchyBar's redraw
//!   *decision* (`bar_manager_bar_needs_redraw`, forced refreshes, `associated_bar` bits,
//!   bar resizes) and records the windows that must be re-rendered in `dirty`;
//! * [`Runtime::frame`] steps animations, refreshes again, runs **one** layout pass for
//!   everything that changed since the last frame (several messages between two frames
//!   share one pass) and returns scenes only for windows whose content, geometry or window
//!   properties changed. When nothing is dirty `frame` returns immediately without any
//!   allocation.
//!
//! Helper methods are grouped by spec area below; each group lists the functions it calls
//! from other work packages (see `docs/IMPLEMENTATION-PLAN.md` "Cross-package contracts").

use crate::aerospace::{self, AerospaceEvent};
use crate::animation::{self, AnimStep, Animator};
use crate::bar::{BarState, DISPLAY_MAIN};
use crate::command::{
    self, AddCommand, Command, MenuBarAction, MonitorMode, Placement, QueryTarget, Selector,
    SetToken,
};
use crate::event::{self, AppendResult, BarSpace, EventInfo, EventKind, ScrollThrottle};
use crate::geometry::{Point, Rect};
use crate::group;
use crate::item::{BarItem, ItemId, ItemType, Position};
use crate::layout::{self, BarLayout, Layout, PopupLayout, WindowHit};
use crate::model::Model;
use crate::platform::{
    Effect, FrameOutput, ImageInfo, Input, LuaRequest, MouseInput, MouseKind, OsEvent,
    PlatformRequest, ReplyToken, Resources, SpaceMove, SystemQuery, SystemValue, TextKey,
    WindowKey, WindowUpdate,
};
use crate::privacy::{self, PrivacySample};
use crate::props::{
    AnimSpec, AnimTarget, HiddenRequest, PropCx, PropEffects, PropRequest, PropResult,
};
use crate::provider::{self, ProviderKind};
use crate::query::{self, QueryCx, Stats};
use crate::script::{self, EnvVars, Sender, MACH_HELPER_DESTROY};
use crate::value;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt::Write;
use std::time::{Duration, Instant};

/// Static configuration of a daemon instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Bar name / IPC identity (basename of argv[0]; `sketchybar` maps to `mbar`). The
    /// `BAR_NAME` env var keeps the unmapped basename.
    pub bar_name: String,
    /// `$HOME` for `~` expansion.
    pub home: String,
    /// Resolved config file path (for `--reload` without a path).
    pub config_path: Option<String>,
}

/// Lazily started OS listeners (`begin_receiving_*`, never undone, survive hotload).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Listeners {
    pub volume: bool,
    pub brightness: bool,
    pub media: bool,
    pub space_windows: bool,
}

/// The routine clock period (`SHELL_REFRESH`, 1.0 s).
pub const CLOCK_PERIOD: Duration = Duration::from_secs(1);
/// Delay of the second `system_woke` after a real sleep.
pub const WAKE_REPOST_DELAY: Duration = Duration::from_millis(500);

/// Number of frame/layout timing samples kept for `--query stats`.
const SAMPLE_CAP: usize = 512;
/// Pending script start times kept per item (for durations from `ScriptFinished`).
const PENDING_SCRIPTS_CAP: usize = 64;
/// Compiled regex selectors kept by [`Runtime::regex_select`] (cleared when full).
const REGEX_CACHE_SIZE: usize = 64;

/// Window properties of a bar window (part of every `WindowUpdate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WinProps {
    level: i32,
    blur: u32,
    shadow: bool,
    sticky: bool,
    font_smoothing: bool,
}

/// Outcome of [`Runtime::bar_needs_redraw`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redraw {
    No,
    /// Only popup members changed: update the association bits, re-render the bar window
    /// only if a popup member clips it.
    PopupMembers,
    Bar,
}

/// What was last sent to the platform for a bar window.
#[derive(Debug, Clone)]
struct EmittedBar {
    /// The layout as far as [`layout::bar_scene`] paints it ([`bar_scene_layout`]).
    layout: BarLayout,
    props: WinProps,
    /// The emitted scene contains clip holes of popup members.
    popup_clips: bool,
}

/// What was last sent to the platform for a popup window.
#[derive(Debug, Clone)]
struct EmittedPopup {
    layout: PopupLayout,
    blur: u32,
    props: WinProps,
}

/// Fixed-size ring of timing samples (µs).
#[derive(Debug, Default)]
struct Samples {
    buf: Vec<u64>,
    pos: usize,
}

impl Samples {
    fn push(&mut self, v: u64) {
        if self.buf.capacity() == 0 {
            self.buf.reserve_exact(SAMPLE_CAP);
        }
        if self.buf.len() < SAMPLE_CAP {
            self.buf.push(v);
        } else {
            self.buf[self.pos] = v;
        }
        self.pos = (self.pos + 1) % SAMPLE_CAP;
    }

    /// (avg, p95, max).
    fn summary(&self) -> (u64, u64, u64) {
        if self.buf.is_empty() {
            return (0, 0, 0);
        }
        let mut s = self.buf.clone();
        s.sort_unstable();
        let avg = s.iter().sum::<u64>() / s.len() as u64;
        let p95 = s[((s.len() * 95).div_ceil(100)).saturating_sub(1)];
        (avg, p95, *s.last().unwrap_or(&0))
    }
}

/// Per-item script statistics.
#[derive(Debug, Default)]
struct ScriptStat {
    runs: u64,
    finished: u64,
    total_ms: f64,
    max_ms: f64,
    pending: VecDeque<Instant>,
}

/// Outcome of [`Runtime::apply_requests`].
#[derive(Debug, Clone, Copy, Default)]
struct ReqOutcome {
    /// End-of-message refresh flag (`--bar hidden`, bar resets).
    refresh: bool,
    /// `AddToPopup` failed: the item must not be marked dirty (`item.md` §2.3).
    suppress_update: bool,
}

pub struct Runtime {
    pub config: RuntimeConfig,
    pub model: Model,
    pub animator: Animator,
    /// `bar_manager.frozen` (a plain bool, D14).
    frozen: bool,
    /// Between `system_will_sleep` and wake: routine ticks and `--update` are no-ops.
    sleeps: bool,
    /// `--hotload` state (survives reloads).
    hotload: bool,
    /// The `--animate` setting of the message being executed.
    anim: Option<AnimSpec>,
    /// Next routine tick.
    next_tick: Option<Instant>,
    /// Pending delayed `system_woke` re-post.
    wake_repost: Option<Instant>,
    /// Scroll coalescing (`events.md` §6.3).
    scroll: ScrollThrottle,
    /// Alias capture suspended by WindowServer notifications.
    capture_disabled: bool,
    listeners: Listeners,
    /// Last layout (hit testing, scenes).
    layout: Option<Layout>,
    /// The model changed in a way that needs a new layout pass.
    needs_layout: bool,
    /// Windows may need to be (re)sent to the platform.
    needs_render: bool,
    /// A forced refresh (`bar_manager_refresh(true)`) is pending.
    force_refresh: bool,
    /// Windows that must be re-rendered by the next `frame`.
    dirty: BTreeSet<WindowKey>,
    /// Bars (adid) redrawn by `refresh` only because of popup members: their windows are
    /// re-rendered only if a popup member punches a clip hole into the bar.
    popup_member_bars: BTreeSet<u32>,
    /// `bar_change_space` requests (adid → dsid) for the next `frame` (`bar.md` §6.4).
    space_moves: Vec<(u32, u64)>,
    /// Compiled `--set /regex/` selectors by pattern.
    regex_cache: HashMap<String, command::BreRegex>,
    /// Bar windows currently open on the platform (by adid) and what they show.
    emitted_bars: HashMap<u32, EmittedBar>,
    /// Popup windows currently open on the platform (by host).
    emitted_popups: HashMap<ItemId, EmittedPopup>,
    stats: Stats,
    frame_times: Samples,
    layout_times: Samples,
    redraws: HashMap<WindowKey, u64>,
    scripts: HashMap<String, ScriptStat>,
    scripts_finished: u64,
    script_total_ms: f64,
    script_finished_timed: u64,
    started: Option<Instant>,
    /// Most recent clock reading (`Resources::now` / `frame(now)`).
    last_now: Option<Instant>,
    /// `--monitor` subscribers exist (extension).
    monitor_events: bool,
    monitor_stats: bool,
    /// Front app menus (app_menu extension).
    menu_app: String,
    menu_titles: Vec<String>,
    /// The current message sends no reply (`--exit`).
    no_reply: bool,
    /// `--monitor` executed by the current message (merged mode; extension).
    monitor_req: Option<MonitorMode>,
    /// Time spent in executed Lua callbacks (µs), for `lua.avg_us`.
    lua_total_us: u64,
    /// `--exit` was executed: ignore the rest of the message.
    exiting: bool,
    /// `provider=aerospace` items whose label must be re-applied from
    /// `model.aerospace` at the end of the current input (coalesces repeated
    /// `ProviderChanged` requests of one message). `true`: apply even when the sample did
    /// not change (the provider was (re)configured).
    aerospace_pending: Vec<(ItemId, bool)>,
    /// The last `aerospace` sample applied to each `provider=aerospace` item: an event that
    /// does not change an item's sample neither re-applies its label nor runs its script.
    aerospace_applied: HashMap<ItemId, Vec<(String, String)>>,
    /// Late subscribers to built-in events (`aerospace_*`, `privacy_indicator_change`) that
    /// get the stored state as a synthetic event at the end of the current input (after layout, so `updates=when_shown`
    /// gating sees the item's real visibility).
    initial_events: Vec<(Listener, &'static str)>,
    /// Item-less in-process handlers (`LuaRequest::On`): `(event, handler)` in
    /// registration order. Cleared by `--reload`.
    global_handlers: Vec<(String, u64)>,
}

/// Who gets a synthetic built-in event (`Runtime::initial_events`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listener {
    Item(ItemId),
    Global(u64),
}

/// `lua:<id>` script values (`docs/LUA.md`).
fn lua_id(script: &str) -> Option<u64> {
    script.strip_prefix("lua:")?.parse().ok()
}

/// `CGRectContainsPoint` (half-open).
fn contains_half_open(r: &Rect, p: Point) -> bool {
    p.x >= r.x && p.x < r.x + r.width && p.y >= r.y && p.y < r.y + r.height
}

fn bit32(n: u32) -> u32 {
    1u32.checked_shl(n).unwrap_or(0)
}

fn name_or_null(n: Option<&str>) -> &str {
    n.unwrap_or("(null)")
}

/// The item punches a clip hole into the bar it is drawn on (`bar_item_clip_bar`).
fn item_clips_bar(item: &BarItem) -> bool {
    item.background.clips_bar()
        || item.icon.background.clips_bar()
        || item.label.background.clips_bar()
}

/// The part of a bar layout that [`layout::bar_scene`] paints: popup members (ids in
/// `popup_members`, value = clips the bar) are dropped unless they punch a clip hole.
/// Returns the filtered layout and whether any popup member clips the bar.
fn bar_scene_layout(bl: &BarLayout, popup_members: &HashMap<ItemId, bool>) -> (BarLayout, bool) {
    let mut clips = false;
    let mut keep = |id: &ItemId| match popup_members.get(id) {
        Some(c) => {
            clips |= *c;
            *c
        }
        None => true,
    };
    let items = if popup_members.is_empty() {
        bl.items.clone()
    } else {
        bl.items.iter().filter(|p| keep(&p.id)).copied().collect()
    };
    let menu_lines = bl
        .menu_lines
        .iter()
        .filter(|(id, _)| !popup_members.contains_key(id))
        .cloned()
        .collect();
    (
        BarLayout {
            adid: bl.adid,
            frame: bl.frame,
            items,
            menu_lines,
            geometry: bl.geometry.clone(),
        },
        clips,
    )
}

impl Runtime {
    /// `bar_manager_init` (no bars yet; call [`Runtime::begin`] once the platform is ready).
    pub fn new(config: RuntimeConfig) -> Self {
        Runtime {
            config,
            model: Model::new(),
            animator: Animator::new(),
            frozen: false,
            sleeps: false,
            hotload: false,
            anim: None,
            next_tick: None,
            wake_repost: None,
            scroll: ScrollThrottle::default(),
            capture_disabled: false,
            listeners: Listeners::default(),
            layout: None,
            needs_layout: false,
            needs_render: false,
            force_refresh: false,
            dirty: BTreeSet::new(),
            popup_member_bars: BTreeSet::new(),
            space_moves: Vec::new(),
            regex_cache: HashMap::new(),
            emitted_bars: HashMap::new(),
            emitted_popups: HashMap::new(),
            stats: Stats::default(),
            frame_times: Samples::default(),
            layout_times: Samples::default(),
            redraws: HashMap::new(),
            scripts: HashMap::new(),
            scripts_finished: 0,
            script_total_ms: 0.0,
            script_finished_timed: 0,
            started: None,
            last_now: None,
            monitor_events: false,
            monitor_stats: false,
            menu_app: String::new(),
            menu_titles: Vec::new(),
            no_reply: false,
            monitor_req: None,
            lua_total_us: 0,
            exiting: false,
            aerospace_pending: Vec::new(),
            aerospace_applied: HashMap::new(),
            initial_events: Vec::new(),
            global_handlers: Vec::new(),
        }
    }

    /// The last layout pass (hit testing, tests).
    pub fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }

    /// Lazily started OS listeners.
    pub fn listeners(&self) -> Listeners {
        self.listeners
    }

    /// `--hotload` state.
    pub fn hotload(&self) -> bool {
        self.hotload
    }

    /// `bar_manager_begin` at startup: create bars for the selected displays, poll the
    /// active display and schedule the first routine tick (now + 1 s).
    pub fn begin(&mut self, res: &mut dyn Resources) -> Vec<Effect> {
        let now = res.now();
        self.started = Some(now);
        self.last_now = Some(now);
        self.begin_bars(res);
        self.model.active_adid = res.active_display();
        self.next_tick = Some(now + CLOCK_PERIOD);
        self.refresh(false, res);
        Vec::new()
    }

    /// Processes one input. Before every input the active display is polled
    /// (`bar_manager_poll_active_display`, `events.md` §1.1).
    pub fn handle(&mut self, input: Input, res: &mut dyn Resources) -> Vec<Effect> {
        let now = res.now();
        self.last_now = Some(now);
        if self.started.is_none() {
            self.started = Some(now);
        }
        let mut effects = Vec::new();
        self.poll_active_display(&mut effects, res);
        match input {
            Input::Message { args, reply } => {
                let fx = self.handle_message(&args, reply, res);
                effects.extend(fx);
            }
            Input::Event(ev) => self.handle_os_event(ev, &mut effects, res),
            Input::Mouse(m) => self.handle_mouse(m, &mut effects, res),
            Input::Timer => self.on_timer(&mut effects, res),
            Input::ScriptFinished { item, .. } => self.script_finished(item, now),
            Input::ProviderSample { item, values } => {
                self.provider_sample(item, values, &mut effects, res)
            }
            Input::AliasImage {
                item,
                window_id,
                frame,
                image,
                disabled,
            } => self.alias_image(item, image, window_id, frame, disabled),
            Input::DisplaysChanged => self.displays_changed(&mut effects, res),
            Input::Lua(req) => self.handle_lua(req, &mut effects, res),
            Input::MenuTitles { app, titles } => self.menu_titles(app, titles, &mut effects),
            Input::Aerospace(ev) => self.aerospace_event(ev, &mut effects),
            Input::AerospaceStatus(status) => self.model.aerospace.status = status,
            Input::PrivacyIndicator(sample) => self.privacy_sample(sample, &mut effects),
        }
        self.flush_aerospace_providers(&mut effects, res);
        self.refresh(false, res);
        if self.flush_initial(&mut effects) {
            self.refresh(false, res);
        }
        effects
    }

    /// Requests the AeroSpace connection from outside a message (Lua `mbar.aerospace.*`
    /// in the binary): `Some(Effect::Platform(PlatformRequest::StartAerospace))` the first
    /// time it is needed in this runtime's lifetime (subscriptions to `aerospace_*`
    /// events and `provider=aerospace` request it too; `--query aerospace` does not),
    /// `None` once it was requested. The caller handles the returned effect like any other effect.
    pub fn request_aerospace(&mut self) -> Option<Effect> {
        let mut effects = Vec::new();
        self.start_aerospace(&mut effects);
        effects.pop()
    }

    /// Whether `PlatformRequest::StartAerospace` has been emitted.
    pub fn aerospace_started(&self) -> bool {
        self.model.aerospace.active
    }

    /// Steps animations (`Animator::step` + [`Runtime::apply_anim_steps`]), lays out
    /// (`layout::layout`), updates `associated_bar` bits, and returns scenes for dirty
    /// windows only (`bar_manager_refresh` / `bar_draw` redraw decision, `bar.md` §5).
    pub fn frame(&mut self, now: Instant, res: &mut dyn Resources) -> FrameOutput {
        let t0 = Instant::now();
        self.last_now = Some(now);
        if !self.sleeps && !self.animator.is_empty() {
            let steps = self.animator.step(now);
            if !steps.is_empty() {
                self.apply_anim_steps(steps);
            }
        }
        self.refresh(false, res);
        if self.needs_layout {
            self.run_layout(res);
        }
        if !self.needs_render {
            return FrameOutput::default();
        }
        self.needs_render = false;
        let Some(layout) = self.layout.take() else {
            self.dirty.clear();
            self.popup_member_bars.clear();
            self.space_moves.clear();
            return FrameOutput::default();
        };
        let mut out = FrameOutput::default();
        let props = self.bar_props();

        // Bars. Popup members are listed in a bar's layout (association bits) but painted
        // by the popup window; only their clip holes reach the bar scene, so the comparison
        // ignores the members that punch none (PERF-4).
        let popup_members: HashMap<ItemId, bool> = self
            .model
            .items
            .iter()
            .filter(|i| i.position == Position::Popup)
            .map(|i| (i.id, item_clips_bar(i)))
            .collect();
        for bl in &layout.bars {
            let key = WindowKey::Bar(bl.adid);
            let (painted, popup_clips) = bar_scene_layout(bl, &popup_members);
            let changed = self.dirty.contains(&key)
                || match self.emitted_bars.get(&bl.adid) {
                    Some(e) => {
                        e.props != props
                            || e.layout != painted
                            || (self.popup_member_bars.contains(&bl.adid)
                                && (popup_clips || e.popup_clips))
                    }
                    None => true,
                };
            if !changed {
                continue;
            }
            let scene = layout::bar_scene(&self.model, bl);
            out.windows.push(WindowUpdate {
                key,
                frame: bl.frame,
                level: props.level,
                scene,
                blur_radius: props.blur,
                shadow: props.shadow,
                sticky: props.sticky,
                font_smoothing: props.font_smoothing,
                order: 0,
            });
            *self.redraws.entry(key).or_insert(0) += 1;
            self.emitted_bars.insert(
                bl.adid,
                EmittedBar {
                    layout: painted,
                    props,
                    popup_clips,
                },
            );
        }
        if self.emitted_bars.len() != layout.bars.len()
            || self
                .emitted_bars
                .keys()
                .any(|a| !layout.bars.iter().any(|b| b.adid == *a))
        {
            let gone: Vec<u32> = self
                .emitted_bars
                .keys()
                .copied()
                .filter(|a| !layout.bars.iter().any(|b| b.adid == *a))
                .collect();
            for a in gone {
                self.emitted_bars.remove(&a);
                out.closed.push(WindowKey::Bar(a));
            }
        }

        // Popups.
        for pl in &layout.popups {
            let key = WindowKey::Popup(pl.host);
            let blur = self
                .model
                .item(pl.host)
                .map(|h| h.popup.blur_radius)
                .unwrap_or(0);
            let changed = self.dirty.contains(&key)
                || match self.emitted_popups.get(&pl.host) {
                    Some(e) => e.blur != blur || e.props != props || e.layout != *pl,
                    None => true,
                };
            if !changed {
                continue;
            }
            let scene = layout::popup_scene(&self.model, pl);
            out.windows.push(WindowUpdate {
                key,
                frame: pl.frame,
                level: pl.level,
                scene,
                blur_radius: blur,
                shadow: false,
                sticky: props.sticky,
                font_smoothing: props.font_smoothing,
                order: 1,
            });
            *self.redraws.entry(key).or_insert(0) += 1;
            self.emitted_popups.insert(
                pl.host,
                EmittedPopup {
                    layout: pl.clone(),
                    blur,
                    props,
                },
            );
        }
        if self.emitted_popups.len() != layout.popups.len()
            || self
                .emitted_popups
                .keys()
                .any(|h| !layout.popups.iter().any(|p| p.host == *h))
        {
            let gone: Vec<ItemId> = self
                .emitted_popups
                .keys()
                .copied()
                .filter(|h| !layout.popups.iter().any(|p| p.host == *h))
                .collect();
            for h in gone {
                self.emitted_popups.remove(&h);
                out.closed.push(WindowKey::Popup(h));
            }
        }

        // `bar_change_space`: the bar window and the popups open on that bar.
        for (adid, dsid) in std::mem::take(&mut self.space_moves) {
            if self.emitted_bars.contains_key(&adid) {
                out.space_moves.push(SpaceMove {
                    key: WindowKey::Bar(adid),
                    dsid,
                });
            }
            for (host, e) in &self.emitted_popups {
                if e.layout.adid == adid {
                    out.space_moves.push(SpaceMove {
                        key: WindowKey::Popup(*host),
                        dsid,
                    });
                }
            }
        }

        self.layout = Some(layout);
        self.dirty.clear();
        self.popup_member_bars.clear();
        self.stats.frames += 1;
        self.frame_times.push(t0.elapsed().as_micros() as u64);
        out
    }

    /// Earliest of: routine tick, wake re-post, provider/alias schedules, animation frame.
    /// (Providers are sampled by the platform at their own `freq`; aliases are recaptured on
    /// the routine tick.)
    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadline(None)
    }

    /// [`Runtime::next_deadline`] for a platform without a display link: the animation
    /// deadline is paced to one `frame_interval` after the last animation frame
    /// ([`crate::animation::Animator::next_deadline_paced`]) instead of "now", so a loop
    /// that sleeps until the deadline does not busy-spin while animations run.
    pub fn next_deadline_paced(&self, frame_interval: Duration) -> Option<Instant> {
        self.deadline(Some(frame_interval))
    }

    fn deadline(&self, frame_interval: Option<Duration>) -> Option<Instant> {
        let mut d = self.next_tick;
        let mut min = |x: Option<Instant>| {
            if let Some(x) = x {
                d = Some(d.map_or(x, |cur| cur.min(x)));
            }
        };
        min(self.wake_repost);
        if let Some(now) = self.last_now {
            if !self.sleeps {
                min(match frame_interval {
                    Some(i) => self.animator.next_deadline_paced(now, i),
                    None => self.animator.next_deadline(now),
                });
            }
            if self.needs_render || self.needs_layout {
                min(Some(now));
            }
        }
        d
    }

    /// True if [`Input::Timer`] has work to do at `now` (routine tick or delayed wake
    /// re-post). Unlike [`Runtime::next_deadline`] this ignores render and animation
    /// deadlines, which [`Runtime::frame`] serves.
    pub fn timer_due(&self, now: Instant) -> bool {
        [self.next_tick, self.wake_repost]
            .into_iter()
            .flatten()
            .any(|t| t <= now)
    }

    /// True while animations run (each frame is an event in SketchyBar, `events.md` Q5).
    pub fn animating(&self) -> bool {
        !self.sleeps && !self.animator.is_empty()
    }

    /// `bar_manager_poll_active_display` on its own, for platform-driven events that do not
    /// go through [`Runtime::handle`] (animation frames).
    pub fn poll_display(&mut self, res: &mut dyn Resources) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.poll_active_display(&mut effects, res);
        effects
    }

    /// Records `count` executed Lua callbacks (handlers, `mbar.exec` and `mbar.delay`
    /// callbacks) that took `total_us` together, the slowest `max_us` (`lua` in
    /// `--query stats`).
    pub fn record_lua_callbacks(&mut self, count: u64, total_us: u64, max_us: u64) {
        self.stats.lua_callbacks = self.stats.lua_callbacks.saturating_add(count);
        self.lua_total_us = self.lua_total_us.saturating_add(total_us);
        self.stats.lua_max_us = self.stats.lua_max_us.max(max_us);
    }

    /// `--exit` outside of a message, for a platform that was asked to terminate
    /// (`SIGTERM`/`SIGINT`/`SIGHUP`): the mach helpers get `"k"` (`bar_manager_destroy`),
    /// animations stop and [`Effect::Exit`] is emitted. Later messages are ignored like
    /// the rest of an `--exit` message.
    pub fn exit(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        if !self.exiting {
            self.exit_into(&mut effects);
        }
        effects
    }

    fn exit_into(&mut self, effects: &mut Vec<Effect>) {
        self.send_mach_destroy(effects);
        self.animator.clear();
        effects.push(Effect::Exit);
        self.exiting = true;
    }

    /// Calls `f` for every [`TextKey`] the runtime still references, i.e. every text line
    /// a current or future scene can draw: `icon`, `label` and `slider.knob` of every item
    /// (bar and popup members alike) and of the `--default` template, the measured
    /// `app_menu` titles, and the title lines of the last layout pass. Keys may be
    /// reported more than once. Platforms that cache text runs per key use this to prune
    /// exactly (`TextCache::prune_live` on macOS) instead of evicting by age, so a
    /// long-unchanged label is never dropped while it is still shown.
    pub fn for_each_text_key(&self, f: &mut dyn FnMut(TextKey)) {
        fn item_keys(item: &BarItem, f: &mut dyn FnMut(TextKey)) {
            for text in [&item.icon, &item.label, &item.slider.knob] {
                if let Some(k) = text.line {
                    f(k);
                }
            }
            for cell in &item.app_menu.measured.cells {
                f(cell.key);
            }
        }
        item_keys(&self.model.default_item, f);
        for item in &self.model.items {
            item_keys(item, f);
        }
        if let Some(layout) = &self.layout {
            let bar_lines = layout.bars.iter().flat_map(|b| &b.menu_lines);
            let popup_lines = layout.popups.iter().flat_map(|p| &p.menu_lines);
            for (_, lines) in bar_lines.chain(popup_lines) {
                for line in lines {
                    f(line.key);
                }
            }
        }
    }

    /// Tells the runtime which `--monitor` streams still have subscribers, so it stops
    /// building event lines / stats snapshots nobody reads (extension).
    pub fn set_monitor(&mut self, events: bool, stats: bool) {
        self.monitor_events = events;
        self.monitor_stats = stats;
    }

    /// True if `frame` should run now (dirty windows or running animations).
    pub fn needs_frame(&self) -> bool {
        self.needs_render
            || self.needs_layout
            || self.force_refresh
            || self.model.bar_needs_update
            || self.model.bar_needs_resize
            || (!self.sleeps && self.animator.needs_frame())
    }

    // ------------------------------------------------------------------------------
    // Messages (cli.md §2.5, §3). Calls WP-B: command::parse, command::bre_to_regex,
    // query::query. Calls data model: BarItem::set_prop, BarProps::set_prop, Model::*.
    // ------------------------------------------------------------------------------

    /// One IPC request: parse, execute in order, finish (`cli.md` §2.5). Returns the reply
    /// text (no reply after `--exit`).
    fn handle_message(
        &mut self,
        args: &[String],
        reply: ReplyToken,
        res: &mut dyn Resources,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        let text = self.run_message(args, &mut effects, res);
        let monitor = self.monitor_req.take();
        match (text, monitor) {
            (Some(text), Some(mode)) => effects.push(Effect::MonitorStart { reply, mode, text }),
            (Some(text), None) => effects.push(Effect::Reply { reply, text }),
            (None, _) => {}
        }
        effects
    }

    /// Executes a message; `None` = no reply (`--exit`). A `--monitor` in the message is
    /// left in `monitor_req` for [`Runtime::handle_message`].
    fn run_message(
        &mut self,
        args: &[String],
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) -> Option<String> {
        self.stats.ipc_messages += 1;
        self.anim = None;
        self.no_reply = false;
        self.monitor_req = None;
        let cmds = command::parse(args);
        // Queries see the state as of the last refresh (frames/bounding rects included).
        if self.needs_layout && cmds.iter().any(|c| matches!(c, Command::Query(_))) {
            self.run_layout(res);
        }
        self.frozen = true;
        let mut rsp = String::new();
        let mut refresh = false;
        for cmd in cmds {
            if self.exiting {
                break;
            }
            let is_query = matches!(cmd, Command::Query(_));
            let before = rsp.len();
            refresh |= self.exec(cmd, &mut rsp, effects, res);
            // `provider=aerospace` labels are applied per command, so a later `--query` of
            // the same message sees them.
            self.flush_aerospace_providers(effects, res);
            if !is_query && rsp.len() > before {
                effects.push(Effect::Log(rsp[before..].to_string()));
            }
        }
        if refresh {
            // `bar_needs_resize` is applied by `refresh` (resize before redraw).
            self.model.bar_needs_update = true;
        }
        self.animator.lock_all();
        self.frozen = false;
        self.anim = None;
        if self.no_reply {
            None
        } else {
            Some(rsp)
        }
    }

    /// Executes one command, appending to `rsp`. Returns true if it asked for the
    /// end-of-message refresh flag (`--bar` change, `--update`, `--remove`).
    fn exec(
        &mut self,
        cmd: Command,
        rsp: &mut String,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) -> bool {
        match cmd {
            Command::Set { target, tokens } => {
                let ids = self.select(&target, rsp);
                if !ids.is_empty() {
                    self.exec_set(&ids, &tokens, rsp, effects, res);
                }
                false
            }
            Command::Default { pairs, malformed } => {
                self.exec_default(&pairs, malformed.as_deref(), rsp, effects, res);
                false
            }
            Command::Bar { pairs, malformed } => {
                self.exec_bar(&pairs, malformed.as_deref(), rsp, effects, res)
            }
            Command::Animate { curve, duration } => {
                self.anim = Some(AnimSpec { curve, duration });
                false
            }
            Command::Add(add) => {
                self.exec_add(&add, rsp, effects, res);
                false
            }
            Command::AddEvent { name, notification } => {
                // The AeroSpace events and `privacy_indicator_change` are built in
                // (delivered by the core, aerospace design §Events): the SketchyBar recipe's
                // `--add event aerospace_workspace_change [<notification>]` registers the
                // name as before (same bit, `--trigger` keeps working) but never observes a
                // notification.
                let notification = notification
                    .filter(|_| !aerospace::is_event_name(&name) && name != privacy::EVENT_NAME);
                match self.model.events.append(&name, notification.as_deref()) {
                    AppendResult::Added(_) => {
                        if let Some(n) = notification {
                            effects.push(Effect::Platform(PlatformRequest::ObserveNotification(n)));
                        }
                    }
                    AppendResult::Exists => {}
                    AppendResult::Full => {
                        let _ = write!(rsp, "[!] Event: Too many events '{name}'\n");
                    }
                }
                false
            }
            Command::Clone {
                name,
                parent,
                placement,
            } => {
                self.exec_clone(&name, &parent, placement, rsp, effects, res);
                false
            }
            Command::Subscribe { item, events } => {
                self.exec_subscribe(&item, &events, rsp, effects);
                false
            }
            Command::Push { item, values } => {
                self.exec_push(&item, &values, rsp);
                false
            }
            Command::Update => {
                self.exec_update(effects, res);
                true
            }
            Command::Trigger { event, args } => {
                self.exec_trigger(&event, &args, effects, res);
                false
            }
            Command::Query(target) => {
                self.exec_query(&target, rsp, res);
                false
            }
            Command::Reorder(names) => {
                self.exec_reorder(&names, rsp);
                false
            }
            Command::Move {
                item,
                before,
                reference,
            } => {
                self.exec_move(&item, before, &reference, rsp);
                false
            }
            Command::Remove(sel) => {
                let ids = match &sel {
                    Selector::Name(n) => match self.model.find(n) {
                        Some(id) => vec![id],
                        None => {
                            let _ = write!(rsp, "[!] Remove: Item '{n}' not found\n");
                            Vec::new()
                        }
                    },
                    Selector::Regex(tok) => {
                        self.regex_select(tok, sel.pattern().unwrap_or(""), rsp)
                    }
                };
                for id in ids {
                    self.remove_item(id, effects);
                }
                true
            }
            Command::Rename { old, new } => {
                self.exec_rename(&old, &new, rsp);
                false
            }
            Command::Exit => {
                self.exit_into(effects);
                self.no_reply = true;
                false
            }
            Command::Hotload(tok) => {
                self.hotload = value::parse_bool(&tok, self.hotload);
                effects.push(Effect::Platform(PlatformRequest::SetHotload(self.hotload)));
                false
            }
            Command::LoadFont(path) => {
                effects.push(Effect::Platform(PlatformRequest::LoadFont(path)));
                false
            }
            Command::Reload(path) => {
                self.reload_with_response(path, rsp, effects, res);
                false
            }
            Command::UnknownDomain(tok) => {
                let _ = write!(rsp, "[!] Unknown domain '{tok}'\n");
                false
            }
            Command::Monitor(mode) => {
                match mode {
                    MonitorMode::Events => self.monitor_events = true,
                    MonitorMode::Stats => self.monitor_stats = true,
                    MonitorMode::All => {
                        self.monitor_events = true;
                        self.monitor_stats = true;
                    }
                }
                // The connection stays open for the stream: the reply becomes its first
                // frame (`Effect::MonitorStart`).
                self.monitor_req = Some(match self.monitor_req {
                    Some(prev) if prev != mode => MonitorMode::All,
                    _ => mode,
                });
                false
            }
            Command::Menu(which) => {
                let index = which
                    .parse::<usize>()
                    .ok()
                    .or_else(|| self.menu_titles.iter().position(|t| *t == which));
                match index {
                    Some(index) => {
                        effects.push(Effect::Platform(PlatformRequest::OpenMenu { index }))
                    }
                    None => {
                        let _ = write!(rsp, "[!] Menu: Menu '{which}' not found\n");
                    }
                }
                false
            }
            Command::MenuBar(action) => {
                let hide = match action {
                    Some(MenuBarAction::Hide) => Some(true),
                    Some(MenuBarAction::Show) => Some(false),
                    Some(MenuBarAction::Toggle) => Some(res.menu_bar_visible()),
                    None => None,
                };
                match hide {
                    Some(h) => effects.push(Effect::Platform(PlatformRequest::SetMenuBarHidden(h))),
                    None => {
                        rsp.push_str(
                            "[!] Menubar: Invalid argument, expected 'hide', 'show' or 'toggle'\n",
                        );
                    }
                }
                false
            }
            Command::Borders { pairs, malformed } => {
                self.exec_borders(&pairs, malformed.as_deref(), rsp, effects);
                false
            }
        }
    }

    /// Resolves a selector (`cli.md` §3.5): exact name or BRE over all names in global order;
    /// appends `[!] Set: Item not found '<name>'\n` / regex messages as appropriate (the
    /// caller decides the error prefix for `--remove`).
    fn select(&mut self, sel: &Selector, rsp: &mut String) -> Vec<ItemId> {
        match sel {
            Selector::Name(n) => match self.model.find(n) {
                Some(id) => vec![id],
                None => {
                    let _ = write!(rsp, "[!] Set: Item not found '{n}'\n");
                    Vec::new()
                }
            },
            Selector::Regex(tok) => self.regex_select(tok, sel.pattern().unwrap_or(""), rsp),
        }
    }

    /// Regex selection (`regcomp` BRE, unanchored `regexec` over all names in order).
    /// Compiled patterns are cached (configs re-run the same selectors on every event).
    fn regex_select(&mut self, token: &str, pattern: &str, rsp: &mut String) -> Vec<ItemId> {
        if !self.regex_cache.contains_key(pattern) {
            let Ok(re) = command::compile_bre(pattern) else {
                let _ = write!(rsp, "[!] Regex: Could not compile regex '{token}'\n");
                return Vec::new();
            };
            if self.regex_cache.len() >= REGEX_CACHE_SIZE {
                self.regex_cache.clear();
            }
            self.regex_cache.insert(pattern.to_string(), re);
        }
        let re = &self.regex_cache[pattern];
        let mut ids = Vec::new();
        for item in &self.model.items {
            let Some(name) = item.name.as_deref() else {
                continue;
            };
            match re.is_match(name) {
                Ok(true) => ids.push(item.id),
                Ok(false) => {}
                Err(e) => {
                    let _ = write!(rsp, "[!] Regex: Regex match failed '{e}'\n");
                    return Vec::new();
                }
            }
        }
        if ids.is_empty() {
            let _ = write!(rsp, "[?] Regex: No match found for regex '{token}'\n");
        }
        ids
    }

    /// Runs one property setter on `target` with a fresh `PropCx` (animation setting of the
    /// current message). Returns the result, the non-fatal response text, the global
    /// effects and the requests.
    fn set_prop_on(
        &mut self,
        target: AnimTarget,
        key: &str,
        value: &str,
        res: &mut dyn Resources,
    ) -> (PropResult, String, PropEffects, Vec<PropRequest>) {
        let idx = match target {
            AnimTarget::Item(id) => self.model.index_of(id),
            _ => None,
        };
        self.set_prop_at(target, idx, key, value, res)
    }

    /// [`Runtime::set_prop_on`] with the item's index already resolved (`idx` must hold
    /// the item of an `AnimTarget::Item` target; `None` = item gone).
    fn set_prop_at(
        &mut self,
        target: AnimTarget,
        idx: Option<usize>,
        key: &str,
        value: &str,
        res: &mut dyn Resources,
    ) -> (PropResult, String, PropEffects, Vec<PropRequest>) {
        let anim = self.anim;
        let Runtime {
            model,
            animator,
            config,
            ..
        } = self;
        let mut cx = PropCx::new(res, animator, &config.home);
        cx.anim = anim;
        cx.set_target(target);
        let r = match target {
            AnimTarget::Bar => model.bar.set_prop(key, value, &mut cx),
            AnimTarget::Default => model.default_item.set_prop(key, value, &mut cx),
            AnimTarget::Item(id) => match idx.and_then(|i| model.items.get_mut(i)) {
                Some(item) if item.id == id => item.set_prop(key, value, &mut cx),
                _ => Ok(false),
            },
        };
        let response = std::mem::take(&mut cx.response);
        let fx = cx.fx;
        let requests = std::mem::take(&mut cx.requests);
        (r, response, fx, requests)
    }

    /// `--set`: token-major / item-minor application with `PropCx` (target
    /// `AnimTarget::Item`), malformed-token message naming the first item, `needs_update` on
    /// change, then `apply_requests`.
    fn exec_set(
        &mut self,
        ids: &[ItemId],
        tokens: &[SetToken],
        rsp: &mut String,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        // Index of each selected item: regex selections are in global order, so one pass.
        let mut hints: Vec<Option<usize>> = Vec::with_capacity(ids.len());
        let mut from = 0;
        for &id in ids {
            let found = self.model.items[from.min(self.model.items.len())..]
                .iter()
                .position(|it| it.id == id)
                .map(|p| p + from)
                .or_else(|| self.model.index_of(id));
            if let Some(i) = found {
                from = i + 1;
            }
            hints.push(found);
        }
        for tok in tokens {
            match tok {
                SetToken::Malformed(t) => {
                    let first = self.model.name_of(ids[0]);
                    let _ = write!(
                        rsp,
                        "[!] Set ({}): Expected <key>=<value> pair, but got: '{t}'\n",
                        name_or_null(first.as_deref())
                    );
                }
                SetToken::Pair { key, value } => {
                    for (n, &id) in ids.iter().enumerate() {
                        // Index lookups are O(1) while the items keep their places (setters
                        // that move items, e.g. `position=popup.x`, fall back to a scan).
                        let idx = match hints[n] {
                            Some(i) if self.model.items.get(i).is_some_and(|it| it.id == id) => i,
                            _ => match self.model.index_of(id) {
                                Some(i) => i,
                                None => continue,
                            },
                        };
                        hints[n] = Some(idx);
                        let (r, response, fx, reqs) =
                            self.set_prop_at(AnimTarget::Item(id), Some(idx), key, value, res);
                        rsp.push_str(&response);
                        let changed = match r {
                            Ok(c) => c,
                            Err(e) => {
                                let _ = write!(rsp, "{e}");
                                false
                            }
                        };
                        let unmoved = reqs.is_empty();
                        let out = self.apply_requests(Some(id), reqs, fx, rsp, effects, res);
                        if changed && !out.suppress_update {
                            let item = if unmoved {
                                self.model.items.get_mut(idx)
                            } else {
                                self.model.item_mut(id)
                            };
                            if let Some(item) = item {
                                item.needs_update = true;
                            }
                        }
                    }
                }
            }
        }
    }

    /// `--default` (target `AnimTarget::Default`; malformed pair ends the domain).
    fn exec_default(
        &mut self,
        pairs: &[(String, String)],
        malformed: Option<&str>,
        rsp: &mut String,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        for (k, v) in pairs {
            let (r, response, fx, reqs) = self.set_prop_on(AnimTarget::Default, k, v, res);
            rsp.push_str(&response);
            if let Err(e) = r {
                let _ = write!(rsp, "{e}");
            }
            self.apply_requests(None, reqs, fx, rsp, effects, res);
        }
        if let Some(t) = malformed {
            let _ = write!(
                rsp,
                "[!] Set (default): Expected <key>=<value> pair, but got: '{t}'\n"
            );
        }
    }

    /// `--bar` (target `AnimTarget::Bar`); returns the refresh flag.
    fn exec_bar(
        &mut self,
        pairs: &[(String, String)],
        malformed: Option<&str>,
        rsp: &mut String,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) -> bool {
        let mut refresh = false;
        for (k, v) in pairs {
            let (r, response, fx, reqs) = self.set_prop_on(AnimTarget::Bar, k, v, res);
            rsp.push_str(&response);
            match r {
                Ok(c) => refresh |= c,
                Err(e) => {
                    let _ = write!(rsp, "{e}");
                }
            }
            refresh |= self
                .apply_requests(None, reqs, fx, rsp, effects, res)
                .refresh;
        }
        if let Some(t) = malformed {
            let _ = write!(
                rsp,
                "[!] Bar: Expected <key>=<value> pair, but got: '{t}'\n"
            );
        }
        refresh
    }

    /// `--borders` (extension, `docs/spec/borders.md` BR-IPC-07): applies the pairs to
    /// `model.borders` and sends one `PlatformRequest::SetBorders` when the configuration
    /// changed. A list that is only a malformed token applies nothing (so a typo does not
    /// count as the first, drawing-enabling message).
    fn exec_borders(
        &mut self,
        pairs: &[(String, String)],
        malformed: Option<&str>,
        rsp: &mut String,
        effects: &mut Vec<Effect>,
    ) {
        if !(pairs.is_empty() && malformed.is_some()) {
            if let Some(update) = self.model.borders.apply(pairs, rsp) {
                effects.push(Effect::Platform(PlatformRequest::SetBorders(Box::new(
                    update,
                ))));
            }
        }
        if let Some(t) = malformed {
            let _ = write!(
                rsp,
                "[!] Borders: Expected <key>=<value> pair, but got: '{t}'\n"
            );
        }
    }

    /// Merges setter side effects into the model (and starts media events).
    fn merge_fx(&mut self, fx: PropEffects, effects: &mut Vec<Effect>) {
        self.model.bar_needs_update |= fx.bar_needs_update;
        self.model.bar_needs_resize |= fx.bar_needs_resize;
        self.model.might_need_clipping |= fx.might_need_clipping;
        if fx.begin_media_events && !self.listeners.media {
            self.listeners.media = true;
            effects.push(Effect::Platform(PlatformRequest::StartMediaEvents));
        }
    }

    /// Executes `PropRequest`s pushed by a setter on item `id` (or the bar / default item
    /// when `None`).
    fn apply_requests(
        &mut self,
        id: Option<ItemId>,
        reqs: Vec<PropRequest>,
        fx: PropEffects,
        rsp: &mut String,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) -> ReqOutcome {
        self.merge_fx(fx, effects);
        let mut out = ReqOutcome::default();
        for r in reqs {
            match r {
                PropRequest::RemoveFromParentPopup => {
                    if let Some(id) = id {
                        if let Some(p) = self.model.item(id).and_then(|i| i.parent) {
                            self.popup_remove_item(p, id);
                        }
                    }
                }
                PropRequest::AddToPopup { host } => match self.model.find(&host) {
                    Some(h) => {
                        if let Some(id) = id {
                            self.popup_add_item(h, id);
                        }
                    }
                    None => {
                        let name = match id {
                            Some(id) => self.model.name_of(id),
                            None => self.model.default_item.name.clone(),
                        };
                        let _ = write!(
                            rsp,
                            "[!] Item Position ({}): Item '{host}' is not a valid popup host\n",
                            name_or_null(name.as_deref())
                        );
                        out.suppress_update = true;
                    }
                },
                PropRequest::ResetDefaultItem => self.model.default_item.reset_default(),
                PropRequest::ProviderChanged => {
                    if let Some(id) = id {
                        self.configure_provider(id, effects);
                    }
                }
                PropRequest::BarHidden(HiddenRequest::Current) => {
                    let adid = self.model.active_adid;
                    if adid >= 1 && adid as usize <= self.model.bars.len() {
                        let hidden = !self.model.bars[adid as usize - 1].hidden;
                        self.set_hidden(Some(adid), hidden);
                        out.refresh = true;
                    } else {
                        effects.push(Effect::Log(format!("No bar on display {adid} \n")));
                    }
                }
                PropRequest::BarHidden(HiddenRequest::All(h)) => {
                    self.set_hidden(None, h);
                    out.refresh = true;
                }
                PropRequest::ResetBars => {
                    self.reset_bars(res);
                    out.refresh = true;
                }
                PropRequest::MenuBarHidden(h) => {
                    effects.push(Effect::Platform(PlatformRequest::SetMenuBarHidden(h)));
                }
                PropRequest::StartPrivacyIndicator => self.start_privacy(effects),
            }
        }
        out
    }

    /// `--add` (`cli.md` §6.1 / `item.md` §2.1) incl. types, positions, popup hosts, graph /
    /// slider widths, alias setup (`PlatformRequest::CaptureAlias`, screen-capture request),
    /// brackets (`group::add_member`, D8 missing member skipped), `app_menu` (registers
    /// `menus_change` on first use).
    fn exec_add(
        &mut self,
        add: &AddCommand,
        rsp: &mut String,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let name = add.name.as_str();
        if self.model.find(name).is_some() {
            let _ = write!(rsp, "[?] Add: Item '{name}' already exists\n");
            return;
        }
        let id = self.model.create_item(res);
        let (t, known) = ItemType::from_add_token(&add.item_type);
        if !known {
            let _ = write!(
                rsp,
                "[?] Add {name}: Invalid type '{}', assuming 'item'\n",
                add.item_type
            );
        }
        let home = self.config.home.clone();
        {
            let Some(item) = self.model.item_mut(id) else {
                return;
            };
            item.set_type(t, &home);
            if t != ItemType::Bracket && item.set_position(&add.position).is_none() {
                let _ = write!(rsp, "[!] Add {name}: Illegal position '{}'\n", add.position);
                self.remove_item(id, effects);
                return;
            }
        }
        if !self.model.item_mut(id).is_some_and(|i| i.set_name(name)) {
            let _ = write!(rsp, "[!] Add: Illegal name '{name}'\n");
            self.remove_item(id, effects);
            return;
        }

        if !add.item_type.is_empty() && add.item_type != "item" {
            let width = add.args.first().map(|w| value::parse_u32(w)).unwrap_or(0);
            match t {
                ItemType::Graph => {
                    if let Some(item) = self.model.item_mut(id) {
                        item.graph.setup(width);
                    }
                }
                ItemType::Slider => {
                    if let Some(item) = self.model.item_mut(id) {
                        item.slider.setup(width);
                    }
                }
                ItemType::Alias => {
                    if let Some(item) = self.model.item_mut(id) {
                        item.alias.setup(name);
                        let owner = item.alias.owner.clone();
                        let alias_name = item.alias.name.clone();
                        effects.push(Effect::Platform(PlatformRequest::RequestScreenCapture));
                        if item.alias.update_frequency != 0 && !self.capture_disabled {
                            effects.push(Effect::Platform(PlatformRequest::CaptureAlias {
                                item: id,
                                owner,
                                name: alias_name,
                                forced: true,
                            }));
                        }
                    }
                }
                ItemType::Bracket => {
                    let mut first_resolved = false;
                    let members = std::iter::once(add.position.as_str())
                        .chain(add.args.iter().map(String::as_str));
                    for tok in members {
                        if tok.is_empty() {
                            continue;
                        }
                        let resolved = match Selector::parse(tok) {
                            sel @ Selector::Regex(_) => {
                                self.regex_select(tok, sel.pattern().unwrap_or(""), rsp)
                            }
                            Selector::Name(n) => match self.model.find(&n) {
                                Some(m) => vec![m],
                                None => {
                                    let _ = write!(
                                        rsp,
                                        "[?] Add (Group) {name}: Failed to add member '{tok}', item not found\n"
                                    );
                                    Vec::new()
                                }
                            },
                        };
                        if resolved.is_empty() {
                            continue;
                        }
                        if !first_resolved {
                            first_resolved = true;
                            let first = self.model.item(resolved[0]);
                            if let Some(f) = first {
                                if f.position == Position::Popup {
                                    let parent = f.parent;
                                    if let Some(b) = self.model.item_mut(id) {
                                        b.position = Position::Popup;
                                    }
                                    if let Some(p) = parent {
                                        self.popup_add_item(p, id);
                                    }
                                }
                            }
                        }
                        for m in resolved {
                            group::add_member(&mut self.model, id, m);
                        }
                    }
                }
                ItemType::AppMenu => {
                    if self.model.events.flag("menus_change").is_none() {
                        self.model.events.append("menus_change", None);
                    }
                    let app = self.menu_app.clone();
                    let titles = self.menu_titles.clone();
                    if let Some(item) = self.model.item_mut(id) {
                        item.app_menu.app_name = app;
                        item.app_menu.titles = titles;
                    }
                }
                _ => {}
            }
        }

        // Popup host (`p….<host>`; for brackets the first member token, quirk).
        let pos = add.position.as_str();
        if pos.starts_with('p') {
            if let Some((_, host)) = pos.split_once('.') {
                if !host.is_empty() {
                    match self.model.find(host) {
                        None => {
                            let _ = write!(
                                rsp,
                                "[!] Add (Popup) {name}: Item '{host}' is not a valid popup host\n"
                            );
                            self.remove_item(id, effects);
                            return;
                        }
                        Some(h) => self.popup_add_item(h, id),
                    }
                }
            }
        }

        if let Some(item) = self.model.item_mut(id) {
            item.needs_update = true;
            if item.provider.kind.is_some() {
                self.configure_provider(id, effects);
            }
        }
    }

    /// `--clone` (`item.md` §10.1; D6/D7; popup members not attached).
    fn exec_clone(
        &mut self,
        name: &str,
        parent: &str,
        placement: Option<Placement>,
        rsp: &mut String,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let Some(pid) = self.model.find(parent) else {
            let _ = write!(rsp, "[!] Clone: Parent Item '{parent}' not found\n");
            return;
        };
        if self.model.find(name).is_some() {
            let _ = write!(rsp, "[?] Clone: Item '{name}' already exists\n");
            return;
        }
        if name.is_empty() {
            // C creates an item with a NULL name; mbar rejects it (cli.md §6.8).
            return;
        }
        let id = self.model.create_item(res);
        let Some(ancestor) = self.model.item(pid).cloned() else {
            return;
        };
        if let Some(item) = self.model.item_mut(id) {
            item.inherit_from(&ancestor, res);
            item.set_name(name);
            item.needs_update = true;
        }
        match placement {
            Some(Placement::Before) => self.move_item(id, pid, true),
            Some(Placement::After) => self.move_item(id, pid, false),
            None => {}
        }
        if self
            .model
            .item(id)
            .is_some_and(|i| i.provider.kind.is_some())
        {
            self.configure_provider(id, effects);
        }
    }

    /// `bar_manager_remove_item` (`item.md` §10.5): popup lists, brackets, popup children,
    /// D18 `animator.cancel_target`, provider stop, alias capture stop
    /// ([`PlatformRequest::RemoveAlias`]; an item's type never changes after `--add`, so
    /// removal is the only way an alias item goes away).
    fn remove_item(&mut self, id: ItemId, effects: &mut Vec<Effect>) {
        let Some(idx) = self.model.index_of(id) else {
            return;
        };
        // Remove it from every popup list (closing popups that become empty).
        for it in &mut self.model.items {
            if it.popup.items.contains(&id) {
                it.popup.items.retain(|m| *m != id);
                if it.popup.items.is_empty() {
                    it.popup.frame = None;
                }
            }
        }
        if self.model.items[idx].is_bracket() {
            group::destroy_group(&mut self.model, id);
        }
        let brackets: Vec<ItemId> = self
            .model
            .items
            .iter()
            .filter(|i| i.is_bracket() && i.bracket_members.contains(&id))
            .map(|i| i.id)
            .collect();
        for b in brackets {
            group::remove_member(&mut self.model, b, id);
        }
        // Popup children are detached (cli.md §6.4: C leaves a dangling parent).
        for it in &mut self.model.items {
            if it.parent == Some(id) {
                it.parent = None;
            }
        }
        self.animator.cancel_target(AnimTarget::Item(id));
        let Some(idx) = self.model.index_of(id) else {
            return;
        };
        let item = self.model.items.remove(idx);
        if item.provider.kind.is_some_and(|k| !k.is_core()) {
            effects.push(Effect::Platform(PlatformRequest::StopProvider { item: id }));
        }
        if item.has_alias() {
            effects.push(Effect::Platform(PlatformRequest::RemoveAlias { item: id }));
        }
        self.model.needs_ordering = true;
        self.model.bar_needs_update = true;
    }

    /// Moves `id` directly before/after `reference` in the global order.
    fn move_item(&mut self, id: ItemId, reference: ItemId, before: bool) {
        if id == reference {
            return;
        }
        let Some(idx) = self.model.index_of(id) else {
            return;
        };
        let mut item = self.model.items.remove(idx);
        item.needs_update = true;
        let Some(r) = self.model.index_of(reference) else {
            self.model.items.insert(idx, item);
            return;
        };
        let at = if before { r } else { r + 1 };
        self.model.items.insert(at, item);
        self.model.needs_ordering = true;
    }

    /// `--move` (D8: self-move is a no-op), `--reorder` (D8: duplicates → first wins),
    /// `--rename`, `--push` (D5).
    fn exec_move(&mut self, item: &str, before: bool, reference: &str, rsp: &mut String) {
        match (self.model.find(item), self.model.find(reference)) {
            (Some(a), Some(b)) => self.move_item(a, b, before),
            _ => {
                let _ = write!(rsp, "[!] Move: Item '{item}' or '{reference}' not found\n");
            }
        }
    }
    fn exec_reorder(&mut self, names: &[String], rsp: &mut String) {
        let mut order: Vec<ItemId> = Vec::new();
        for n in names {
            match self.model.find(n) {
                Some(id) => {
                    if !order.contains(&id) {
                        order.push(id);
                    }
                }
                None => {
                    let _ = write!(rsp, "[!] Order: Item '{n}' not found\n");
                }
            }
        }
        if order.is_empty() {
            return;
        }
        let slots: Vec<usize> = self
            .model
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| order.contains(&i.id))
            .map(|(k, _)| k)
            .collect();
        let original: Vec<ItemId> = slots.iter().map(|&s| self.model.items[s].id).collect();
        let mut old: Vec<Option<BarItem>> = std::mem::take(&mut self.model.items)
            .into_iter()
            .map(Some)
            .collect();
        let mut taken: HashMap<ItemId, BarItem> = HashMap::new();
        for &s in &slots {
            if let Some(it) = old[s].take() {
                taken.insert(it.id, it);
            }
        }
        for (k, &s) in slots.iter().enumerate() {
            if let Some(mut it) = taken.remove(&order[k]) {
                if it.id != original[k] {
                    it.needs_update = true;
                }
                old[s] = Some(it);
            }
        }
        self.model.items = old.into_iter().flatten().collect();
        self.model.needs_ordering = true;
    }
    fn exec_rename(&mut self, old: &str, new: &str, rsp: &mut String) {
        let src = self.model.find(old);
        if src.is_none() || self.model.find(new).is_some() {
            let _ = write!(rsp, "[!] Rename: Failed to rename item: {old} -> {new}\n");
            return;
        }
        if let Some(item) = src.and_then(|id| self.model.item_mut(id)) {
            item.set_name(new);
        }
    }
    fn exec_push(&mut self, item: &str, values: &[f32], rsp: &mut String) {
        let Some(id) = self.model.find(item) else {
            let _ = write!(rsp, "[!] Push: Item '{item}' not found\n");
            return;
        };
        let Some(it) = self.model.item_mut(id) else {
            return;
        };
        if !it.has_graph() {
            let _ = write!(rsp, "[!] Push: Item '{item}' not a graph\n");
            return;
        }
        for v in values {
            it.graph.push(*v);
        }
        it.needs_update = true;
    }

    /// `popup_add_item` / `popup_remove_item` (`item.md` §6.1).
    fn popup_add_item(&mut self, host: ItemId, item: ItemId) {
        if self
            .model
            .item(host)
            .map_or(true, |h| h.popup.items.contains(&item))
        {
            return;
        }
        if let Some(old) = self.model.item(item).and_then(|i| i.parent) {
            if old != host {
                self.popup_remove_item(old, item);
            }
        }
        if let Some(h) = self.model.item_mut(host) {
            h.popup.items.push(item);
            h.popup.needs_ordering = true;
            h.needs_update = true;
        }
        if let Some(i) = self.model.item_mut(item) {
            i.parent = Some(host);
        }
    }
    fn popup_remove_item(&mut self, host: ItemId, item: ItemId) {
        if let Some(h) = self.model.item_mut(host) {
            if !h.popup.items.contains(&item) {
                return;
            }
            h.popup.items.retain(|m| *m != item);
            if h.popup.items.is_empty() {
                // The window is closed; `popup.drawing` stays on.
                h.popup.frame = None;
            }
            h.needs_update = true;
        }
    }

    /// `--query` (calls `query::query` with a `QueryCx`). `--query aerospace` only reports:
    /// it never starts the AeroSpace connection (mbar.app polls it).
    fn exec_query(&mut self, target: &QueryTarget, rsp: &mut String, res: &mut dyn Resources) {
        if matches!(target, QueryTarget::Stats) {
            self.fill_stats();
        }
        let extras = if matches!(target, QueryTarget::DefaultMenuItems) {
            res.menu_extras()
        } else {
            None
        };
        let menus_allowed = !matches!(target, QueryTarget::Menus) || res.accessibility_trusted();
        let cx = QueryCx {
            model: &self.model,
            displays: res.displays(),
            menu_extras: extras.as_deref(),
            stats: &self.stats,
            menus: menus_allowed.then_some(self.menu_titles.as_slice()),
        };
        rsp.push_str(&query::query(target, &cx));
    }

    /// `--reload [<path>]` inside a message (`[?] Reload: Invalid config path` on a bad path).
    fn reload_with_response(
        &mut self,
        path: Option<String>,
        rsp: &mut String,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let mut resolved = None;
        if let Some(p) = &path {
            match std::fs::canonicalize(p) {
                Ok(abs) => {
                    let abs = abs.to_string_lossy().into_owned();
                    self.config.config_path = Some(abs.clone());
                    resolved = Some(abs);
                }
                Err(_) => {
                    let _ = write!(rsp, "[?] Reload: Invalid config path '{p}'\n");
                    return;
                }
            }
        }
        self.reload(resolved, effects, res);
    }

    /// `--reload` / hotload (`cli.md` §11): mach helpers get `"k"`, animations dropped, model
    /// re-initialised (events back to built-ins, listeners kept; the borders configuration is
    /// carried over, as JankyBorders was a separate process unaffected by bar reloads, so the
    /// borders a window manager's launch line set survive; the re-run config applies its
    /// `--borders` keys on top; the AeroSpace state and status are carried over as the
    /// connection survives and AeroSpace does not resend its initial state), bars recreated,
    /// `Effect::RunConfig`.
    fn reload(&mut self, path: Option<String>, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        self.send_mach_destroy(effects);
        for it in &self.model.items {
            if it.provider.kind.is_some_and(|k| !k.is_core()) {
                effects.push(Effect::Platform(PlatformRequest::StopProvider {
                    item: it.id,
                }));
            }
            if it.has_alias() {
                effects.push(Effect::Platform(PlatformRequest::RemoveAlias {
                    item: it.id,
                }));
            }
        }
        self.animator.clear();
        let borders = std::mem::take(&mut self.model.borders);
        let aerospace = std::mem::take(&mut self.model.aerospace);
        let privacy = std::mem::take(&mut self.model.privacy);
        self.model = Model::new();
        self.model.borders = borders;
        self.model.aerospace = aerospace;
        self.model.privacy = privacy;
        self.aerospace_pending.clear();
        self.aerospace_applied.clear();
        self.initial_events.clear();
        // The re-run (Lua) config registers its item-less handlers again.
        self.global_handlers.clear();
        self.anim = None;
        self.sleeps = false;
        self.force_refresh = false;
        self.dirty.clear();
        let now = res.now();
        self.next_tick = Some(now + CLOCK_PERIOD);
        self.begin_bars(res);
        effects.push(Effect::RunConfig { path });
    }

    /// `"k\0"` to every mach helper (`bar_manager_destroy`).
    fn send_mach_destroy(&self, effects: &mut Vec<Effect>) {
        for it in &self.model.items {
            if let Some(service) = &it.mach_helper {
                effects.push(Effect::Platform(PlatformRequest::MachSend {
                    service: service.clone(),
                    payload: MACH_HELPER_DESTROY.to_vec(),
                }));
            }
        }
    }

    // ------------------------------------------------------------------------------
    // Events & scripts (events.md §3–§9). Calls WP-D: script::build_update_env,
    // script::build_click_script_env, script::serialize_for_mach, event::* builders,
    // animation::marquee.
    // ------------------------------------------------------------------------------

    /// Starts the marquee of icon/label(/knob) of the item at `idx` (`text_animate_scroll`).
    fn start_marquee(&mut self, idx: usize) {
        let item = &self.model.items[idx];
        let target = AnimTarget::Item(item.id);
        let mut sets = Vec::new();
        if let Some(a) = animation::marquee(target, "icon.", &item.icon) {
            sets.push(a);
        }
        if let Some(a) = animation::marquee(target, "label.", &item.label) {
            sets.push(a);
        }
        if item.has_slider() {
            if let Some(a) = animation::marquee(target, "slider.knob.", &item.slider.knob) {
                sets.push(a);
            }
        }
        if sets.is_empty() {
            return;
        }
        for set in sets {
            for (i, p) in set.into_iter().enumerate() {
                if i != 1 {
                    self.animator.cancel_locked(p.target, &p.path);
                }
                self.animator.add(p);
            }
        }
        // Quirk Q8: the marquee resets `--animate` for the rest of the message.
        self.anim = None;
    }

    /// `bar_item_update(item, sender, forced, env)` (`events.md` §4.2): counter/marquee
    /// handling, gating (`updates`, `update_freq`, `when_shown`), env building (D1), script
    /// spawn, mach helper send, Lua handler call. Returns true if something was run.
    fn update_item(
        &mut self,
        id: ItemId,
        sender: Option<Sender>,
        forced: bool,
        env: Option<&EnvVars>,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some(idx) = self.model.index_of(id) else {
            return false;
        };
        let (is_shown, scroll, counter) = {
            let it = &self.model.items[idx];
            (it.associated_bar != 0, it.scroll_texts, it.counter)
        };
        if is_shown && scroll && counter % 15 == 0 {
            self.start_marquee(idx);
        }
        let item = &mut self.model.items[idx];
        item.counter = item.counter.wrapping_add(1);
        if (!item.updates || (item.update_frequency == 0 && sender.is_none())) && !forced {
            return false;
        }
        let scheduled = item.update_frequency <= item.counter;
        let should = if item.updates_only_when_shown {
            is_shown
        } else {
            true
        };
        if !(((scheduled || sender.is_some()) && should) || forced) {
            return false;
        }
        item.counter = 0;
        let lua = item
            .lua_handler
            .or_else(|| item.script.as_deref().and_then(lua_id));
        let script = item.script.clone().filter(|s| !s.is_empty());
        let mach = item.mach_helper.clone();
        if lua.is_none() && script.is_none() && mach.is_none() {
            return false;
        }
        let sender = sender.unwrap_or(if forced {
            Sender::Forced
        } else {
            Sender::Routine
        });
        let name = item.name.clone();
        let env = script::build_update_env(&mut item.env, env, name.as_deref(), &sender);
        let mach_payload = mach.map(|service| (service, script::serialize_for_mach(&env)));
        if let Some(h) = lua {
            effects.push(Effect::LuaCallback {
                handler: h,
                env: env.into_vec(),
            });
        } else if let Some(script) = script {
            self.note_script_spawn(name.as_deref());
            effects.push(Effect::RunScript {
                script,
                env: env.into_vec(),
                item: name,
            });
        }
        if let Some((service, payload)) = mach_payload {
            effects.push(Effect::Platform(PlatformRequest::MachSend {
                service,
                payload,
            }));
        }
        true
    }

    /// Runs a `click_script` (shell or `lua:<id>`).
    fn run_click_script(&mut self, id: ItemId, env: EnvVars, effects: &mut Vec<Effect>) {
        let Some(item) = self.model.item(id) else {
            return;
        };
        let Some(cs) = item.click_script.clone().filter(|s| !s.is_empty()) else {
            return;
        };
        let name = item.name.clone();
        if let Some(h) = lua_id(&cs) {
            effects.push(Effect::LuaCallback {
                handler: h,
                env: env.into_vec(),
            });
        } else {
            self.note_script_spawn(name.as_deref());
            effects.push(Effect::RunScript {
                script: cs,
                env: env.into_vec(),
                item: name,
            });
        }
    }

    fn note_script_spawn(&mut self, name: Option<&str>) {
        self.stats.scripts_spawned += 1;
        let key = name.unwrap_or("(null)");
        let now = self.last_now;
        let st = match self.scripts.get_mut(key) {
            Some(s) => s,
            None => self.scripts.entry(key.to_string()).or_default(),
        };
        st.runs += 1;
        if let Some(now) = now {
            if st.pending.len() >= PENDING_SCRIPTS_CAP {
                st.pending.pop_front();
            }
            st.pending.push_back(now);
        }
    }

    fn script_finished(&mut self, item: Option<String>, now: Instant) {
        self.scripts_finished += 1;
        let Some(name) = item else { return };
        let Some(st) = self.scripts.get_mut(&name) else {
            return;
        };
        st.finished += 1;
        if let Some(start) = st.pending.pop_front() {
            let ms = now.saturating_duration_since(start).as_secs_f64() * 1000.0;
            st.total_ms += ms;
            st.max_ms = st.max_ms.max(ms);
            self.script_total_ms += ms;
            self.script_finished_timed += 1;
        }
    }

    fn count_event(&mut self, name: &str) {
        match self.stats.events.get_mut(name) {
            Some(c) => *c += 1,
            None => {
                self.stats.events.insert(name.to_string(), 1);
            }
        }
    }

    /// One `--monitor` event line (extension).
    fn monitor_event(
        &self,
        name: &str,
        sender: &str,
        info: Option<&str>,
        items: &[String],
    ) -> Effect {
        let info = match info {
            Some(s) => serde_json::from_str::<serde_json::Value>(s)
                .ok()
                .filter(|v| v.is_object() || v.is_array())
                .unwrap_or_else(|| serde_json::Value::String(s.to_string())),
            None => serde_json::Value::String(String::new()),
        };
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let v = serde_json::json!({
            "type": "event",
            "name": name,
            "sender": sender,
            "info": info,
            "items": items,
            "ts_ms": ts_ms,
        });
        Effect::Monitor(v.to_string())
    }

    /// Delivers an event to one item directly (mouse events), with stats and monitor.
    fn deliver(
        &mut self,
        id: ItemId,
        name: &str,
        forced: bool,
        env: Option<&EnvVars>,
        effects: &mut Vec<Effect>,
    ) {
        self.count_event(name);
        let ran = self.update_item(
            id,
            Some(Sender::Event(name.to_string())),
            forced,
            env,
            effects,
        );
        if self.monitor_events {
            let items: Vec<String> = if ran {
                self.model.name_of(id).into_iter().collect()
            } else {
                Vec::new()
            };
            let info = env.and_then(|e| e.get("INFO"));
            let line = self.monitor_event(name, name, info, &items);
            effects.push(line);
        }
    }

    /// `bar_manager_custom_events_trigger(name, env)`: every subscribed item in global order,
    /// non-forced (D16: fresh env per item).
    fn trigger_event(&mut self, ev: EventInfo, effects: &mut Vec<Effect>) {
        self.count_event(&ev.name);
        let mut ran: Vec<String> = Vec::new();
        if let Some(flag) = self.model.events.flag(&ev.name) {
            let mut idx = 0;
            while idx < self.model.items.len() {
                let item = &self.model.items[idx];
                idx += 1;
                if !item.update_mask.contains(flag) {
                    continue;
                }
                let id = item.id;
                let r = self.update_item(
                    id,
                    Some(Sender::Event(ev.name.clone())),
                    false,
                    ev.env.as_ref(),
                    effects,
                );
                if r && self.monitor_events {
                    if let Some(n) = self.model.name_of(id) {
                        ran.push(n);
                    }
                }
            }
        }
        self.run_global_handlers(&ev.name, ev.env.as_ref(), effects);
        if self.monitor_events {
            let info = ev.env.as_ref().and_then(|e| e.get("INFO"));
            let line = self.monitor_event(&ev.name, &ev.name, info, &ran);
            effects.push(line);
        }
    }

    /// Triggers `name` with `INFO=info`.
    fn trigger_info(&mut self, name: &str, info: String, effects: &mut Vec<Effect>) {
        let mut env = EnvVars::new();
        env.set("INFO", info);
        self.trigger_event(EventInfo::new(name, Some(env)), effects);
    }

    /// `--subscribe` (`events.md` §3.3): bits, lazy listeners (`PlatformRequest::Start*`).
    fn exec_subscribe(
        &mut self,
        item: &str,
        events: &[String],
        rsp: &mut String,
        effects: &mut Vec<Effect>,
    ) {
        let Some(id) = self.model.find(item) else {
            let _ = write!(rsp, "[!] Subscribe: Item not found '{item}'\n");
            return;
        };
        for ev in events {
            let builtin: Option<&'static str> = aerospace::EVENT_NAMES
                .iter()
                .copied()
                .find(|n| *n == ev.as_str())
                .or_else(|| (ev == privacy::EVENT_NAME).then_some(privacy::EVENT_NAME));
            if let Some(name) = builtin {
                // Built-in events: registered on first use (same registry entry as
                // `--add event`), and their source is started.
                if self.model.events.flag(ev).is_none() {
                    self.model.events.append(ev, None);
                }
                if name == privacy::EVENT_NAME {
                    self.start_privacy(effects);
                } else {
                    self.start_aerospace(effects);
                }
            }
            let Some(flag) = self.model.events.flag(ev) else {
                let _ = write!(rsp, "[?] Event: '{ev}' not found\n");
                continue;
            };
            match EventKind::from_name(ev) {
                Some(EventKind::VolumeChange) if !self.listeners.volume => {
                    self.listeners.volume = true;
                    effects.push(Effect::Platform(PlatformRequest::StartVolumeEvents));
                }
                Some(EventKind::BrightnessChange) if !self.listeners.brightness => {
                    self.listeners.brightness = true;
                    effects.push(Effect::Platform(PlatformRequest::StartBrightnessEvents));
                }
                Some(EventKind::MediaChange) if !self.listeners.media => {
                    self.listeners.media = true;
                    effects.push(Effect::Platform(PlatformRequest::StartMediaEvents));
                }
                Some(EventKind::SpaceWindowsChange) if !self.listeners.space_windows => {
                    self.listeners.space_windows = true;
                    effects.push(Effect::Platform(PlatformRequest::StartSpaceWindowEvents));
                }
                _ => {}
            }
            if let Some(it) = self.model.item_mut(id) {
                let new = !it.update_mask.contains(flag);
                it.update_mask.insert(flag);
                if let Some(name) = builtin.filter(|_| new) {
                    // The state may have arrived before this item existed (shell loops,
                    // `--reload`): deliver what is known.
                    self.queue_initial(Listener::Item(id), name);
                }
            }
        }
    }

    /// `--trigger` (`events.md` §7): forced OS handlers for the built-ins listed in
    /// `event::is_forced_trigger`, else a custom trigger with `event::trigger_env`.
    fn exec_trigger(
        &mut self,
        event: &str,
        args: &[String],
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        if !event::is_forced_trigger(event) {
            let env = event::trigger_env(args);
            if event == aerospace::EVENT_NAMES[0] {
                self.manual_aerospace_trigger(&env);
            }
            self.trigger_event(EventInfo::new(event, Some(env)), effects);
            return;
        }
        match event {
            "space_change" => self.handle_space_change(true, effects, res),
            "display_change" => self.handle_display_change(effects, res),
            "space_windows_change" => self.forced_space_windows(effects, res),
            "volume_change" => self.forced_volume(effects, res),
            "media_change" => self.forced_media(effects),
            "wifi_change" => self.forced_wifi(effects, res),
            "power_source_change" => self.forced_power(effects, res),
            _ => {}
        }
    }

    fn forced_wifi(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let ssid = match res.query_system(SystemQuery::Wifi) {
            Some(SystemValue::Text(s)) => s,
            _ => String::new(),
        };
        self.trigger_info("wifi_change", ssid, effects);
    }

    fn forced_volume(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let v = match res.query_system(SystemQuery::Volume) {
            Some(SystemValue::Level(v)) => v,
            _ => 0.0,
        };
        self.trigger_info("volume_change", event::level_info(v), effects);
    }

    fn forced_brightness(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let v = match res.query_system(SystemQuery::Brightness) {
            Some(SystemValue::Level(v)) => v,
            _ => 0.0,
        };
        self.trigger_info("brightness_change", event::level_info(v), effects);
    }

    fn forced_power(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        if let Some(SystemValue::Power(p)) = res.query_system(SystemQuery::PowerSource) {
            self.trigger_info("power_source_change", p.as_str().to_string(), effects);
        }
    }

    fn forced_front_app(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        if let Some(SystemValue::Text(name)) = res.query_system(SystemQuery::FrontApp) {
            self.trigger_info("front_app_switched", name, effects);
        }
    }

    fn forced_media(&mut self, effects: &mut Vec<Effect>) {
        if self.listeners.media {
            effects.push(Effect::Platform(PlatformRequest::RefreshMedia));
        }
    }

    fn forced_space_windows(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        if !self.listeners.space_windows {
            return;
        }
        if let Some(SystemValue::SpaceWindows(infos)) = res.query_system(SystemQuery::SpaceWindows)
        {
            for info in infos {
                self.trigger_info("space_windows_change", info, effects);
            }
        }
    }

    /// `--update` = `bar_manager_update(forced=true)` (`events.md` §8.2).
    fn exec_update(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        if self.sleeps {
            return;
        }
        self.handle_space_change(true, effects, res);
        self.forced_wifi(effects, res);
        self.forced_volume(effects, res);
        self.forced_brightness(effects, res);
        self.forced_power(effects, res);
        self.forced_front_app(effects, res);
        self.forced_media(effects);
        self.forced_space_windows(effects, res);
        self.count_event("forced");
        let mut ran = Vec::new();
        let mut idx = 0;
        while idx < self.model.items.len() {
            let id = self.model.items[idx].id;
            idx += 1;
            if self.update_item(id, None, true, None, effects) && self.monitor_events {
                if let Some(n) = self.model.name_of(id) {
                    ran.push(n);
                }
            }
        }
        if self.monitor_events {
            let line = self.monitor_event("forced", "forced", None, &ran);
            effects.push(line);
        }
        self.force_refresh = true;
    }

    /// Input::Timer: routine tick and delayed wake.
    fn on_timer(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let now = res.now();
        if let Some(t) = self.wake_repost {
            if now >= t {
                self.wake_repost = None;
                self.system_woke(effects, res);
            }
        }
        if let Some(t) = self.next_tick {
            if now >= t {
                self.routine_tick(effects, res);
                // Missed fires are skipped, not caught up (CFRunLoopTimer semantics).
                let mut next = t + CLOCK_PERIOD;
                while next <= now {
                    next += CLOCK_PERIOD;
                }
                self.next_tick = Some(next);
            }
        }
    }

    /// 1 s routine clock (`bar_manager_update(false)`, `events.md` §8.1): routine updates,
    /// marquee starts, alias recapture requests (`Alias::tick`).
    fn routine_tick(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = res;
        if self.frozen || self.sleeps {
            return;
        }
        self.count_event("routine");
        let mut ran = Vec::new();
        let mut idx = 0;
        while idx < self.model.items.len() {
            let id = self.model.items[idx].id;
            if self.update_item(id, None, false, None, effects) && self.monitor_events {
                if let Some(n) = self.model.name_of(id) {
                    ran.push(n);
                }
            }
            let capture_disabled = self.capture_disabled;
            let item = &mut self.model.items[idx];
            if item.has_alias() && item.is_shown() && item.alias.tick(false) && !capture_disabled {
                effects.push(Effect::Platform(PlatformRequest::CaptureAlias {
                    item: id,
                    owner: item.alias.owner.clone(),
                    name: item.alias.name.clone(),
                    forced: false,
                }));
            }
            idx += 1;
        }
        if self.monitor_events && !ran.is_empty() {
            let line = self.monitor_event("routine", "routine", None, &ran);
            effects.push(line);
        }
        if self.monitor_stats {
            self.fill_stats();
            let text = query::stats_json(&self.stats, self.model.items.len());
            // One compact line, `"type"` first (key order of the rest is serde's).
            let line = match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v @ serde_json::Value::Object(_)) => {
                    let compact = v.to_string();
                    if compact.len() > 2 {
                        format!("{{\"type\":\"stats\",{}", &compact[1..])
                    } else {
                        "{\"type\":\"stats\"}".to_string()
                    }
                }
                _ => "{\"type\":\"stats\"}".to_string(),
            };
            effects.push(Effect::Monitor(line));
        }
    }

    /// OS notifications (`events.md` §2, §5).
    fn handle_os_event(&mut self, ev: OsEvent, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        match ev {
            OsEvent::FrontAppSwitched { name, .. } => {
                let mut env = EnvVars::new();
                if let Some(n) = name {
                    env.set("INFO", n);
                }
                self.trigger_event(EventInfo::new("front_app_switched", Some(env)), effects);
            }
            OsEvent::SpaceChanged => self.handle_space_change(false, effects, res),
            OsEvent::ActiveDisplayChanged => self.handle_display_change(effects, res),
            OsEvent::MenuBarHiddenChanged => {
                self.model.bar_needs_resize = true;
                self.model.bar_needs_update = true;
            }
            OsEvent::SystemWillSleep => self.system_will_sleep(effects),
            OsEvent::SystemWoke => self.system_woke(effects, res),
            OsEvent::VolumeChanged(v) => {
                self.trigger_info("volume_change", event::level_info(v), effects)
            }
            OsEvent::BrightnessChanged(v) => {
                self.trigger_info("brightness_change", event::level_info(v), effects)
            }
            OsEvent::PowerSourceChanged(p) => {
                self.trigger_info("power_source_change", p.as_str().to_string(), effects)
            }
            OsEvent::WifiChanged(s) => self.trigger_info("wifi_change", s, effects),
            OsEvent::MediaChanged(info) => self.trigger_info("media_change", info, effects),
            OsEvent::MediaArtwork(img) => {
                self.model.current_artwork = img;
                for it in &mut self.model.items {
                    if it.background.image.link
                        || it.icon.background.image.link
                        || it.label.background.image.link
                    {
                        it.needs_update = true;
                    }
                }
            }
            OsEvent::SpaceWindowsChanged(info) => {
                self.trigger_info("space_windows_change", info, effects)
            }
            OsEvent::DistributedNotification { name, info } => {
                if let Some(ev) = self.model.events.name_for_notification(&name) {
                    let ev = ev.to_string();
                    let mut env = EnvVars::new();
                    if let Some(i) = info {
                        env.set("INFO", i);
                    }
                    self.trigger_event(EventInfo::new(ev, Some(env)), effects);
                }
            }
            OsEvent::ConfigChanged => {
                if self.hotload {
                    let path = None;
                    self.reload(path, effects, res);
                }
            }
            OsEvent::CaptureDisabled(d) => self.capture_disabled = d,
        }
    }

    /// `(dsid, sid, fullscreen)` of the current space of display `display`.
    fn space_state(res: &dyn Resources, display: u32) -> (u64, u32, bool) {
        let dsid = res
            .displays()
            .iter()
            .find(|d| d.id == display)
            .map(|d| d.current_space)
            .unwrap_or(0);
        if dsid == 0 {
            return (0, 0, false);
        }
        match res.spaces().iter().position(|s| s.id == dsid) {
            Some(p) => (dsid, p as u32 + 1, res.spaces()[p].fullscreen),
            None => (dsid, 0, false),
        }
    }

    /// `bar_manager_handle_space_change(forced)` (`events.md` §5.2, `bar.md` §6.4–6.5):
    /// sids, `shown`, `bar_change_space` of non-sticky bars, space-item selection,
    /// `space_change` with INFO, then `unfreeze(); bar_manager_refresh(force_refresh)`.
    fn handle_space_change(
        &mut self,
        forced: bool,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let show_fs = self.model.bar.show_in_fullscreen;
        let sticky = self.model.bar.sticky;
        let mut force = false;
        let mut infos = Vec::with_capacity(self.model.bars.len());
        for i in 0..self.model.bars.len() {
            let (dsid, sid, fullscreen) = Self::space_state(res, self.model.bars[i].display);
            let bar = &mut self.model.bars[i];
            bar.sid = sid;
            let was_shown = bar.shown;
            bar.shown = !fullscreen || show_fs;
            if !was_shown && bar.shown {
                self.model.needs_ordering = true;
            }
            force |= was_shown != bar.shown;
            if bar.dsid != dsid {
                bar.dsid = dsid;
                if !sticky && bar.shown && bar.adid >= 1 {
                    let adid = bar.adid;
                    self.space_moves.retain(|(a, _)| *a != adid);
                    self.space_moves.push((adid, dsid));
                    self.needs_render = true;
                }
            }
            infos.push(BarSpace {
                adid: bar.adid,
                sid: bar.sid,
            });
        }
        let info = event::space_change_info(&infos);
        self.update_space_components(forced, res);
        self.trigger_info("space_change", info, effects);
        // `unfreeze(); bar_manager_refresh(force_refresh)`. Like SketchyBar this also ends
        // the freeze of a message batch (`--update`, `--trigger space_change`; Q7, D14), so
        // items added earlier in the batch are associated with their bars before the
        // forced events that follow are dispatched (`updates=when_shown`).
        self.frozen = false;
        self.refresh(force, res);
    }

    /// `bar_manager_update_space_components(forced)` (`events.md` §5.2.1).
    fn update_space_components(&mut self, forced: bool, res: &dyn Resources) {
        for idx in 0..self.model.items.len() {
            if self.model.items[idx].item_type != ItemType::Space {
                continue;
            }
            if !self.model.items[idx].overrides_association {
                let sp = self.model.items[idx].associated_space;
                let space = if sp == 0 {
                    u32::MAX
                } else {
                    sp.trailing_zeros()
                };
                let adid = if space == u32::MAX || space == 0 {
                    None
                } else {
                    res.spaces().get(space as usize - 1).map(|s| s.display)
                };
                self.model.items[idx].associated_display = match adid {
                    Some(a) => bit32(a),
                    None => 1 << 30,
                };
            }
            for b in 0..self.model.bars.len() {
                let (adid, sid) = (self.model.bars[b].adid, self.model.bars[b].sid);
                let item = &mut self.model.items[idx];
                if bit32(adid) & item.associated_display == 0 || sid == 0 {
                    continue;
                }
                let in_space = item.associated_space & bit32(sid) != 0;
                if (!item.selected || forced) && in_space {
                    item.selected = true;
                    item.updates = true;
                    item.env.set("SELECTED", "true");
                } else if (item.selected || forced) && !in_space {
                    item.selected = false;
                    item.updates = true;
                    item.env.set("SELECTED", "false");
                } else {
                    item.updates = false;
                }
            }
        }
    }

    /// `bar_manager_handle_display_change` (`events.md` §5.3).
    fn handle_display_change(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let adid = res.active_display();
        self.model.active_adid = adid;
        self.trigger_info("display_change", event::display_change_info(adid), effects);
    }

    /// `bar_manager_poll_active_display` before every input.
    fn poll_active_display(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        if res.active_display() != self.model.active_adid {
            self.handle_display_change(effects, res);
        }
    }

    /// `bar_manager_display_changed` (`events.md` §9.3): full reset of bars, forced refresh,
    /// `display_change`, forced `space_change`. The forced refresh runs before the events
    /// so the `associated_bar` bits cleared by the reset are set again (`is_shown` for
    /// `updates=when_shown` and marquee starts).
    fn displays_changed(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        self.model.active_adid = res.active_display();
        self.reset_bars(res);
        self.frozen = false;
        self.refresh(true, res);
        self.handle_display_change(effects, res);
        self.handle_space_change(true, effects, res);
    }

    /// Sleep / wake (`events.md` §9.1–9.2) incl. the +500 ms `system_woke` re-post.
    fn system_will_sleep(&mut self, effects: &mut Vec<Effect>) {
        self.trigger_event(EventInfo::new("system_will_sleep", None), effects);
        self.sleeps = true;
    }
    fn system_woke(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        if self.sleeps {
            self.sleeps = false;
            self.wake_repost = Some(res.now() + WAKE_REPOST_DELAY);
        }
        self.displays_changed(effects, res);
        self.trigger_event(EventInfo::new("system_woke", None), effects);
    }

    // ------------------------------------------------------------------------------
    // Mouse (events.md §6, item.md §9). Calls WP-A: layout::window_at, item_at_point,
    // bar_at_point, popup_at_point, item_local_point, slider_track_contains,
    // app_menu_title_at. Calls WP-D: event::click_env/scroll_env/scroll_global_env,
    // ScrollThrottle::feed.
    // ------------------------------------------------------------------------------

    fn handle_mouse(&mut self, m: MouseInput, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        match m.kind {
            MouseKind::Up { .. } => self.on_click(&m, effects),
            MouseKind::Dragged => self.on_drag(&m),
            MouseKind::Moved => self.sync_hover(m.point, effects),
            MouseKind::Entered | MouseKind::Exited => self.on_enter_exit(&m, effects),
            MouseKind::Scrolled { delta } => self.on_scroll(&m, delta, effects, res),
        }
    }

    /// The emulated window under `p`.
    fn hit(&self, p: Point) -> WindowHit {
        match &self.layout {
            Some(l) => layout::window_at(&self.model, l, p),
            None => WindowHit::None,
        }
    }

    /// `get_item_by_wid` with the point fallback for brackets / no item window.
    fn click_target(&self, p: Point) -> (WindowHit, Option<ItemId>) {
        let hit = self.hit(p);
        let mut item = match hit {
            WindowHit::Item(id) => Some(id),
            _ => None,
        };
        if item.map_or(true, |id| {
            self.model.item(id).map_or(true, |i| i.is_bracket())
        }) {
            item = layout::item_at_point(&self.model, p);
        }
        (hit, item)
    }

    /// First display (adid) on which the item's virtual window contains `p`.
    fn adid_of(item: &BarItem, p: Point) -> Option<u32> {
        item.frames
            .iter()
            .position(|f| f.is_some_and(|r| r.contains(p)))
            .map(|i| i as u32 + 1)
    }

    /// `event_mouse_up` → `bar_item_on_click` (§6.2) incl. slider finalisation (D2) and
    /// app_menu `PlatformRequest::OpenMenu`.
    fn on_click(&mut self, m: &MouseInput, effects: &mut Vec<Effect>) {
        let MouseKind::Up {
            button,
            button_code,
        } = m.kind
        else {
            return;
        };
        let (_, item) = self.click_target(m.point);
        let Some(id) = item else { return };
        let env = event::click_env(button, button_code, m.modifiers);
        let active = self.model.active_adid;
        let Some(it) = self.model.item_mut(id) else {
            return;
        };
        let adid = Self::adid_of(it, m.point).unwrap_or(active);
        let local = layout::item_local_point(it, adid, m.point);
        // Hit tests against the bounds the item has on the clicked bar.
        let geometry = self.layout.as_ref().and_then(|l| l.geometry_of(adid, id));
        let (proceed, menu) = layout::with_geometry(it, geometry, |it| {
            if it.has_slider() {
                let inside = local.is_some_and(|l| layout::slider_track_contains(it, l));
                if !(it.slider.is_dragged || inside) {
                    return (false, None);
                }
                if let Some(l) = local {
                    if it.slider.handle_drag(l) {
                        it.needs_update = true;
                    }
                }
                it.slider.is_dragged = false;
                let pct = it.slider.percentage.to_string();
                it.env.set("PERCENTAGE", pct);
            }
            let menu = if it.item_type == ItemType::AppMenu {
                local.and_then(|l| layout::app_menu_title_at(it, l))
            } else {
                None
            };
            (true, menu)
        });
        if !proceed {
            return;
        }
        if let Some(index) = menu {
            effects.push(Effect::Platform(PlatformRequest::OpenMenu { index }));
        }
        let subscribed = it.update_mask.has(EventKind::MouseClicked);
        let persistent = it.env.clone();
        let click_env = script::build_click_script_env(&env, &persistent);
        self.run_click_script(id, click_env, effects);
        if subscribed {
            self.deliver(id, "mouse.clicked", true, Some(&env), effects);
        }
    }

    /// `event_mouse_scrolled` (§6.3).
    fn on_scroll(
        &mut self,
        m: &MouseInput,
        delta: i32,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let Some(total) = self.scroll.feed(delta, res.now()) else {
            return;
        };
        let (hit, item) = self.click_target(m.point);
        let Some(id) = item else {
            match hit {
                WindowHit::Bar(adid) => {
                    let over = self.model.bar(adid).is_some_and(|b| b.mouse_over);
                    if over && !self.any_popup_mouse_over() {
                        let env = event::scroll_global_env(total, adid, m.modifiers);
                        self.trigger_event(
                            EventInfo::new("mouse.scrolled.global", Some(env)),
                            effects,
                        );
                    }
                }
                WindowHit::Popup(host) => {
                    let (over, adid) = self
                        .model
                        .item(host)
                        .map(|h| (h.popup.mouse_over, h.popup.adid))
                        .unwrap_or((false, 0));
                    if over && !self.any_bar_mouse_over() {
                        let env = event::scroll_global_env(total, adid, m.modifiers);
                        self.trigger_event(
                            EventInfo::new("mouse.scrolled.global", Some(env)),
                            effects,
                        );
                    }
                }
                _ => {}
            }
            self.scroll.reset();
            return;
        };
        if self
            .model
            .item(id)
            .is_some_and(|i| i.update_mask.has(EventKind::MouseScrolled))
        {
            let env = event::scroll_env(total, m.modifiers);
            self.deliver(id, "mouse.scrolled", true, Some(&env), effects);
        }
        self.scroll.reset();
    }

    fn any_popup_mouse_over(&self) -> bool {
        self.model
            .items
            .iter()
            .any(|i| i.popup.drawing && i.popup.mouse_over)
    }

    fn any_bar_mouse_over(&self) -> bool {
        self.model.bars.iter().any(|b| b.mouse_over)
    }

    /// Frame of bar window `adid` (layout result, else the stored bar frame).
    fn bar_window_frame(&self, adid: u32) -> Option<Rect> {
        self.layout
            .as_ref()
            .and_then(|l| l.bars.iter().find(|b| b.adid == adid).map(|b| b.frame))
            .or_else(|| self.model.bar(adid).map(|b| b.frame))
    }

    /// `bar_item_mouse_entered`.
    fn mouse_entered(&mut self, id: ItemId, effects: &mut Vec<Effect>) {
        let Some(it) = self.model.item(id) else {
            return;
        };
        if it.update_mask.has(EventKind::MouseEntered) && !it.mouse_over {
            self.deliver(id, "mouse.entered", true, None, effects);
        }
        if let Some(it) = self.model.item_mut(id) {
            it.mouse_over = true;
        }
    }

    /// `bar_item_mouse_exited` (no `mouse_over` precondition).
    fn mouse_exited(&mut self, id: ItemId, effects: &mut Vec<Effect>) {
        let Some(it) = self.model.item(id) else {
            return;
        };
        if it.update_mask.has(EventKind::MouseExited) {
            self.deliver(id, "mouse.exited", true, None, effects);
        }
        if let Some(it) = self.model.item_mut(id) {
            it.mouse_over = false;
        }
    }

    /// Synthesized item enter/exit: items with a tracking area (subscribed to
    /// `mouse.entered` or `mouse.exited`) get entered when the pointer moves into one of
    /// their virtual windows and exited when it leaves (`events.md` §6.4/§6.5).
    fn sync_hover(&mut self, p: Point, effects: &mut Vec<Effect>) {
        let track = EventKind::MouseEntered.bit() | EventKind::MouseExited.bit();
        let mut idx = 0;
        while idx < self.model.items.len() {
            let it = &self.model.items[idx];
            idx += 1;
            if it.update_mask.0 & track == 0 {
                continue;
            }
            let id = it.id;
            let inside = it.drawing && it.contains_point(p);
            if inside && !it.mouse_over {
                self.mouse_entered(id, effects);
            } else if !inside && it.mouse_over {
                // Moving from an item into its own popup keeps the hover.
                let into_popup = it.update_mask.has(EventKind::MouseExitedGlobal)
                    && layout::popup_at_point(&self.model, p) == Some(id);
                if !into_popup {
                    self.mouse_exited(id, effects);
                }
            }
        }
    }

    /// `event_mouse_entered` / `event_mouse_exited` (§6.4/§6.5) for bar/popup windows, and
    /// synthesized item enter/exit from pointer motion (`MouseKind::Moved`).
    fn on_enter_exit(&mut self, m: &MouseInput, effects: &mut Vec<Effect>) {
        let p = m.point;
        match (m.kind, m.window) {
            (MouseKind::Entered, Some(WindowKey::Bar(adid))) => {
                let over_popup = self.any_popup_mouse_over();
                if let Some(bar) = self.model.bars.iter_mut().find(|b| b.adid == adid) {
                    if !bar.mouse_over && !over_popup {
                        bar.mouse_over = true;
                        self.trigger_event(EventInfo::new("mouse.entered.global", None), effects);
                    }
                }
            }
            (MouseKind::Entered, Some(WindowKey::Popup(host))) => {
                let over_bar = self.any_bar_mouse_over();
                if let Some(h) = self.model.item_mut(host) {
                    if !h.popup.mouse_over && !over_bar {
                        h.popup.mouse_over = true;
                        self.trigger_event(EventInfo::new("mouse.entered.global", None), effects);
                    }
                }
            }
            (MouseKind::Exited, Some(WindowKey::Bar(adid))) => {
                let origin = self.bar_window_frame(adid).unwrap_or(Rect::ZERO);
                let target = layout::popup_at_point(&self.model, p);
                let over_origin = contains_half_open(&origin.inset(1.0, 1.0), p);
                if !over_origin && target.is_none() {
                    if let Some(b) = self.model.bars.iter_mut().find(|b| b.adid == adid) {
                        b.mouse_over = false;
                    }
                    self.exit_global(effects);
                } else if !over_origin {
                    if let Some(b) = self.model.bars.iter_mut().find(|b| b.adid == adid) {
                        b.mouse_over = false;
                    }
                    if let Some(h) = target.and_then(|t| self.model.item_mut(t)) {
                        h.popup.mouse_over = true;
                    }
                }
            }
            (MouseKind::Exited, Some(WindowKey::Popup(host))) => {
                let origin = self
                    .model
                    .item(host)
                    .and_then(|h| h.popup.frame)
                    .unwrap_or(Rect::ZERO);
                let target = layout::bar_at_point(&self.model, p);
                let over_origin = contains_half_open(&origin.inset(1.0, 1.0), p);
                if !over_origin && target.is_none() {
                    if let Some(h) = self.model.item_mut(host) {
                        h.popup.mouse_over = false;
                    }
                    self.exit_global(effects);
                } else if !over_origin {
                    if let Some(b) =
                        target.and_then(|a| self.model.bars.iter_mut().find(|b| b.adid == a))
                    {
                        b.mouse_over = true;
                    }
                    let mask = match self.model.item_mut(host) {
                        Some(h) => {
                            h.popup.mouse_over = false;
                            h.update_mask
                        }
                        None => Default::default(),
                    };
                    if (mask.has(EventKind::MouseExited) || mask.has(EventKind::MouseExitedGlobal))
                        && layout::item_at_point(&self.model, p) != Some(host)
                    {
                        self.mouse_exited(host, effects);
                    }
                }
            }
            _ => {}
        }
        self.sync_hover(p, effects);
    }

    /// Leaving the union of bars and popups: `mouse.exited.global`, then `mouse.exited` to
    /// every subscribed item (Quirk Q6).
    fn exit_global(&mut self, effects: &mut Vec<Effect>) {
        self.trigger_event(EventInfo::new("mouse.exited.global", None), effects);
        let mut idx = 0;
        while idx < self.model.items.len() {
            let id = self.model.items[idx].id;
            idx += 1;
            self.mouse_exited(id, effects);
        }
    }

    /// `event_mouse_dragged` (§6.6).
    fn on_drag(&mut self, m: &MouseInput) {
        let Some(id) = self.item_under(m.point) else {
            return;
        };
        let active = self.model.active_adid;
        let Some(it) = self.model.item_mut(id) else {
            return;
        };
        if !it.has_slider() {
            return;
        }
        let adid = Self::adid_of(it, m.point).unwrap_or(active);
        if let Some(local) = layout::item_local_point(it, adid, m.point) {
            let geometry = self.layout.as_ref().and_then(|l| l.geometry_of(adid, id));
            if layout::with_geometry(it, geometry, |it| it.slider.handle_drag(local)) {
                it.needs_update = true;
            }
        }
    }

    // ------------------------------------------------------------------------------
    // Bars, displays, redraw (bar.md §2.3–2.5, §5, §6). Calls WP-A: layout::bar_frame,
    // layout::layout, layout::bar_scene, layout::popup_scene.
    // ------------------------------------------------------------------------------

    /// `bar_manager_begin` / `bar_manager_reset` (`bar.md` §6.6): one `BarState` per selected
    /// display (`displays` pattern, main mode), `any_bar_hidden` applied in pattern mode.
    fn begin_bars(&mut self, res: &mut dyn Resources) {
        self.model.bars.clear();
        let pattern = self.model.bar.displays;
        let hidden = self.model.bar.any_bar_hidden;
        let mut bars = Vec::new();
        if pattern == DISPLAY_MAIN {
            if let Some(d) = res.displays().first() {
                bars.push(BarState::new(d.id, d.adid));
            }
        } else {
            for d in res.displays() {
                if d.adid >= 1 && pattern & bit32(d.adid - 1) != 0 {
                    let mut b = BarState::new(d.id, d.adid);
                    b.hidden = hidden;
                    bars.push(b);
                }
            }
        }
        for b in &mut bars {
            let (dsid, sid, fullscreen) = Self::space_state(res, b.display);
            b.dsid = dsid;
            b.sid = sid;
            // `bar_create` ignores `show_in_fullscreen` (quirk).
            b.shown = !fullscreen;
        }
        self.model.bars = bars;
        self.model.active_adid = res.active_display();
        self.model.needs_ordering = true;
        self.model.bar_needs_update = true;
        self.model.bar_needs_resize = true;
    }

    /// `bar_manager_reset`: drop every bar association and recreate the bars.
    fn reset_bars(&mut self, res: &mut dyn Resources) {
        for it in &mut self.model.items {
            it.associated_bar = 0;
        }
        self.begin_bars(res);
    }

    /// `bar_manager_resize`: recompute every bar window frame.
    fn resize_bars(&mut self, res: &mut dyn Resources) {
        let visible = res.menu_bar_visible();
        for i in 0..self.model.bars.len() {
            let did = self.model.bars[i].display;
            if let Some(d) = res.displays().iter().find(|d| d.id == did) {
                let f = layout::bar_frame(&self.model.bar, d, visible);
                self.model.bars[i].frame = f;
            }
        }
        self.model.bar_needs_resize = false;
        self.needs_layout = true;
        self.needs_render = true;
    }

    /// `bar_manager_set_hidden` (`bar.md` §2.3) incl. closing all popups when hiding.
    /// `adid = Some(n)` addresses `bars[n-1]` (quirk); `None` = all bars.
    fn set_hidden(&mut self, adid: Option<u32>, hidden: bool) {
        match adid {
            Some(a) => {
                if let Some(b) = self.model.bars.get_mut(a as usize - 1) {
                    b.hidden = hidden;
                }
            }
            None => {
                for b in &mut self.model.bars {
                    b.hidden = hidden;
                }
                self.model.bar.any_bar_hidden = hidden;
            }
        }
        if hidden {
            for it in &mut self.model.items {
                it.popup.set_drawing(false);
            }
        }
        self.model.bar_needs_update = true;
    }

    /// `bar_manager_bar_needs_redraw` (`bar.md` §5.2) for one bar. `Redraw::PopupMembers`
    /// when only popup members triggered it: SketchyBar still runs `bar_draw` (association
    /// bits), but the bar window shows nothing of them except clip holes (PERF-4).
    fn bar_needs_redraw(&self, adid: u32) -> Redraw {
        let m = &self.model;
        if m.bar_needs_update {
            return Redraw::Bar;
        }
        let Some(bar) = m.bar(adid) else {
            return Redraw::No;
        };
        let mask = bit32(adid) as u64;
        let sid_bit = bit32(bar.sid);
        let mut popup_only = false;
        for item in &m.items {
            if Self::item_needs_redraw(m, bar, item, adid, mask, sid_bit) {
                if item.position != Position::Popup {
                    return Redraw::Bar;
                }
                popup_only = true;
            }
        }
        if popup_only {
            Redraw::PopupMembers
        } else {
            Redraw::No
        }
    }

    /// The per-item conditions of `bar_manager_bar_needs_redraw`.
    fn item_needs_redraw(
        m: &Model,
        bar: &BarState,
        item: &BarItem,
        adid: u32,
        mask: u64,
        sid_bit: u32,
    ) -> bool {
        let draws = m.draws_item(bar, item);
        if item.needs_update && draws {
            return true;
        }
        if !item.drawing && item.associated_bar != 0 {
            return true;
        }
        if item.ignore_association {
            return false;
        }
        let drawn_here = ((item.associated_bar as u64) << 1) & mask != 0;
        let in_display = (item.associated_display as u64) & mask != 0;
        if draws && in_display && !drawn_here {
            return true;
        }
        if draws && item.associated_to_active_display && m.active_adid == adid && !drawn_here {
            return true;
        }
        if !item.associated_to_active_display
            && item.associated_display > 0
            && !in_display
            && drawn_here
        {
            return true;
        }
        if item.drawing && item.associated_to_active_display && drawn_here && adid != m.active_adid
        {
            return true;
        }
        if item.item_type == ItemType::Space {
            return false;
        }
        if item.associated_space > 0 && item.associated_space & sid_bit == 0 && drawn_here {
            return true;
        }
        draws && item.associated_space > 0 && item.associated_space & sid_bit != 0 && !drawn_here
    }

    /// Window properties of bar windows.
    fn bar_props(&self) -> WinProps {
        let b = &self.model.bar;
        WinProps {
            level: b.window_level,
            blur: b.blur_radius,
            shadow: b.shadow,
            sticky: b.sticky,
            font_smoothing: b.font_smoothing,
        }
    }

    /// `bar_manager_refresh(forced)` (`bar.md` §5.2): the redraw decision. Marks the windows
    /// to re-render, updates `associated_bar` bits of redrawn bars and clears the dirty
    /// flags. No-op while frozen.
    fn refresh(&mut self, forced: bool, res: &mut dyn Resources) {
        if self.frozen {
            return;
        }
        let forced = forced | std::mem::take(&mut self.force_refresh);
        if forced {
            for it in &mut self.model.items {
                it.associated_bar = 0;
                it.needs_update = true;
            }
        }
        if forced || self.model.bar_needs_resize {
            self.resize_bars(res);
        }
        let props = self.bar_props();
        let props_changed = self.emitted_bars.values().any(|e| e.props != props);
        for i in 0..self.model.bars.len() {
            let (adid, sid) = (self.model.bars[i].adid, self.model.bars[i].sid);
            let redraw = if forced || props_changed {
                Redraw::Bar
            } else {
                self.bar_needs_redraw(adid)
            };
            if redraw == Redraw::No || sid < 1 || adid < 1 {
                continue;
            }
            if redraw == Redraw::Bar {
                self.dirty.insert(WindowKey::Bar(adid));
            } else {
                self.popup_member_bars.insert(adid);
            }
            let bit = bit32(adid - 1);
            for idx in 0..self.model.items.len() {
                let draws = self
                    .model
                    .draws_item(&self.model.bars[i], &self.model.items[idx]);
                let it = &mut self.model.items[idx];
                if draws {
                    it.associated_bar |= bit;
                } else {
                    it.associated_bar &= !bit;
                }
            }
        }
        // Popups: re-render when the host or a member changed, or the blur changed.
        for host in &self.model.items {
            if !host.popup.drawing || host.popup.items.is_empty() {
                continue;
            }
            let blur_changed = self
                .emitted_popups
                .get(&host.id)
                .is_some_and(|e| e.blur != host.popup.blur_radius);
            let dirty = forced
                || blur_changed
                || host.needs_update
                || host
                    .popup
                    .items
                    .iter()
                    .any(|m| self.model.item(*m).is_some_and(|i| i.needs_update));
            if dirty {
                self.dirty.insert(WindowKey::Popup(host.id));
            }
        }
        if !self.dirty.is_empty() || !self.popup_member_bars.is_empty() {
            self.needs_layout = true;
            self.needs_render = true;
        }
        for it in &mut self.model.items {
            it.needs_update = false;
        }
        self.model.needs_ordering = false;
        self.model.bar_needs_update = false;
    }

    /// One layout pass over everything (`layout::layout`), timed for `--query stats`.
    fn run_layout(&mut self, res: &mut dyn Resources) {
        let t = Instant::now();
        let l = layout::layout(&mut self.model, res);
        self.layout_times.push(t.elapsed().as_micros() as u64);
        self.layout = Some(l);
        self.needs_layout = false;
        self.needs_render = true;
    }

    /// Applies animator steps: resolves `AnimTarget` (Bar → `BarProps::anim_set`, Default →
    /// default item, Item → `BarItem::anim_set`), marks owners dirty (`events.md` §10.7),
    /// merges `PropEffects`.
    fn apply_anim_steps(&mut self, steps: Vec<AnimStep>) {
        for s in steps {
            let mut fx = PropEffects::default();
            let changed = match s.target {
                AnimTarget::Bar => self.model.bar.anim_set(&s.path, s.value, &mut fx),
                AnimTarget::Default => self.model.default_item.anim_set(&s.path, s.value, &mut fx),
                AnimTarget::Item(id) => match self.model.item_mut(id) {
                    Some(it) => it.anim_set(&s.path, s.value, &mut fx),
                    None => false,
                },
            };
            self.model.bar_needs_update |= fx.bar_needs_update;
            self.model.bar_needs_resize |= fx.bar_needs_resize;
            self.model.might_need_clipping |= fx.might_need_clipping;
            if changed {
                self.mark_dirty(s.target);
            }
        }
    }

    /// Marks the windows showing `id` dirty (or everything for `AnimTarget::Bar`/`Default`).
    fn mark_dirty(&mut self, target: AnimTarget) {
        match target {
            AnimTarget::Bar | AnimTarget::Default => self.model.bar_needs_update = true,
            AnimTarget::Item(id) => {
                if let Some(it) = self.model.item_mut(id) {
                    it.needs_update = true;
                    let parent = it.parent.filter(|_| it.position == Position::Popup);
                    if let Some(p) = parent {
                        self.dirty.insert(WindowKey::Popup(p));
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------------------
    // Providers, aliases, app menus, Lua (extensions). Calls WP-D: provider::apply_sample,
    // provider::sample_info_json.
    // ------------------------------------------------------------------------------

    /// (Re)configures the platform provider of an item after `PropRequest::ProviderChanged`.
    fn configure_provider(&mut self, id: ItemId, effects: &mut Vec<Effect>) {
        let Some(item) = self.model.item(id) else {
            return;
        };
        let cfg = &item.provider;
        let req = match cfg.kind {
            Some(k) if k.is_core() => {
                // Core provider: stop a platform provider the item may have had, start the
                // connection and apply the stored state at the end of the input.
                effects.push(Effect::Platform(PlatformRequest::StopProvider { item: id }));
                self.queue_aerospace_provider(id, true);
                self.start_aerospace(effects);
                return;
            }
            None => PlatformRequest::StopProvider { item: id },
            Some(k) => PlatformRequest::StartProvider {
                item: id,
                provider: k.name().to_string(),
                freq: cfg.freq.or(Some(k.default_freq())),
                args: cfg
                    .args
                    .clone()
                    .or_else(|| k.default_args().map(str::to_string)),
            },
        };
        effects.push(Effect::Platform(req));
    }

    /// `Input::ProviderSample`: label/icon update (as `--set`) + script run with
    /// `SENDER=provider`, `INFO=<json>`. Samples for items without a provider or with a core
    /// provider (a late sample of a replaced platform provider) are dropped.
    fn provider_sample(
        &mut self,
        id: ItemId,
        values: Vec<(String, String)>,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let Some(item) = self.model.item(id) else {
            return;
        };
        match item.provider.kind {
            Some(k) if !k.is_core() => {}
            _ => return,
        }
        self.apply_provider_sample(id, values, effects, res);
    }

    /// Applies one sample to an item with a provider: label/icon from the templates (as
    /// `--set`, no animation), then the item's script with `SENDER=provider`.
    fn apply_provider_sample(
        &mut self,
        id: ItemId,
        values: Vec<(String, String)>,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let Some(item) = self.model.item(id) else {
            return;
        };
        let out = provider::apply_sample(&item.provider, &values);
        self.anim = None;
        for (key, v) in [("label", out.label), ("icon", out.icon)] {
            let Some(v) = v else { continue };
            let (r, _, fx, reqs) = self.set_prop_on(AnimTarget::Item(id), key, &v, res);
            self.merge_fx(fx, effects);
            let _ = reqs;
            if matches!(r, Ok(true)) {
                if let Some(it) = self.model.item_mut(id) {
                    it.needs_update = true;
                }
            }
        }
        self.count_event("provider");
        let mut env = EnvVars::new();
        env.set("INFO", out.info);
        self.update_item(id, Some(Sender::Provider), false, Some(&env), effects);
    }

    /// `Input::AliasImage` (`components.md` §9.6): `Alias::apply_capture`, redraw on change.
    fn alias_image(
        &mut self,
        id: ItemId,
        image: Option<ImageInfo>,
        window_id: u32,
        frame: Rect,
        disabled: bool,
    ) {
        let Some(it) = self.model.item_mut(id) else {
            return;
        };
        if !it.has_alias() {
            return;
        }
        if it
            .alias
            .apply_capture(image, window_id, frame, disabled, false)
        {
            it.needs_update = true;
        }
    }

    /// `Input::MenuTitles`: update every `app_menu` item, fire `menus_change` (INFO = JSON
    /// array of titles).
    fn menu_titles(&mut self, app: String, titles: Vec<String>, effects: &mut Vec<Effect>) {
        let changed = self.menu_app != app || self.menu_titles != titles;
        if !changed {
            return;
        }
        for it in &mut self.model.items {
            if it.item_type == ItemType::AppMenu {
                it.app_menu.app_name = app.clone();
                it.app_menu.titles = titles.clone();
                it.app_menu.hovered = None;
                it.needs_update = true;
            }
        }
        let info = serde_json::to_string(&titles).unwrap_or_else(|_| "[]".to_string());
        self.menu_app = app;
        self.menu_titles = titles;
        self.trigger_info("menus_change", info, effects);
    }

    /// `Input::Lua` (commands with callbacks, in-process subscriptions).
    fn handle_lua(&mut self, req: LuaRequest, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        match req {
            LuaRequest::Command { args, callback } => {
                let rsp = self.run_message(&args, effects, res).unwrap_or_default();
                // In-process callers cannot stream.
                self.monitor_req = None;
                if let Some(handler) = callback {
                    effects.push(Effect::LuaCallback {
                        handler,
                        env: vec![("RESPONSE".to_string(), rsp)],
                    });
                }
            }
            LuaRequest::Subscribe {
                item,
                events,
                handler,
            } => {
                let mut rsp = String::new();
                match self
                    .model
                    .find(&item)
                    .and_then(|id| self.model.item_mut(id))
                {
                    Some(it) => {
                        it.lua_handler = Some(handler);
                        let evs: Vec<String> = events
                            .into_iter()
                            .filter(|e| !matches!(e.as_str(), "routine" | "forced" | "*"))
                            .collect();
                        self.exec_subscribe(&item, &evs, &mut rsp, effects);
                    }
                    None => {
                        let _ = write!(rsp, "[!] Subscribe: Item not found '{item}'\n");
                    }
                }
                if !rsp.is_empty() {
                    effects.push(Effect::Log(rsp));
                }
            }
            LuaRequest::On { events, handler } => {
                self.register_global_handler(events, handler, effects)
            }
        }
    }

    // ------------------------------------------------------------------------------
    // AeroSpace (extension, `docs/superpowers/specs/2026-10-09-aerospace-design.md` §Core).
    // ------------------------------------------------------------------------------

    /// Emits `PlatformRequest::StartAerospace` the first time AeroSpace is used.
    fn start_aerospace(&mut self, effects: &mut Vec<Effect>) {
        if !self.model.aerospace.active {
            self.model.aerospace.active = true;
            effects.push(Effect::Platform(PlatformRequest::StartAerospace));
        }
    }

    /// The script variables of an AeroSpace event: `INFO` + [`AerospaceEvent::env`].
    fn aerospace_env(ev: &AerospaceEvent) -> EnvVars {
        let mut env = EnvVars::new();
        env.set("INFO", ev.info_json());
        for (k, v) in ev.env() {
            env.set(k, v);
        }
        env
    }

    /// `Input::Aerospace`: updates `model.aerospace`, triggers the event for its
    /// subscribers (`INFO` + the event's variables, like every other event) and queues the
    /// `provider=aerospace` items when the state changed.
    fn aerospace_event(&mut self, ev: AerospaceEvent, effects: &mut Vec<Effect>) {
        let state = self.model.aerospace.apply(&ev);
        let env = Self::aerospace_env(&ev);
        self.trigger_event(EventInfo::new(ev.event_name(), Some(env)), effects);
        if state {
            self.queue_all_aerospace_providers();
        }
    }

    /// A manual `--trigger aerospace_workspace_change FOCUSED_WORKSPACE=…` (old AeroSpace
    /// without `subscribe`, driven by `exec-on-workspace-change`): updates the stored
    /// workspace while there is no connection, so `provider=aerospace` and
    /// `--query aerospace` keep working in that setup.
    fn manual_aerospace_trigger(&mut self, env: &EnvVars) {
        let var = |k: &str, alias: &str| env.get(k).or_else(|| env.get(alias)).map(str::to_string);
        let focused = var("FOCUSED_WORKSPACE", aerospace::ALIAS_FOCUSED_WORKSPACE);
        let prev = var("PREV_WORKSPACE", aerospace::ALIAS_PREV_WORKSPACE);
        if self
            .model
            .aerospace
            .apply_manual_trigger(focused.as_deref(), prev.as_deref())
        {
            self.queue_all_aerospace_providers();
        }
    }

    /// Queues `id` for [`Runtime::flush_aerospace_providers`]; `force` applies the sample
    /// even when it did not change since the last one applied to the item.
    fn queue_aerospace_provider(&mut self, id: ItemId, force: bool) {
        match self.aerospace_pending.iter_mut().find(|(p, _)| *p == id) {
            Some((_, f)) => *f |= force,
            None => self.aerospace_pending.push((id, force)),
        }
    }

    /// Queues every `provider=aerospace` item (the state changed).
    fn queue_all_aerospace_providers(&mut self) {
        let ids: Vec<ItemId> = self
            .model
            .items
            .iter()
            .filter(|it| it.provider.kind == Some(ProviderKind::Aerospace))
            .map(|it| it.id)
            .collect();
        for id in ids {
            self.queue_aerospace_provider(id, false);
        }
    }

    /// Applies the AeroSpace state to the queued `provider=aerospace` items (a provider
    /// sample: label/icon templates + script with `SENDER=provider`). Nothing is applied
    /// before the first state-carrying event arrived, and an item whose sample equals the
    /// last one applied to it is skipped unless its provider was (re)configured.
    fn flush_aerospace_providers(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        if self.aerospace_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.aerospace_pending);
        if !self.model.aerospace.known {
            return;
        }
        // Provider labels never animate; keep the message's `--animate` for later commands.
        let anim = self.anim.take();
        for (id, force) in pending {
            let Some(item) = self.model.item(id) else {
                self.aerospace_applied.remove(&id);
                continue;
            };
            if item.provider.kind != Some(ProviderKind::Aerospace) {
                self.aerospace_applied.remove(&id);
                continue;
            }
            let values =
                provider::aerospace_sample(&self.model.aerospace, item.provider.args.as_deref());
            if !force && self.aerospace_applied.get(&id) == Some(&values) {
                continue;
            }
            self.aerospace_applied.insert(id, values.clone());
            self.apply_provider_sample(id, values, effects, res);
        }
        self.anim = anim;
    }

    /// The env of the synthetic event `name` for a late subscriber, if its state is known.
    fn initial_env(&self, name: &str) -> Option<EnvVars> {
        if name == privacy::EVENT_NAME {
            return self.model.privacy.known.then(|| self.privacy_env());
        }
        self.model
            .aerospace
            .synthetic_event(name)
            .map(|ev| Self::aerospace_env(&ev))
    }

    /// Queues the synthetic event `name` for `who` (a late subscriber), if the state for it
    /// is known (delivered by [`Runtime::flush_initial`]).
    fn queue_initial(&mut self, who: Listener, name: &'static str) {
        if self.initial_env(name).is_some() && !self.initial_events.contains(&(who, name)) {
            self.initial_events.push((who, name));
        }
    }

    /// Delivers the queued synthetic AeroSpace events, each to its subscriber only (item
    /// gating as for a real event). Returns whether anything was queued.
    fn flush_initial(&mut self, effects: &mut Vec<Effect>) -> bool {
        if self.initial_events.is_empty() {
            return false;
        }
        for (who, name) in std::mem::take(&mut self.initial_events) {
            let Some(env) = self.initial_env(name) else {
                continue;
            };
            match who {
                Listener::Item(id) => {
                    let subscribed = self
                        .model
                        .events
                        .flag(name)
                        .zip(self.model.item(id))
                        .is_some_and(|(flag, it)| it.update_mask.contains(flag));
                    if subscribed {
                        self.update_item(
                            id,
                            Some(Sender::Event(name.to_string())),
                            false,
                            Some(&env),
                            effects,
                        );
                    }
                }
                Listener::Global(handler) => {
                    let registered = self
                        .global_handlers
                        .iter()
                        .any(|(e, h)| e == name && *h == handler);
                    if registered {
                        effects.push(Effect::LuaCallback {
                            handler,
                            env: Self::global_env(name, Some(&env)),
                        });
                    }
                }
            }
        }
        true
    }

    /// The env of an item-less handler: the event's variables plus `SENDER` (no `NAME`).
    fn global_env(name: &str, env: Option<&EnvVars>) -> Vec<(String, String)> {
        let mut env = env.cloned().unwrap_or_default();
        env.set("SENDER", name);
        env.into_vec()
    }

    /// Calls every item-less handler of `name` (`LuaRequest::On`), in registration order.
    fn run_global_handlers(
        &mut self,
        name: &str,
        env: Option<&EnvVars>,
        effects: &mut Vec<Effect>,
    ) {
        if !self.global_handlers.iter().any(|(e, _)| e == name) {
            return;
        }
        let env = Self::global_env(name, env);
        for (_, handler) in self.global_handlers.iter().filter(|(e, _)| e == name) {
            effects.push(Effect::LuaCallback {
                handler: *handler,
                env: env.clone(),
            });
        }
    }

    /// `LuaRequest::On`: registers an item-less handler for `events`.
    fn register_global_handler(
        &mut self,
        events: Vec<String>,
        handler: u64,
        effects: &mut Vec<Effect>,
    ) {
        for ev in events {
            if self
                .global_handlers
                .iter()
                .any(|(e, h)| *e == ev && *h == handler)
            {
                continue;
            }
            let aerospace_event = aerospace::EVENT_NAMES.iter().find(|n| **n == ev.as_str());
            self.global_handlers.push((ev, handler));
            if let Some(name) = aerospace_event {
                self.start_aerospace(effects);
                self.queue_initial(Listener::Global(handler), name);
            }
        }
    }

    // ------------------------------------------------------------------------------
    // Privacy indicator (extension, `docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`).
    // ------------------------------------------------------------------------------

    /// Emits `PlatformRequest::StartPrivacyIndicator` the first time it is needed.
    fn start_privacy(&mut self, effects: &mut Vec<Effect>) {
        if !self.model.privacy.active {
            self.model.privacy.active = true;
            effects.push(Effect::Platform(PlatformRequest::StartPrivacyIndicator));
        }
    }

    /// `INFO` + the event's variables.
    fn privacy_env(&self) -> EnvVars {
        let mut env = EnvVars::new();
        env.set("INFO", self.model.privacy.info_json());
        for (k, v) in self.model.privacy.env() {
            env.set(k, v);
        }
        env
    }

    /// `Input::PrivacyIndicator`: stores the sample; on a change fires
    /// `privacy_indicator_change` and, when the dot moved and the inset is on, lays the
    /// bars out again.
    fn privacy_sample(&mut self, s: PrivacySample, effects: &mut Vec<Effect>) {
        let change = self.model.privacy.apply(s);
        if change.geometry && self.model.bar.privacy_indicator_inset {
            self.model.bar_needs_update = true;
        }
        if change.any {
            let env = self.privacy_env();
            self.trigger_event(EventInfo::new(privacy::EVENT_NAME, Some(env)), effects);
        }
    }

    /// Hover tracking helper: the item whose (emulated) window is topmost under `p`.
    fn item_under(&self, p: Point) -> Option<ItemId> {
        match self.hit(p) {
            WindowHit::Item(id) => Some(id),
            _ => None,
        }
    }

    /// Fills the derived fields of `stats` (`--query stats`, `--monitor stats`).
    fn fill_stats(&mut self) {
        if let (Some(s), Some(n)) = (self.started, self.last_now) {
            self.stats.uptime_s = n.saturating_duration_since(s).as_secs_f64();
        }
        self.stats.windows = (self.emitted_bars.len() + self.emitted_popups.len()) as u32;
        self.stats.frame_time_us = self.frame_times.summary();
        self.stats.layout_time_us = self.layout_times.summary();
        self.stats.redraws_by_window = self
            .redraws
            .iter()
            .map(|(k, v)| {
                let name = match k {
                    WindowKey::Bar(a) => format!("bar:{a}"),
                    WindowKey::Popup(h) => {
                        let adid = self.model.item(*h).map(|i| i.popup.adid).unwrap_or(0);
                        format!(
                            "popup:{}:{adid}",
                            name_or_null(self.model.name_of(*h).as_deref())
                        )
                    }
                };
                (name, *v)
            })
            .collect();
        self.stats.scripts_running = self
            .stats
            .scripts_spawned
            .saturating_sub(self.scripts_finished)
            .min(u32::MAX as u64) as u32;
        self.stats.script_avg_ms = if self.script_finished_timed > 0 {
            self.script_total_ms / self.script_finished_timed as f64
        } else {
            0.0
        };
        self.stats.script_max_ms = self.scripts.values().map(|s| s.max_ms).fold(0.0, f64::max);
        self.stats.lua_avg_us = self
            .lua_total_us
            .checked_div(self.stats.lua_callbacks)
            .unwrap_or(0);
        self.stats.scripts_by_item = self
            .scripts
            .iter()
            .map(|(k, s)| {
                let avg = if s.finished > 0 {
                    s.total_ms / s.finished as f64
                } else {
                    0.0
                };
                (k.clone(), (s.runs, avg, s.max_ms))
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::HeadlessResources;

    #[test]
    fn regex_selectors_are_compiled_once() {
        let mut res = HeadlessResources::default();
        let mut rt = Runtime::new(RuntimeConfig {
            bar_name: "mbar".into(),
            home: "/home/u".into(),
            config_path: None,
        });
        rt.begin(&mut res);
        let mut fx = Vec::new();
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        rt.run_message(&args(&["--add", "item", "a1", "left"]), &mut fx, &mut res);
        for _ in 0..3 {
            let rsp = rt.run_message(&args(&["--set", "/a.*/", "label=x"]), &mut fx, &mut res);
            assert_eq!(rsp.as_deref(), Some(""));
        }
        assert_eq!(rt.regex_cache.len(), 1);
        assert!(rt.regex_cache.contains_key("a.*"));
        // Failed compiles are not cached.
        let rsp = rt.run_message(&args(&["--set", "/a\\(/", "label=x"]), &mut fx, &mut res);
        assert_eq!(
            rsp.as_deref(),
            Some("[!] Regex: Could not compile regex '/a\\(/'\n")
        );
        assert_eq!(rt.regex_cache.len(), 1);
        // The cache is bounded.
        for i in 0..(REGEX_CACHE_SIZE + 5) {
            let sel = format!("/a{i}/");
            rt.run_message(&args(&["--set", &sel, "label=x"]), &mut fx, &mut res);
        }
        assert!(rt.regex_cache.len() <= REGEX_CACHE_SIZE);
    }

    fn runtime() -> (Runtime, HeadlessResources) {
        let mut res = HeadlessResources::default();
        let mut rt = Runtime::new(RuntimeConfig {
            bar_name: "mbar".into(),
            home: "/home/u".into(),
            config_path: None,
        });
        rt.begin(&mut res);
        (rt, res)
    }

    fn msg(rt: &mut Runtime, res: &mut HeadlessResources, a: &[&str]) -> Vec<Effect> {
        let args = a.iter().map(|s| s.to_string()).collect();
        rt.handle(
            Input::Message {
                args,
                reply: ReplyToken(1),
            },
            res,
        )
    }

    fn text_keys(rt: &Runtime) -> std::collections::HashSet<TextKey> {
        let mut keys = std::collections::HashSet::new();
        rt.for_each_text_key(&mut |k| {
            keys.insert(k);
        });
        keys
    }

    /// Every drawable text line is reported as live (macOS `TextCache` prunes by this),
    /// however long ago it was measured; replaced lines are not.
    #[test]
    fn for_each_text_key_reports_every_drawable_line() {
        let (mut rt, mut res) = runtime();
        msg(
            &mut rt,
            &mut res,
            &[
                "--add",
                "item",
                "a",
                "left",
                "--set",
                "a",
                "icon=I",
                "label=old",
            ],
        );
        msg(&mut rt, &mut res, &["--add", "item", "p", "popup.a"]);
        msg(&mut rt, &mut res, &["--set", "p", "label=in popup"]);
        msg(&mut rt, &mut res, &["--add", "slider", "s", "right", "100"]);
        msg(&mut rt, &mut res, &["--set", "s", "slider.knob=K"]);
        msg(&mut rt, &mut res, &["--add", "app_menu", "m", "left"]);
        rt.handle(
            Input::MenuTitles {
                app: "Finder".into(),
                titles: vec!["Apple".into(), "File".into(), "Edit".into()],
            },
            &mut res,
        );
        rt.frame(res.now, &mut res);
        let old = rt
            .model
            .item(rt.model.find("a").unwrap())
            .unwrap()
            .label
            .line;
        msg(&mut rt, &mut res, &["--set", "a", "label=new"]);
        // Many unrelated updates later the popup label is still live.
        for i in 0..50 {
            msg(&mut rt, &mut res, &["--set", "s", &format!("icon={i}")]);
            rt.frame(res.now, &mut res);
        }

        let keys = text_keys(&rt);
        let line = |name: &str, f: fn(&BarItem) -> Option<TextKey>| {
            f(rt.model.item(rt.model.find(name).unwrap()).unwrap()).unwrap()
        };
        assert!(keys.contains(&line("a", |i| i.icon.line)));
        assert!(keys.contains(&line("a", |i| i.label.line)));
        assert!(keys.contains(&line("p", |i| i.label.line)));
        assert!(keys.contains(&line("s", |i| i.slider.knob.line)));
        assert!(keys.contains(&line("s", |i| i.icon.line)));
        let font = rt
            .model
            .item(rt.model.find("p").unwrap())
            .unwrap()
            .label
            .font
            .clone();
        let popup_key = res.text_metrics(&font, "in popup").key;
        assert!(keys.contains(&popup_key));
        let menu = rt.model.item(rt.model.find("m").unwrap()).unwrap();
        assert!(!menu.app_menu.measured.cells.is_empty());
        for cell in &menu.app_menu.measured.cells {
            assert!(keys.contains(&cell.key), "app_menu title not live");
        }
        // The replaced label is no longer referenced.
        assert_ne!(old, Some(line("a", |i| i.label.line)));
        assert!(!keys.contains(&old.unwrap()));
    }

    /// `Runtime::exit` (signal shutdown) behaves like `--exit`, once.
    #[test]
    fn exit_outside_a_message_is_like_exit_command() {
        let (mut rt, mut res) = runtime();
        msg(&mut rt, &mut res, &["--add", "item", "a", "left"]);
        msg(
            &mut rt,
            &mut res,
            &["--set", "a", "mach_helper=dev.test.helper"],
        );
        let fx = rt.exit();
        assert!(fx.contains(&Effect::Platform(PlatformRequest::MachSend {
            service: "dev.test.helper".into(),
            payload: MACH_HELPER_DESTROY.to_vec(),
        })));
        assert_eq!(fx.last(), Some(&Effect::Exit));
        assert!(rt.exit().is_empty(), "second exit emits nothing");
        let fx = msg(&mut rt, &mut res, &["--set", "a", "label=late"]);
        assert!(fx.iter().all(|e| !matches!(e, Effect::Exit)));
        let label = &rt
            .model
            .item(rt.model.find("a").unwrap())
            .unwrap()
            .label
            .string;
        assert_eq!(label, "", "messages after exit are ignored");
    }

    fn remove_alias_requests(fx: &[Effect]) -> Vec<ItemId> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Platform(PlatformRequest::RemoveAlias { item }) => Some(*item),
                _ => None,
            })
            .collect()
    }

    /// Removing an alias item (or resetting the model) stops its captures on the
    /// platform; other items emit nothing.
    #[test]
    fn removing_an_alias_emits_remove_alias() {
        let (mut rt, mut res) = runtime();
        msg(
            &mut rt,
            &mut res,
            &["--add", "alias", "Control Center,Clock", "right"],
        );
        msg(
            &mut rt,
            &mut res,
            &["--add", "alias", "Control Center,WiFi", "right"],
        );
        msg(&mut rt, &mut res, &["--add", "item", "plain", "left"]);
        let clock = rt.model.find("Control Center,Clock").unwrap();
        let wifi = rt.model.find("Control Center,WiFi").unwrap();

        let fx = msg(&mut rt, &mut res, &["--remove", "plain"]);
        assert!(remove_alias_requests(&fx).is_empty());

        let fx = msg(&mut rt, &mut res, &["--remove", "Control Center,Clock"]);
        assert_eq!(remove_alias_requests(&fx), vec![clock]);

        let fx = msg(&mut rt, &mut res, &["--reload"]);
        assert_eq!(remove_alias_requests(&fx), vec![wifi]);
    }
}
