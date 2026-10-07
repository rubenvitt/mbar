//! `--query` output — **WP-B** (`docs/spec/cli.md` §9, `docs/spec/bar.md` §9,
//! `docs/spec/events.md` §3.4, `docs/spec/components.md` §9.4, `docs/EXTENSIONS.md`).
//!
//! Item, bar, popup and component fragments are implemented on the data model
//! (`BarItem::to_json`, `BarProps::to_json`, …). This module dispatches the query targets
//! and serializes the registries that are not part of the model. Byte-exact rules: tabs,
//! `0x%x`, `%f`, `(null)`, strings JSON-escaped (D13), every output ends with `\n`.

use crate::command::QueryTarget;
use crate::event::CustomEvents;
use crate::model::Model;
use crate::platform::{DisplayInfo, MenuExtra};
use std::collections::BTreeMap;

/// Runtime statistics for `--query stats` / `--monitor stats` (filled by WP-C).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub uptime_s: f64,
    pub windows: u32,
    pub frames: u64,
    /// avg / p95 / max in µs.
    pub frame_time_us: (u64, u64, u64),
    pub layout_time_us: (u64, u64, u64),
    pub redraws_by_window: BTreeMap<String, u64>,
    pub scripts_spawned: u64,
    pub scripts_running: u32,
    pub script_avg_ms: f64,
    pub script_max_ms: f64,
    /// Per item: (runs, avg ms, max ms).
    pub scripts_by_item: BTreeMap<String, (u64, f64, f64)>,
    pub lua_callbacks: u64,
    pub lua_avg_us: u64,
    pub lua_max_us: u64,
    /// Per event name (incl. `routine`).
    pub events: BTreeMap<String, u64>,
    pub ipc_messages: u64,
}

/// Everything a query may read.
pub struct QueryCx<'a> {
    pub model: &'a Model,
    pub displays: &'a [DisplayInfo],
    /// `None` = no Screen Recording permission.
    pub menu_extras: Option<&'a [MenuExtra]>,
    pub stats: &'a Stats,
    /// Front app menu titles (`--query menus`).
    pub menus: &'a [String],
}

/// Executes one `--query`. Returns the full response text (JSON or the exact error
/// message): `item <name>` → `[!] Query: Item '<name>' not found\n`; fallback name →
/// `[!] Query: Invalid query, or item '<name>' not found \n`; `defaults` serializes the
/// default item (`"name": "defaults"` or `(null)` after reset).
pub fn query(target: &QueryTarget, cx: &QueryCx) -> String {
    let _ = (target, cx);
    todo!("WP-B: cli.md §9")
}

/// `--query <item>` JSON (`BarItem::to_json` with names resolved through the model).
pub fn item_json(model: &Model, item: &crate::item::BarItem) -> String {
    item.to_json(&|id| model.name_of(id))
}

/// `custom_events_serialize` (`events.md` §3.4).
pub fn events_json(events: &CustomEvents) -> String {
    let _ = events;
    todo!("WP-B: events.md §3.4")
}

/// `display_serialize` (`bar.md` §9.2): no spaces after colons, `%.4f` frames, last object
/// closes with `\t}\n`, empty list → empty output.
pub fn displays_json(displays: &[DisplayInfo]) -> String {
    let _ = displays;
    todo!("WP-B: bar.md §9.2")
}

/// `print_all_menu_items` (`components.md` §9.4): permission error text when `None`;
/// entries `\t"<owner>,<name>(<k>)"` joined by `", \n"`; empty list → no output.
pub fn default_menu_items(extras: Option<&[MenuExtra]>) -> String {
    let _ = extras;
    todo!("WP-B: components.md §9.4")
}

/// `--query stats` (extension, `docs/EXTENSIONS.md`).
pub fn stats_json(stats: &Stats, items: usize) -> String {
    let _ = (stats, items);
    todo!("WP-B: EXTENSIONS.md")
}

/// `--query menus` (extension): JSON array of the front application's top-level titles.
pub fn menus_json(titles: &[String]) -> String {
    let _ = titles;
    todo!("WP-B: EXTENSIONS.md")
}
