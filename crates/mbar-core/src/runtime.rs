//! The state machine — **WP-C** (`docs/DESIGN-CORE.md` "Runtime").
//!
//! `Runtime` owns all state, consumes [`Input`]s and emits [`Effect`]s; `frame` produces the
//! windows to redraw. Single-threaded; `handle` never draws, it marks dirty state.
//!
//! Message handling follows `cli.md` §2.5: reset `--animate`, freeze, run every command in
//! order (appending responses), resize/`bar_needs_update` if a `--bar`/`--update`/`--remove`
//! asked for it, `animator.lock_all()`, unfreeze, refresh, reply.
//!
//! Helper methods are grouped by spec area below; each group lists the functions it calls
//! from other work packages (see `docs/IMPLEMENTATION-PLAN.md` "Cross-package contracts").

use crate::animation::{AnimStep, Animator};
use crate::command::{AddCommand, Command, QueryTarget, Selector, SetToken};
use crate::event::{EventInfo, ScrollThrottle};
use crate::geometry::Point;
use crate::item::{ItemId, ItemType};
use crate::layout::Layout;
use crate::model::Model;
use crate::platform::{
    Effect, FrameOutput, Input, LuaRequest, MouseInput, OsEvent, ReplyToken, Resources, WindowKey,
};
use crate::props::{AnimSpec, AnimTarget, PropEffects, PropRequest};
use crate::query::Stats;
use crate::script::{EnvVars, Sender};
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

/// Static configuration of a daemon instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// `BAR_NAME` (basename of argv[0]; `sketchybar` maps to `mbar`).
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
    /// Item the pointer is over (synthesized `mouse.entered`/`mouse.exited`).
    hovered: Option<ItemId>,
    /// Alias capture suspended by WindowServer notifications.
    capture_disabled: bool,
    listeners: Listeners,
    /// Last layout (hit testing, scenes).
    layout: Option<Layout>,
    /// Windows that must be re-rendered by the next `frame`.
    dirty: BTreeSet<WindowKey>,
    /// Windows currently open on the platform.
    open_windows: BTreeSet<WindowKey>,
    /// Effects produced outside `handle` (e.g. by animation frames) to return next time.
    pending: Vec<Effect>,
    stats: Stats,
    started: Option<Instant>,
    /// `--monitor` subscribers exist (extension).
    monitoring: bool,
    /// Front app menus (app_menu extension).
    menu_app: String,
    menu_titles: Vec<String>,
    /// In-process Lua handlers per item.
    lua_handlers: HashMap<ItemId, u64>,
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
            hovered: None,
            capture_disabled: false,
            listeners: Listeners::default(),
            layout: None,
            dirty: BTreeSet::new(),
            open_windows: BTreeSet::new(),
            pending: Vec::new(),
            stats: Stats::default(),
            started: None,
            monitoring: false,
            menu_app: String::new(),
            menu_titles: Vec::new(),
            lua_handlers: HashMap::new(),
        }
    }

    /// `bar_manager_begin` at startup: create bars for the selected displays, poll the
    /// active display and schedule the first routine tick (now + 1 s).
    pub fn begin(&mut self, res: &mut dyn Resources) -> Vec<Effect> {
        let _ = res;
        todo!("WP-C: bar.md §6.6 bar_manager_begin, §8.1")
    }

    /// Processes one input. Before every input the active display is polled
    /// (`bar_manager_poll_active_display`, `events.md` §1.1).
    pub fn handle(&mut self, input: Input, res: &mut dyn Resources) -> Vec<Effect> {
        let _ = (input, res);
        todo!("WP-C")
    }

    /// Steps animations (`Animator::step` + [`Runtime::apply_anim_steps`]), lays out
    /// (`layout::layout`), updates `associated_bar` bits, and returns scenes for dirty
    /// windows only (`bar_manager_refresh` / `bar_draw` redraw decision, `bar.md` §5).
    pub fn frame(&mut self, now: Instant, res: &mut dyn Resources) -> FrameOutput {
        let _ = (now, res);
        todo!("WP-C: bar.md §5")
    }

    /// Earliest of: routine tick, wake re-post, provider/alias schedules, animation frame.
    pub fn next_deadline(&self) -> Option<Instant> {
        todo!("WP-C")
    }

    /// True if `frame` should run now (dirty windows or running animations).
    pub fn needs_frame(&self) -> bool {
        !self.dirty.is_empty() || self.animator.needs_frame() || self.model.bar_needs_update
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
        let _ = (args, reply, res);
        todo!("WP-C: cli.md §2.5")
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
        let _ = (cmd, rsp, effects, res);
        todo!("WP-C: cli.md §3.4")
    }

    /// Resolves a selector (`cli.md` §3.5): exact name or BRE over all names in global order;
    /// appends `[!] Set: Item not found '<name>'\n` / regex messages as appropriate (the
    /// caller decides the error prefix for `--remove`).
    fn select(&self, sel: &Selector, rsp: &mut String) -> Vec<ItemId> {
        let _ = (sel, rsp);
        todo!("WP-C: cli.md §3.5")
    }

    /// `--set`: token-major / item-minor application with `PropCx` (target
    /// `AnimTarget::Item`), malformed-token message naming the first item, `needs_update` on
    /// change, then `apply_requests`.
    fn exec_set(
        &mut self,
        ids: &[ItemId],
        tokens: &[SetToken],
        rsp: &mut String,
        res: &mut dyn Resources,
    ) {
        let _ = (ids, tokens, rsp, res);
        todo!("WP-C: cli.md §6.2")
    }

    /// `--default` (target `AnimTarget::Default`; malformed pair ends the domain).
    fn exec_default(
        &mut self,
        pairs: &[(String, String)],
        malformed: Option<&str>,
        rsp: &mut String,
        res: &mut dyn Resources,
    ) {
        let _ = (pairs, malformed, rsp, res);
        todo!("WP-C: cli.md §6.3")
    }

    /// `--bar` (target `AnimTarget::Bar`); returns the refresh flag.
    fn exec_bar(
        &mut self,
        pairs: &[(String, String)],
        malformed: Option<&str>,
        rsp: &mut String,
        res: &mut dyn Resources,
    ) -> bool {
        let _ = (pairs, malformed, rsp, res);
        todo!("WP-C: bar.md §2")
    }

    /// Executes `PropRequest`s pushed by a setter on item `id` (or the bar).
    fn apply_requests(
        &mut self,
        id: Option<ItemId>,
        reqs: Vec<PropRequest>,
        fx: PropEffects,
        rsp: &mut String,
        res: &mut dyn Resources,
    ) -> bool {
        let _ = (id, reqs, fx, rsp, res);
        todo!("WP-C: item.md §2.3, bar.md §2.3–2.5")
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
        let _ = (add, rsp, effects, res);
        todo!("WP-C: cli.md §6.1")
    }

    /// `--clone` (`item.md` §10.1; D6/D7; popup members not attached).
    fn exec_clone(
        &mut self,
        name: &str,
        parent: &str,
        placement: Option<crate::command::Placement>,
        rsp: &mut String,
        res: &mut dyn Resources,
    ) {
        let _ = (name, parent, placement, rsp, res);
        todo!("WP-C: item.md §10.1")
    }

    /// `bar_manager_remove_item` (`item.md` §10.5): popup lists, brackets, popup children,
    /// D18 `animator.cancel_target`, provider stop.
    fn remove_item(&mut self, id: ItemId, effects: &mut Vec<Effect>) {
        let _ = (id, effects);
        todo!("WP-C: item.md §10.5")
    }

    /// `--move` (D8: self-move is a no-op), `--reorder` (D8: duplicates → first wins),
    /// `--rename`, `--push` (D5).
    fn exec_move(&mut self, item: &str, before: bool, reference: &str, rsp: &mut String) {
        let _ = (item, before, reference, rsp);
        todo!("WP-C: item.md §10.4")
    }
    fn exec_reorder(&mut self, names: &[String], rsp: &mut String) {
        let _ = (names, rsp);
        todo!("WP-C: item.md §10.3")
    }
    fn exec_rename(&mut self, old: &str, new: &str, rsp: &mut String) {
        let _ = (old, new, rsp);
        todo!("WP-C: item.md §10.2")
    }
    fn exec_push(&mut self, item: &str, values: &[f32], rsp: &mut String) {
        let _ = (item, values, rsp);
        todo!("WP-C: item.md §10.6")
    }

    /// `popup_add_item` / `popup_remove_item` (`item.md` §6.1).
    fn popup_add_item(&mut self, host: ItemId, item: ItemId) {
        let _ = (host, item);
        todo!("WP-C: item.md §6.1")
    }
    fn popup_remove_item(&mut self, host: ItemId, item: ItemId) {
        let _ = (host, item);
        todo!("WP-C: item.md §6.1")
    }

    /// `--query` (calls `query::query` with a `QueryCx`).
    fn exec_query(&mut self, target: &QueryTarget, rsp: &mut String, res: &mut dyn Resources) {
        let _ = (target, rsp, res);
        todo!("WP-C → WP-B query::query")
    }

    /// `--reload` / hotload (`cli.md` §11): mach helpers get `"k"`, animations dropped, model
    /// re-initialised (events back to built-ins, listeners kept), bars recreated,
    /// `Effect::RunConfig`.
    fn reload(&mut self, path: Option<String>, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (path, effects, res);
        todo!("WP-C: cli.md §11")
    }

    // ------------------------------------------------------------------------------
    // Events & scripts (events.md §3–§9). Calls WP-D: script::build_update_env,
    // script::build_click_script_env, script::serialize_for_mach, event::* builders,
    // animation::marquee.
    // ------------------------------------------------------------------------------

    /// `bar_item_update(item, sender, forced, env)` (`events.md` §4.2): counter/marquee
    /// handling, gating (`updates`, `update_freq`, `when_shown`), env building (D1), script
    /// spawn, mach helper send, Lua handler call.
    fn update_item(
        &mut self,
        id: ItemId,
        sender: Option<Sender>,
        forced: bool,
        env: Option<&EnvVars>,
        effects: &mut Vec<Effect>,
    ) {
        let _ = (id, sender, forced, env, effects);
        todo!("WP-C: events.md §4.2")
    }

    /// `bar_manager_custom_events_trigger(name, env)`: every subscribed item in global order,
    /// non-forced (D16: fresh env per item).
    fn trigger_event(&mut self, ev: EventInfo, effects: &mut Vec<Effect>) {
        let _ = (ev, effects);
        todo!("WP-C: events.md §4.1")
    }

    /// `--subscribe` (`events.md` §3.3): bits, lazy listeners (`PlatformRequest::Start*`).
    fn exec_subscribe(
        &mut self,
        item: &str,
        events: &[String],
        rsp: &mut String,
        effects: &mut Vec<Effect>,
    ) {
        let _ = (item, events, rsp, effects);
        todo!("WP-C: events.md §3.3")
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
        let _ = (event, args, effects, res);
        todo!("WP-C: events.md §7")
    }

    /// `--update` = `bar_manager_update(forced=true)` (`events.md` §8.2).
    fn exec_update(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (effects, res);
        todo!("WP-C: events.md §8.2")
    }

    /// 1 s routine clock (`bar_manager_update(false)`, `events.md` §8.1): routine updates,
    /// marquee starts, alias recapture requests (`Alias::tick`).
    fn routine_tick(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (effects, res);
        todo!("WP-C: events.md §8.1")
    }

    /// OS notifications (`events.md` §2, §5).
    fn handle_os_event(&mut self, ev: OsEvent, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (ev, effects, res);
        todo!("WP-C: events.md §5")
    }

    /// `bar_manager_handle_space_change(forced)` (`events.md` §5.2, `bar.md` §6.4–6.5):
    /// sids, `shown`, space-item selection, `space_change` with INFO.
    fn handle_space_change(
        &mut self,
        forced: bool,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let _ = (forced, effects, res);
        todo!("WP-C: events.md §5.2")
    }

    /// `bar_manager_handle_display_change` (`events.md` §5.3).
    fn handle_display_change(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (effects, res);
        todo!("WP-C: events.md §5.3")
    }

    /// `bar_manager_poll_active_display` before every input.
    fn poll_active_display(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (effects, res);
        todo!("WP-C: events.md §1.1")
    }

    /// `bar_manager_display_changed` (`events.md` §9.3): full reset of bars, forced refresh,
    /// `display_change`, forced `space_change`.
    fn displays_changed(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (effects, res);
        todo!("WP-C: events.md §9.3")
    }

    /// Sleep / wake (`events.md` §9.1–9.2) incl. the +500 ms `system_woke` re-post.
    fn system_will_sleep(&mut self, effects: &mut Vec<Effect>) {
        let _ = effects;
        todo!("WP-C: events.md §9.1")
    }
    fn system_woke(&mut self, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (effects, res);
        todo!("WP-C: events.md §9.2")
    }

    // ------------------------------------------------------------------------------
    // Mouse (events.md §6, item.md §9). Calls WP-A: layout::window_at, item_at_point,
    // bar_at_point, popup_at_point, item_local_point, slider_track_contains,
    // app_menu_title_at. Calls WP-D: event::click_env/scroll_env/scroll_global_env,
    // ScrollThrottle::feed.
    // ------------------------------------------------------------------------------

    fn handle_mouse(&mut self, m: MouseInput, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (m, effects, res);
        todo!("WP-C: events.md §6")
    }
    /// `event_mouse_up` → `bar_item_on_click` (§6.2) incl. slider finalisation (D2) and
    /// app_menu `PlatformRequest::OpenMenu`.
    fn on_click(&mut self, m: &MouseInput, effects: &mut Vec<Effect>) {
        let _ = (m, effects);
        todo!("WP-C: events.md §6.2")
    }
    /// `event_mouse_scrolled` (§6.3).
    fn on_scroll(
        &mut self,
        m: &MouseInput,
        delta: i32,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let _ = (m, delta, effects, res);
        todo!("WP-C: events.md §6.3")
    }
    /// `event_mouse_entered` / `event_mouse_exited` (§6.4/§6.5) for bar/popup windows, and
    /// synthesized item enter/exit from pointer motion (`MouseKind::Moved`).
    fn on_enter_exit(&mut self, m: &MouseInput, effects: &mut Vec<Effect>) {
        let _ = (m, effects);
        todo!("WP-C: events.md §6.4–6.5")
    }
    /// `event_mouse_dragged` (§6.6).
    fn on_drag(&mut self, m: &MouseInput) {
        let _ = m;
        todo!("WP-C: events.md §6.6")
    }

    // ------------------------------------------------------------------------------
    // Bars, displays, redraw (bar.md §2.3–2.5, §5, §6). Calls WP-A: layout::bar_frame,
    // layout::layout, layout::bar_scene, layout::popup_scene.
    // ------------------------------------------------------------------------------

    /// `bar_manager_begin` / `bar_manager_reset` (`bar.md` §6.6): one `BarState` per selected
    /// display (`displays` pattern, main mode), `any_bar_hidden` applied in pattern mode.
    fn begin_bars(&mut self, res: &mut dyn Resources) {
        let _ = res;
        todo!("WP-C: bar.md §6.6")
    }

    /// `bar_manager_set_hidden` (`bar.md` §2.3) incl. closing all popups when hiding.
    fn set_hidden(&mut self, adid: Option<u32>, hidden: bool) {
        let _ = (adid, hidden);
        todo!("WP-C: bar.md §2.3")
    }

    /// `bar_manager_bar_needs_redraw` (`bar.md` §5.2) for one bar.
    fn bar_needs_redraw(&self, adid: u32) -> bool {
        let _ = adid;
        todo!("WP-C: bar.md §5.2")
    }

    /// Applies animator steps: resolves `AnimTarget` (Bar → `BarProps::anim_set`, Default →
    /// default item, Item → `BarItem::anim_set`), marks owners dirty (`events.md` §10.7),
    /// merges `PropEffects`.
    fn apply_anim_steps(&mut self, steps: Vec<AnimStep>) {
        let _ = steps;
        todo!("WP-C: events.md §10.7")
    }

    /// Marks the windows showing `id` dirty (or everything for `AnimTarget::Bar`/`Default`).
    fn mark_dirty(&mut self, target: AnimTarget) {
        let _ = target;
        todo!("WP-C")
    }

    // ------------------------------------------------------------------------------
    // Providers, aliases, app menus, Lua (extensions). Calls WP-D: provider::apply_sample,
    // provider::sample_info_json.
    // ------------------------------------------------------------------------------

    /// (Re)configures the platform provider of an item after `PropRequest::ProviderChanged`.
    fn configure_provider(&mut self, id: ItemId, effects: &mut Vec<Effect>) {
        let _ = (id, effects);
        todo!("WP-C: EXTENSIONS.md providers")
    }

    /// `Input::ProviderSample`: label/icon update (as `--set`) + script run with
    /// `SENDER=provider`, `INFO=<json>`.
    fn provider_sample(
        &mut self,
        id: ItemId,
        values: Vec<(String, String)>,
        effects: &mut Vec<Effect>,
        res: &mut dyn Resources,
    ) {
        let _ = (id, values, effects, res);
        todo!("WP-C: EXTENSIONS.md providers")
    }

    /// `Input::AliasImage` (`components.md` §9.6): `Alias::apply_capture`, redraw on change.
    fn alias_image(
        &mut self,
        id: ItemId,
        image: Option<crate::platform::ImageInfo>,
        window_id: u32,
        frame: crate::geometry::Rect,
        disabled: bool,
    ) {
        let _ = (id, image, window_id, frame, disabled);
        todo!("WP-C: components.md §9.6")
    }

    /// `Input::MenuTitles`: update every `app_menu` item, fire `menus_change` (INFO = JSON
    /// array of titles).
    fn menu_titles(&mut self, app: String, titles: Vec<String>, effects: &mut Vec<Effect>) {
        let _ = (app, titles, effects);
        todo!("WP-C: EXTENSIONS.md app_menu")
    }

    /// `Input::Lua` (commands with callbacks, in-process subscriptions).
    fn handle_lua(&mut self, req: LuaRequest, effects: &mut Vec<Effect>, res: &mut dyn Resources) {
        let _ = (req, effects, res);
        todo!("WP-C: mbar-lua contract")
    }

    /// Hover tracking helper for synthesized item enter/exit.
    fn item_under(&self, p: Point) -> Option<ItemId> {
        let _ = p;
        todo!("WP-C")
    }

    /// Whether an item type participates in alias recapture.
    fn is_alias(&self, id: ItemId) -> bool {
        self.model
            .item(id)
            .is_some_and(|i| i.item_type == ItemType::Alias)
    }
}
