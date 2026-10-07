//! WP-B: `--query` output, byte-exact (`docs/spec/cli.md` §9, `bar.md` §9,
//! `events.md` §3.4, `components.md` §9.4) plus the extension queries.

use mbar_core::command::QueryTarget;
use mbar_core::event::CustomEvents;
use mbar_core::geometry::Rect;
use mbar_core::item::{ItemId, ItemType};
use mbar_core::platform::{DisplayInfo, HeadlessResources, MenuExtra};
use mbar_core::query::{
    default_menu_items, displays_json, events_json, menus_json, query, stats_json,
    stats_monitor_line, QueryCx, Stats,
};
use mbar_core::Model;

fn add_item(model: &mut Model, name: &str, position: &str) -> ItemId {
    let mut res = HeadlessResources::default();
    let id = model.create_item(&mut res);
    let item = model.item_mut(id).unwrap();
    item.set_type(ItemType::Item, "/home/u");
    assert!(item.set_position(position).is_some());
    assert!(item.set_name(name));
    id
}

fn run(model: &Model, target: QueryTarget) -> String {
    run_with(model, target, &[], Some(&[]), &Stats::default(), &[])
}

fn run_with(
    model: &Model,
    target: QueryTarget,
    displays: &[DisplayInfo],
    menu_extras: Option<&[MenuExtra]>,
    stats: &Stats,
    menus: &[String],
) -> String {
    let cx = QueryCx {
        model,
        displays,
        menu_extras,
        stats,
        menus: Some(menus),
    };
    query(&target, &cx)
}

fn assert_json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("invalid JSON ({e}):\n{text}"))
}

// ---------------------------------------------------------------------------
// --query bar
// ---------------------------------------------------------------------------

const BAR_HEAD: &str = "{\n\
\t\"position\": \"top\",\n\
\t\"topmost\": \"off\",\n\
\t\"sticky\": \"on\",\n\
\t\"hidden\": \"off\",\n\
\t\"shadow\": \"off\",\n\
\t\"font_smoothing\": \"off\",\n\
\t\"show_in_fullscreen\": \"off\",\n\
\t\"blur_radius\": 0,\n\
\t\"margin\": 0,\n\
\t\"drawing\": \"off\",\n\
\t\"color\": \"0x44000000\",\n\
\t\"border_color\": \"0xffff0000\",\n\
\t\"border_width\": 0,\n\
\t\"height\": 25,\n\
\t\"corner_radius\": 0,\n\
\t\"padding_left\": 20,\n\
\t\"padding_right\": 20,\n\
\t\"x_offset\": 0,\n\
\t\"y_offset\": 0,\n\
\t\"clip\": 0.000000,\n\
\t\"image\": {\n\
\t\t\"value\": \"(null)\",\n\
\t\t\"drawing\": \"off\",\n\
\t\t\"scale\": 1.000000\n\
\t},\n";

#[test]
fn bar_factory_defaults_without_items() {
    let model = Model::new();
    let expected = format!("{BAR_HEAD}\t\"items\": [\n\n\t]\n}}\n");
    let out = run(&model, QueryTarget::Bar);
    assert_eq!(out, expected);
    assert_json(&out);
}

#[test]
fn bar_lists_items_in_global_order() {
    let mut model = Model::new();
    add_item(&mut model, "b", "right");
    add_item(&mut model, "a", "left");
    let expected = format!("{BAR_HEAD}\t\"items\": [\n\t\t \"b\",\n\t\t \"a\"\n\t]\n}}\n");
    assert_eq!(run(&model, QueryTarget::Bar), expected);
}

#[test]
fn bar_keyword_shadows_item_named_bar() {
    let mut model = Model::new();
    add_item(&mut model, "bar", "left");
    assert!(run(&model, QueryTarget::Bar).starts_with("{\n\t\"position\""));
    assert!(run(&model, QueryTarget::Item("bar".into())).starts_with("{\n\t\"name\": \"bar\""));
}

// ---------------------------------------------------------------------------
// --query <item> / item <name> / defaults
// ---------------------------------------------------------------------------

const BACKGROUND_3: &str = "\t\t\t\"drawing\": \"off\",\n\
\t\t\t\"color\": \"0x0\",\n\
\t\t\t\"border_color\": \"0x0\",\n\
\t\t\t\"border_width\": 0,\n\
\t\t\t\"height\": 0,\n\
\t\t\t\"corner_radius\": 0,\n\
\t\t\t\"padding_left\": 0,\n\
\t\t\t\"padding_right\": 0,\n\
\t\t\t\"x_offset\": 0,\n\
\t\t\t\"y_offset\": 0,\n\
\t\t\t\"clip\": 0.000000,\n\
\t\t\t\"image\": {\n\
\t\t\t\t\"value\": \"(null)\",\n\
\t\t\t\t\"drawing\": \"off\",\n\
\t\t\t\t\"scale\": 1.000000\n\
\t\t\t},\n\
\t\t\t\"shadow\": {\n\
\t\t\t\t\"drawing\": \"off\",\n\
\t\t\t\t\"color\": \"0xff000000\",\n\
\t\t\t\t\"angle\": 30,\n\
\t\t\t\t\"distance\": 5\n\
\t\t\t}\n";

fn text_block() -> String {
    format!(
        "\t\t\"value\": \"\",\n\
\t\t\"drawing\": \"on\",\n\
\t\t\"highlight\": \"off\",\n\
\t\t\"color\": \"0xffffffff\",\n\
\t\t\"highlight_color\": \"0xff000000\",\n\
\t\t\"padding_left\": 0,\n\
\t\t\"padding_right\": 0,\n\
\t\t\"y_offset\": 0,\n\
\t\t\"font\": \"Hack Nerd Font:Bold:14.00\",\n\
\t\t\"width\": 0,\n\
\t\t\"scroll_duration\": 100,\n\
\t\t\"align\": \"left\",\n\
\t\t\"background\": {{\n\
{BACKGROUND_3}\
\t\t}},\n\
\t\t\"shadow\": {{\n\
\t\t\t\"drawing\": \"off\",\n\
\t\t\t\"color\": \"0xff000000\",\n\
\t\t\t\"angle\": 30,\n\
\t\t\t\"distance\": 5\n\
\t\t}}\n"
    )
}

/// `cli.md` §9.2: fresh `--add item foo left` with factory defaults.
fn reference_item(name: &str, rects: &str) -> String {
    let text = text_block();
    format!(
        "{{\n\
\t\"name\": \"{name}\",\n\
\t\"type\": \"item\",\n\
\t\"geometry\": {{\n\
\t\t\"drawing\": \"on\",\n\
\t\t\"position\": \"left\",\n\
\t\t\"associated_space_mask\": 0,\n\
\t\t\"associated_display_mask\": 0,\n\
\t\t\"ignore_association\": \"off\",\n\
\t\t\"y_offset\": 0,\n\
\t\t\"padding_left\": 0,\n\
\t\t\"padding_right\": 0,\n\
\t\t\"scroll_texts\": \"off\",\n\
\t\t\"width\": -1,\n\
\t\t\"background\": {{\n\
{BACKGROUND_3}\
\t\t}}\n\
\t}},\n\
\t\"icon\": {{\n\
{text}\
\t}},\n\
\t\"label\": {{\n\
{text}\
\t}},\n\
\t\"scripting\": {{\n\
\t\t\"script\": \"(null)\",\n\
\t\t\"click_script\": \"(null)\",\n\
\t\t\"update_freq\": 0,\n\
\t\t\"update_mask\": 0,\n\
\t\t\"updates\": \"on\"\n\
\t}},\n\
\t\"bounding_rects\": {{\n\
{rects}\n\
\t}}\n\
}}\n"
    )
}

#[test]
fn item_matches_reference_without_windows() {
    let mut model = Model::new();
    add_item(&mut model, "foo", "left");
    let out = run(&model, QueryTarget::Name("foo".into()));
    assert_eq!(out, reference_item("foo", ""));
    assert_json(&out);
    assert_eq!(run(&model, QueryTarget::Item("foo".into())), out);
}

#[test]
fn item_matches_reference_with_window() {
    let mut model = Model::new();
    let id = add_item(&mut model, "foo", "left");
    model
        .item_mut(id)
        .unwrap()
        .set_frame(1, Rect::new(120.0, 900.0, 2.0, 25.0));
    let rects = "\t\t\"display-1\": {\n\
\t\t\t\"origin\": [ 120.000000, 900.000000 ],\n\
\t\t\t\"size\": [ 2.000000, 25.000000 ]\n\
\t\t}";
    let out = run(&model, QueryTarget::Name("foo".into()));
    assert_eq!(out, reference_item("foo", rects));
    assert_json(&out);
}

#[test]
fn item_name_is_json_escaped() {
    // D13: names with quotes/backslashes still produce valid JSON.
    let mut model = Model::new();
    add_item(&mut model, "a\"b\\c", "left");
    let out = run(&model, QueryTarget::Name("a\"b\\c".into()));
    assert!(out.starts_with("{\n\t\"name\": \"a\\\"b\\\\c\",\n"));
    assert_eq!(assert_json(&out)["name"], "a\"b\\c");
}

#[test]
fn item_not_found_messages() {
    let model = Model::new();
    assert_eq!(
        run(&model, QueryTarget::Item("nope".into())),
        "[!] Query: Item 'nope' not found\n"
    );
    assert_eq!(
        run(&model, QueryTarget::Name("nope".into())),
        "[!] Query: Invalid query, or item 'nope' not found \n"
    );
    assert_eq!(
        run(&model, QueryTarget::Item("".into())),
        "[!] Query: Item '' not found\n"
    );
    assert_eq!(
        run(&model, QueryTarget::Name("".into())),
        "[!] Query: Invalid query, or item '' not found \n"
    );
}

#[test]
fn defaults_query_uses_default_item() {
    let mut model = Model::new();
    let out = run(&model, QueryTarget::Defaults);
    assert_eq!(out, reference_item("defaults", ""));
    assert_json(&out);
    // After `reset=` the default item has no name.
    model.default_item.reset_default();
    let out = run(&model, QueryTarget::Defaults);
    assert!(out.starts_with("{\n\t\"name\": \"(null)\",\n"), "{out}");
}

// ---------------------------------------------------------------------------
// --query events
// ---------------------------------------------------------------------------

const BUILTINS: [&str; 18] = [
    "front_app_switched",
    "space_change",
    "display_change",
    "system_woke",
    "mouse.entered",
    "mouse.exited",
    "mouse.clicked",
    "mouse.scrolled",
    "system_will_sleep",
    "mouse.entered.global",
    "mouse.exited.global",
    "mouse.scrolled.global",
    "volume_change",
    "brightness_change",
    "power_source_change",
    "wifi_change",
    "media_change",
    "space_windows_change",
];

fn event_entry(name: &str, bit: u64, notification: &str) -> String {
    format!("\t\"{name}\": {{\n\t\t\"bit\": {bit},\n\t\t\"notification\": \"{notification}\"\n")
}

#[test]
fn events_builtins_exact() {
    let mut expected = String::from("{\n");
    for (i, name) in BUILTINS.iter().enumerate() {
        expected.push_str(&event_entry(name, 1 << i, "(null)"));
        expected.push_str(if i + 1 < BUILTINS.len() {
            "\t},\n"
        } else {
            "\t}\n}\n"
        });
    }
    let model = Model::new();
    let out = run(&model, QueryTarget::Events);
    assert_eq!(out, expected);
    assert!(out.contains("\t\"space_windows_change\": {\n\t\t\"bit\": 131072,\n"));
    assert_json(&out);
}

#[test]
fn events_custom_appended() {
    let mut events = CustomEvents::new();
    events.append("my_event", Some("com.example.note"));
    events.append("plain", None);
    let out = events_json(&events);
    let tail = format!(
        "\t}},\n{}\t}},\n{}\t}}\n}}\n",
        event_entry("my_event", 262144, "com.example.note"),
        event_entry("plain", 524288, "(null)")
    );
    assert!(out.ends_with(&tail), "{out}");
    let v = assert_json(&out);
    assert_eq!(v["my_event"]["bit"], 262144);
}

// ---------------------------------------------------------------------------
// --query displays
// ---------------------------------------------------------------------------

fn display(id: u32, adid: u32, uuid: Option<&str>, frame: Rect) -> DisplayInfo {
    DisplayInfo {
        id,
        adid,
        uuid: uuid.map(str::to_string),
        frame,
        builtin: false,
        menu_bar_height: 24.0,
        current_space: 0,
    }
}

#[test]
fn displays_exact() {
    let displays = [
        display(
            69734208,
            2,
            None,
            Rect::new(-1920.0, -180.5, 1920.0, 1080.0),
        ),
        display(
            1,
            1,
            Some("37D8832A-2D66-02CA-B9F7-8F30A301B230"),
            Rect::new(0.0, 0.0, 1512.0, 982.0),
        ),
    ];
    let expected = "[\n\
\t{\n\
\t\t\"arrangement-id\":1,\n\
\t\t\"DirectDisplayID\":1,\n\
\t\t\"UUID\":\"37D8832A-2D66-02CA-B9F7-8F30A301B230\",\n\
\t\t\"frame\":{\n\
\t\t\"x\":0.0000,\n\
\t\t\"y\":0.0000,\n\
\t\t\"w\":1512.0000,\n\
\t\t\"h\":982.0000\n\
\t\t}\n\
\t},\n\
\t{\n\
\t\t\"arrangement-id\":2,\n\
\t\t\"DirectDisplayID\":69734208,\n\
\t\t\"UUID\":\"<unknown>\",\n\
\t\t\"frame\":{\n\
\t\t\"x\":-1920.0000,\n\
\t\t\"y\":-180.5000,\n\
\t\t\"w\":1920.0000,\n\
\t\t\"h\":1080.0000\n\
\t\t}\n\
\t}\n\
]\n";
    assert_eq!(displays_json(&displays), expected);
    assert_json(expected);
    let model = Model::new();
    let out = run_with(
        &model,
        QueryTarget::Displays,
        &displays,
        None,
        &Stats::default(),
        &[],
    );
    assert_eq!(out, expected);
}

#[test]
fn displays_did_printed_as_signed() {
    let d = [display(u32::MAX, 1, None, Rect::new(0.0, 0.0, 1.0, 1.0))];
    assert!(displays_json(&d).contains("\"DirectDisplayID\":-1,\n"));
}

#[test]
fn displays_empty_list_prints_nothing() {
    assert_eq!(displays_json(&[]), "");
}

// ---------------------------------------------------------------------------
// --query default_menu_items
// ---------------------------------------------------------------------------

fn extra(owner: &str, name: &str, x: f32) -> MenuExtra {
    MenuExtra {
        owner: owner.into(),
        name: name.into(),
        window_id: 0,
        frame: Rect::new(x, 0.0, 30.0, 24.0),
    }
}

#[test]
fn default_menu_items_exact() {
    let extras = [
        extra("Control Center", "Clock", 1400.0),
        extra("Control Center", "Battery", 1300.0),
        extra("Spotlight", "Item-0", 1200.0),
    ];
    let expected = "[\n\
\t\"Control Center,Clock(1)\", \n\
\t\"Control Center,Battery(2)\", \n\
\t\"Spotlight,Item-0(3)\"\n\
]\n";
    assert_eq!(default_menu_items(Some(&extras)), expected);
    assert_json(expected);
    let model = Model::new();
    assert_eq!(
        run_with(
            &model,
            QueryTarget::DefaultMenuItems,
            &[],
            Some(&extras),
            &Stats::default(),
            &[]
        ),
        expected
    );
}

#[test]
fn default_menu_items_permission_and_empty() {
    assert_eq!(
        default_menu_items(None),
        "[!] Query (default_menu_items): Screen Recording Permissions not given. \
Restart SketchyBar after granting permissions.\n"
    );
    assert_eq!(default_menu_items(Some(&[])), "");
    let model = Model::new();
    assert!(run_with(
        &model,
        QueryTarget::DefaultMenuItems,
        &[],
        None,
        &Stats::default(),
        &[]
    )
    .starts_with("[!] Query (default_menu_items)"));
}

// ---------------------------------------------------------------------------
// Extensions: stats, menus
// ---------------------------------------------------------------------------

fn sample_stats() -> Stats {
    let mut s = Stats {
        uptime_s: 1234.5,
        windows: 3,
        frames: 1200,
        frame_time_us: (310, 640, 2100),
        layout_time_us: (45, 90, 400),
        scripts_spawned: 812,
        scripts_running: 0,
        script_avg_ms: 6.1,
        script_max_ms: 80.0,
        lua_callbacks: 400,
        lua_avg_us: 35,
        lua_max_us: 900,
        ipc_messages: 950,
        ..Stats::default()
    };
    s.redraws_by_window.insert("bar:1".into(), 300);
    s.redraws_by_window.insert("popup:apple:1".into(), 4);
    s.scripts_by_item.insert("clock".into(), (120, 3.2, 9.0));
    s.events.insert("front_app_switched".into(), 31);
    s.events.insert("routine".into(), 600);
    s
}

const STATS_KEYS: [&str; 12] = [
    "uptime_s",
    "items",
    "windows",
    "frames",
    "frame_time_us",
    "layout_time_us",
    "redraws_by_window",
    "scripts",
    "lua",
    "events",
    "ipc_messages",
    "version",
];

#[test]
fn stats_shape() {
    let out = stats_json(&sample_stats(), 42);
    let expected = "{\n\
\t\"uptime_s\": 1234.5,\n\
\t\"items\": 42,\n\
\t\"windows\": 3,\n\
\t\"frames\": 1200,\n\
\t\"frame_time_us\": { \"avg\": 310, \"p95\": 640, \"max\": 2100 },\n\
\t\"layout_time_us\": { \"avg\": 45, \"p95\": 90, \"max\": 400 },\n\
\t\"redraws_by_window\": { \"bar:1\": 300, \"popup:apple:1\": 4 },\n\
\t\"scripts\": { \"spawned\": 812, \"running\": 0, \"avg_ms\": 6.1, \"max_ms\": 80.0, \
\"by_item\": { \"clock\": { \"runs\": 120, \"avg_ms\": 3.2, \"max_ms\": 9.0 } } },\n\
\t\"lua\": { \"callbacks\": 400, \"avg_us\": 35, \"max_us\": 900 },\n\
\t\"events\": { \"front_app_switched\": 31, \"routine\": 600 },\n\
\t\"ipc_messages\": 950,\n"
        .to_string()
        + &format!("\t\"version\": \"{}\"\n}}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(out, expected);
    let v = assert_json(&out);
    let keys: Vec<&str> = STATS_KEYS.to_vec();
    for k in &keys {
        assert!(v.get(*k).is_some(), "missing {k}");
    }
    assert_eq!(v.as_object().unwrap().len(), keys.len());
    assert_eq!(v["scripts"]["by_item"]["clock"]["runs"], 120);
    assert_eq!(v["frame_time_us"]["p95"], 640);
    assert_eq!(v["uptime_s"], 1234.5);
}

#[test]
fn stats_reports_version_last() {
    let out = stats_json(&Stats::default(), 0);
    let last = out
        .trim_end()
        .trim_end_matches('}')
        .trim_end()
        .lines()
        .last()
        .unwrap();
    assert_eq!(
        last,
        format!("\t\"version\": \"{}\"", env!("CARGO_PKG_VERSION"))
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn stats_defaults_and_non_finite() {
    let s = Stats {
        uptime_s: f64::NAN,
        script_max_ms: f64::INFINITY,
        ..Stats::default()
    };
    let out = stats_json(&s, 0);
    let v = assert_json(&out);
    assert_eq!(v["uptime_s"], 0);
    assert_eq!(v["redraws_by_window"], serde_json::json!({}));
    assert_eq!(v["scripts"]["by_item"], serde_json::json!({}));
    assert_eq!(v["scripts"]["max_ms"], 0);
}

#[test]
fn stats_monitor_line_is_one_compact_object() {
    let line = stats_monitor_line(&sample_stats(), 42);
    assert!(line.ends_with("}\n"));
    assert_eq!(line.matches('\n').count(), 1);
    assert!(line.starts_with("{\"type\":\"stats\",\"uptime_s\":1234.5,\"items\":42,"));
    let v = assert_json(line.trim_end());
    assert_eq!(v["type"], "stats");
    assert_eq!(v["events"]["routine"], 600);
}

#[test]
fn stats_query_and_item_named_stats() {
    let mut model = Model::new();
    add_item(&mut model, "x", "left");
    let stats = sample_stats();
    let out = run_with(&model, QueryTarget::Stats, &[], None, &stats, &[]);
    assert_eq!(out, stats_json(&stats, 1));
    // An item called `stats` keeps answering like in SketchyBar.
    add_item(&mut model, "stats", "left");
    let out = run_with(&model, QueryTarget::Stats, &[], None, &stats, &[]);
    assert!(out.starts_with("{\n\t\"name\": \"stats\""));
}

#[test]
fn menus_json_array() {
    let titles = vec!["Finder".to_string(), "File".into(), "Say \"hi\"".into()];
    let out = menus_json(&titles);
    assert_eq!(out, "[\"Finder\",\"File\",\"Say \\\"hi\\\"\"]\n");
    assert_eq!(
        assert_json(&out),
        serde_json::json!(["Finder", "File", "Say \"hi\""])
    );
    assert_eq!(menus_json(&[]), "[]\n");
    let mut model = Model::new();
    assert_eq!(
        run_with(
            &model,
            QueryTarget::Menus,
            &[],
            None,
            &Stats::default(),
            &titles
        ),
        out
    );
    add_item(&mut model, "menus", "right");
    assert!(run_with(
        &model,
        QueryTarget::Menus,
        &[],
        None,
        &Stats::default(),
        &titles
    )
    .starts_with("{\n\t\"name\": \"menus\""));
}

#[test]
fn menus_without_accessibility_permission() {
    let model = Model::new();
    let cx = QueryCx {
        model: &model,
        displays: &[],
        menu_extras: Some(&[]),
        stats: &Stats::default(),
        menus: None,
    };
    let out = query(&QueryTarget::Menus, &cx);
    assert!(out.starts_with("[!]"));
    assert!(out.to_lowercase().contains("accessibility"));
}
