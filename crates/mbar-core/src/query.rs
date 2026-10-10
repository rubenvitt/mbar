//! `--query` output — **WP-B** (`docs/spec/cli.md` §9, `docs/spec/bar.md` §9,
//! `docs/spec/events.md` §3.4, `docs/spec/components.md` §9.4, `docs/EXTENSIONS.md`;
//! `--query borders`: `docs/superpowers/specs/2026-10-09-borders-design.md` §1;
//! `--query aerospace`: `docs/superpowers/specs/2026-10-09-aerospace-design.md`).
//!
//! Item, bar, popup and component fragments are implemented on the data model
//! (`BarItem::to_json`, `BarProps::to_json`, …). This module dispatches the query targets
//! and serializes the registries that are not part of the model. Byte-exact rules: tabs,
//! `0x%x`, `%f`, `(null)`, strings JSON-escaped (D13), every output ends with `\n`.

use crate::command::QueryTarget;
use crate::event::CustomEvents;
use crate::model::Model;
use crate::platform::{DisplayInfo, MenuExtra};
use crate::value::{json_escape, json_opt};
use std::collections::BTreeMap;
use std::fmt::Write as _;

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
    /// Front app menu titles (`--query menus`); `None` = no Accessibility permission.
    pub menus: Option<&'a [String]>,
}

/// Executes one `--query`. Returns the full response text (JSON or the exact error
/// message): `item <name>` → `[!] Query: Item '<name>' not found\n`; fallback name →
/// `[!] Query: Invalid query, or item '<name>' not found \n`; `defaults` serializes the
/// default item (`"name": "defaults"` or `(null)` after reset).
///
/// The SketchyBar keywords shadow item names (only `--query item <name>` reaches such
/// items). The extension keywords `stats`, `menus`, `borders` and `aerospace` do **not**: an existing
/// item with that name is served as before, so plain SketchyBar configs behave unchanged.
pub fn query(target: &QueryTarget, cx: &QueryCx) -> String {
    let model = cx.model;
    let item_by_name = |name: &str| model.find(name).and_then(|id| model.item(id));
    match target {
        QueryTarget::Bar => model.bar.to_json(&model.item_names()),
        QueryTarget::Defaults => item_json(model, &model.default_item),
        QueryTarget::Events => events_json(&model.events),
        QueryTarget::Displays => displays_json(cx.displays),
        QueryTarget::DefaultMenuItems => default_menu_items(cx.menu_extras),
        QueryTarget::Item(name) => match item_by_name(name) {
            Some(item) => item_json(model, item),
            None => format!("[!] Query: Item '{name}' not found\n"),
        },
        QueryTarget::Name(name) => match item_by_name(name) {
            Some(item) => item_json(model, item),
            None => format!("[!] Query: Invalid query, or item '{name}' not found \n"),
        },
        QueryTarget::Stats => match item_by_name("stats") {
            Some(item) => item_json(model, item),
            None => stats_json(cx.stats, model.items.len()),
        },
        QueryTarget::Menus => match item_by_name("menus") {
            Some(item) => item_json(model, item),
            None => match cx.menus {
                Some(titles) => menus_json(titles),
                None => MENUS_NO_PERMISSION.to_string(),
            },
        },
        QueryTarget::Borders => match item_by_name("borders") {
            Some(item) => item_json(model, item),
            None => model.borders.to_json(),
        },
        QueryTarget::Aerospace => match item_by_name("aerospace") {
            Some(item) => item_json(model, item),
            None => model.aerospace.to_json(),
        },
    }
}

/// `--query <item>` JSON (`BarItem::to_json` with names resolved through the model).
pub fn item_json(model: &Model, item: &crate::item::BarItem) -> String {
    item.to_json(&|id| model.name_of(id))
}

/// `custom_events_serialize` (`events.md` §3.4).
pub fn events_json(events: &CustomEvents) -> String {
    let list = events.events();
    let mut out = String::from("{\n");
    for (i, e) in list.iter().enumerate() {
        let _ = write!(
            out,
            "\t\"{}\": {{\n\t\t\"bit\": {},\n\t\t\"notification\": \"{}\"\n",
            json_escape(&e.name),
            1u64 << i,
            json_opt(e.notification.as_deref()),
        );
        if i + 1 < list.len() {
            out.push_str("\t},\n");
        }
    }
    out.push_str("\t}\n}\n");
    out
}

/// `display_serialize` (`bar.md` §9.2): no spaces after colons, `%.4f` frames, last object
/// closes with `\t}\n`, empty list → empty output. Displays are printed in arrangement
/// order (`adid` 1..N).
pub fn displays_json(displays: &[DisplayInfo]) -> String {
    if displays.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&DisplayInfo> = displays.iter().collect();
    sorted.sort_by_key(|d| d.adid);
    let f4 = |v: f32| format!("{:.4}", v as f64);
    let mut out = String::from("[\n");
    for (i, d) in sorted.iter().enumerate() {
        let uuid = d
            .uuid
            .as_deref()
            .map_or_else(|| "<unknown>".to_string(), json_escape);
        let _ = write!(
            out,
            "\t{{\n\t\t\"arrangement-id\":{},\n\t\t\"DirectDisplayID\":{},\n\t\t\"UUID\":\"{uuid}\",\n",
            d.adid as i32, d.id as i32,
        );
        let _ = write!(
            out,
            "\t\t\"frame\":{{\n\t\t\"x\":{},\n\t\t\"y\":{},\n\t\t\"w\":{},\n\t\t\"h\":{}\n\t\t}}\n",
            f4(d.frame.x),
            f4(d.frame.y),
            f4(d.frame.width),
            f4(d.frame.height),
        );
        let last = i + 1 == sorted.len();
        out.push_str(if last { "\t}\n" } else { "\t},\n" });
    }
    out.push_str("]\n");
    out
}

/// `print_all_menu_items` (`components.md` §9.4): permission error text when `None`;
/// entries `\t"<owner>,<name>(<k>)"` joined by `", \n"`; empty list → no output.
pub fn default_menu_items(extras: Option<&[MenuExtra]>) -> String {
    let Some(extras) = extras else {
        return "[!] Query (default_menu_items): Screen Recording Permissions not given. \
                Restart SketchyBar after granting permissions.\n"
            .to_string();
    };
    if extras.is_empty() {
        return String::new();
    }
    let mut out = String::from("[\n");
    for (i, e) in extras.iter().enumerate() {
        if i > 0 {
            out.push_str(", \n");
        }
        let _ = write!(
            out,
            "\t\"{},{}({})\"",
            json_escape(&e.owner),
            json_escape(&e.name),
            i + 1
        );
    }
    out.push_str("\n]\n");
    out
}

/// A JSON number for an `f64` (non-finite → `0`; integral values keep one decimal).
fn json_f64(v: f64) -> String {
    if !v.is_finite() {
        "0".to_string()
    } else if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

/// Inline JSON object `{ "k": v, … }` (`compact`: `{"k":v,…}`); values are pre-rendered.
fn json_object(pairs: &[(String, String)], compact: bool) -> String {
    if pairs.is_empty() {
        return "{}".to_string();
    }
    let (open, sep, colon, close) = if compact {
        ("{", ",", ":", "}")
    } else {
        ("{ ", ", ", ": ", " }")
    };
    let body: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("\"{}\"{colon}{v}", json_escape(k)))
        .collect();
    format!("{open}{}{close}", body.join(sep))
}

/// The top-level fields of `--query stats` in documentation order.
fn stats_fields(stats: &Stats, items: usize, compact: bool) -> Vec<(String, String)> {
    let obj = |pairs: Vec<(&str, String)>| {
        let pairs: Vec<(String, String)> =
            pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        json_object(&pairs, compact)
    };
    let timing = |t: (u64, u64, u64)| {
        obj(vec![
            ("avg", t.0.to_string()),
            ("p95", t.1.to_string()),
            ("max", t.2.to_string()),
        ])
    };
    let counts = |m: &BTreeMap<String, u64>| {
        let pairs: Vec<(String, String)> =
            m.iter().map(|(k, v)| (k.clone(), v.to_string())).collect();
        json_object(&pairs, compact)
    };
    let by_item: Vec<(String, String)> = stats
        .scripts_by_item
        .iter()
        .map(|(name, &(runs, avg, max))| {
            (
                name.clone(),
                obj(vec![
                    ("runs", runs.to_string()),
                    ("avg_ms", json_f64(avg)),
                    ("max_ms", json_f64(max)),
                ]),
            )
        })
        .collect();
    let scripts = obj(vec![
        ("spawned", stats.scripts_spawned.to_string()),
        ("running", stats.scripts_running.to_string()),
        ("avg_ms", json_f64(stats.script_avg_ms)),
        ("max_ms", json_f64(stats.script_max_ms)),
        ("by_item", json_object(&by_item, compact)),
    ]);
    let lua = obj(vec![
        ("callbacks", stats.lua_callbacks.to_string()),
        ("avg_us", stats.lua_avg_us.to_string()),
        ("max_us", stats.lua_max_us.to_string()),
    ]);
    vec![
        ("uptime_s", json_f64(stats.uptime_s)),
        ("items", items.to_string()),
        ("windows", stats.windows.to_string()),
        ("frames", stats.frames.to_string()),
        ("frame_time_us", timing(stats.frame_time_us)),
        ("layout_time_us", timing(stats.layout_time_us)),
        ("redraws_by_window", counts(&stats.redraws_by_window)),
        ("scripts", scripts),
        ("lua", lua),
        ("events", counts(&stats.events)),
        ("ipc_messages", stats.ipc_messages.to_string()),
        ("version", format!("\"{}\"", env!("CARGO_PKG_VERSION"))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// `--query stats` (extension, `docs/EXTENSIONS.md`): one top-level key per line (TAB
/// indented like the other queries), nested objects inline, maps sorted by key.
pub fn stats_json(stats: &Stats, items: usize) -> String {
    let lines: Vec<String> = stats_fields(stats, items, false)
        .into_iter()
        .map(|(k, v)| format!("\t\"{k}\": {v}"))
        .collect();
    format!("{{\n{}\n}}\n", lines.join(",\n"))
}

/// One `--monitor stats` line: `{"type":"stats",…}` (same fields as [`stats_json`],
/// compact, terminated by `\n`).
pub fn stats_monitor_line(stats: &Stats, items: usize) -> String {
    let body: Vec<String> = stats_fields(stats, items, true)
        .into_iter()
        .map(|(k, v)| format!("\"{k}\":{v}"))
        .collect();
    format!("{{\"type\":\"stats\",{}}}\n", body.join(","))
}

/// Response of `--query menus` without the Accessibility permission (extension).
pub const MENUS_NO_PERMISSION: &str =
    "[!] Query (menus): Accessibility permission not given. Grant it in System Settings > Privacy & Security > Accessibility.\n";

/// `--query menus` (extension): JSON array of the front application's top-level titles,
/// on one line (`["Finder","File"]\n`; trim the newline to reuse it as the
/// `menus_change` INFO).
pub fn menus_json(titles: &[String]) -> String {
    let items: Vec<String> = titles
        .iter()
        .map(|t| format!("\"{}\"", json_escape(t)))
        .collect();
    format!("[{}]\n", items.join(","))
}
