//! Pure data model of the management UI: parsing of `--query` / `--monitor` output,
//! the inspector tree, property flattening, `--set` key mapping, "copy as CLI/Lua"
//! and the event log. Nothing in here depends on GPUI, so it is unit-tested on any host.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde_json::Value;

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Parses daemon JSON. SketchyBar prints string values unescaped (only `script` and
/// `click_script` escape `"` and newlines), so a label containing `"` or `\` yields
/// invalid JSON. The query format puts every value on its own line, which allows a
/// line-based repair when strict parsing fails.
pub fn parse_json_lenient(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty response".into());
    }
    match serde_json::from_str(trimmed) {
        Ok(v) => Ok(v),
        Err(strict_err) => {
            let repaired = repair_json_lines(trimmed);
            serde_json::from_str(&repaired).map_err(|_| strict_err.to_string())
        }
    }
}

fn repair_json_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for line in text.lines() {
        out.push_str(&repair_line(line));
        out.push('\n');
    }
    out
}

/// Re-escapes the string value of a line shaped like `<ws>"key": "value"[,]` or
/// `<ws>"value"[,]`.
fn repair_line(line: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let rest_trim = rest.trim_end();
    let (body, comma) = match rest_trim.strip_suffix(',') {
        Some(b) => (b, ","),
        None => (rest_trim, ""),
    };
    if !body.starts_with('"') || !body.ends_with('"') || body.len() < 2 {
        return line.to_string();
    }
    // Keyed value: the key never contains quotes, so the first `": "` separates it.
    if let Some(sep) = body.find("\": \"") {
        let key = &body[..sep + 1];
        let value = &body[sep + 4..body.len() - 1];
        return format!("{indent}{key}: \"{}\"{comma}", escape_loose(value));
    }
    if body.contains("\": ") {
        // `"key": <non-string>` — nothing to repair.
        return line.to_string();
    }
    let value = &body[1..body.len() - 1];
    format!("{indent}\"{}\"{comma}", escape_loose(value))
}

/// Escapes a raw string for JSON while keeping the escapes SketchyBar already emits
/// (`\"`, `\n`) intact.
fn escape_loose(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 4);
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some('"') | Some('n') | Some('t') | Some('r') | Some('\\') | Some('/') => {
                    out.push('\\');
                    out.push(chars.next().unwrap_or('\\'));
                }
                _ => out.push_str("\\\\"),
            },
            '"' => out.push_str("\\\""),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Formats a JSON scalar the way the daemon would accept it back in `key=value`.
pub fn display_value(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => if *b { "on" } else { "off" }.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                format_float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(display_value).collect::<Vec<_>>().join(", "),
        Value::Object(_) => v.to_string(),
    }
}

/// `1.000000` → `1`, `0.5` → `0.5`.
pub fn format_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        let s = format!("{f:.6}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    }
}

fn get_str<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_str()
}

fn get_f64(v: &Value, path: &[&str]) -> f64 {
    let mut cur = v;
    for p in path {
        match cur.get(*p) {
            Some(n) => cur = n,
            None => return 0.0,
        }
    }
    match cur {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn string_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Bar / items
// ---------------------------------------------------------------------------

/// `--query bar`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarInfo {
    /// Item names in `bar_items` order.
    pub items: Vec<String>,
    pub json: Value,
}

impl BarInfo {
    pub fn parse(text: &str) -> Result<BarInfo, String> {
        let json = parse_json_lenient(text)?;
        if !json.is_object() {
            return Err("bar query did not return an object".into());
        }
        Ok(BarInfo {
            items: string_list(json.get("items")),
            json,
        })
    }
}

/// Item position as reported by `geometry.position`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Position {
    Left,
    Center,
    Right,
    /// Left of the notch.
    Q,
    /// Right of the notch.
    E,
    Popup,
    Unknown,
}

impl Position {
    pub fn parse(s: &str) -> Position {
        match s {
            "left" | "l" => Position::Left,
            "center" | "c" => Position::Center,
            "right" | "r" => Position::Right,
            "q" => Position::Q,
            "e" => Position::E,
            "popup" | "p" => Position::Popup,
            _ => Position::Unknown,
        }
    }

    pub fn group_id(self) -> &'static str {
        match self {
            Position::Left => "group:left",
            Position::Center => "group:center",
            Position::Right => "group:right",
            Position::Q => "group:q",
            Position::E => "group:e",
            Position::Popup => "group:popup",
            Position::Unknown => "group:other",
        }
    }

    pub fn group_label(self) -> &'static str {
        match self {
            Position::Left => "Left",
            Position::Center => "Center",
            Position::Right => "Right",
            Position::Q => "Left of notch (q)",
            Position::E => "Right of notch (e)",
            Position::Popup => "Detached popup items",
            Position::Unknown => "Other",
        }
    }
}

/// `--query <item>`.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemInfo {
    pub name: String,
    pub kind: String,
    pub position: Position,
    pub drawing: bool,
    pub popup_items: Vec<String>,
    pub bracket_members: Vec<String>,
    pub json: Value,
}

impl ItemInfo {
    pub fn parse(fallback_name: &str, text: &str) -> Result<ItemInfo, String> {
        let json = parse_json_lenient(text)?;
        Ok(ItemInfo::from_json(fallback_name, json))
    }

    pub fn from_json(fallback_name: &str, json: Value) -> ItemInfo {
        let name = get_str(&json, &["name"])
            .unwrap_or(fallback_name)
            .to_string();
        let kind = get_str(&json, &["type"]).unwrap_or("item").to_string();
        let position = Position::parse(get_str(&json, &["geometry", "position"]).unwrap_or(""));
        let drawing = get_str(&json, &["geometry", "drawing"]) != Some("off");
        let popup_items = string_list(json.get("popup").and_then(|p| p.get("items")));
        let bracket_members = string_list(json.get("bracket"));
        ItemInfo {
            name,
            kind,
            position,
            drawing,
            popup_items,
            bracket_members,
            json,
        }
    }
}

/// Everything the inspector shows, fetched in one background pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub bar: BarInfo,
    /// Keyed by item name.
    pub items: BTreeMap<String, ItemInfo>,
    /// Items listed by `--query bar` whose own query failed, with the error.
    pub errors: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// Inspector tree
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Group,
    Item,
    BracketMember,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode {
    /// Unique, stable id (`group:left`, `item:clock`, `bracket:b/clock`).
    pub id: String,
    pub label: String,
    /// Item this node selects, if any.
    pub item: Option<String>,
    pub kind: NodeKind,
    /// Item type (`item`, `alias`, `bracket`, ...) for item nodes.
    pub item_type: Option<String>,
    pub drawing: bool,
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    fn group(id: &str, label: &str, children: Vec<TreeNode>) -> TreeNode {
        TreeNode {
            id: id.to_string(),
            label: format!("{label} ({})", children.len()),
            item: None,
            kind: NodeKind::Group,
            item_type: None,
            drawing: true,
            children,
        }
    }

    /// Depth-first visit.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a TreeNode)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }
}

pub fn item_node_id(name: &str) -> String {
    format!("item:{name}")
}

/// Builds the inspector tree: position groups (in `order`), popup items below their
/// host, brackets with their members, then anything that could not be placed.
pub fn build_tree(order: &[String], items: &BTreeMap<String, ItemInfo>) -> Vec<TreeNode> {
    let mut placed: HashSet<&str> = HashSet::new();
    // Popup items are shown below their host; never at top level.
    let hosted: HashSet<&str> = items
        .values()
        .flat_map(|i| i.popup_items.iter().map(String::as_str))
        .collect();

    fn item_node<'a>(
        name: &'a str,
        items: &'a BTreeMap<String, ItemInfo>,
        placed: &mut HashSet<&'a str>,
        depth: usize,
    ) -> TreeNode {
        placed.insert(name);
        let info = items.get(name);
        let mut children = Vec::new();
        if let Some(info) = info {
            if depth < 8 {
                for p in &info.popup_items {
                    if placed.contains(p.as_str()) {
                        continue;
                    }
                    children.push(item_node(p, items, placed, depth + 1));
                }
            }
        }
        TreeNode {
            id: item_node_id(name),
            label: name.to_string(),
            item: Some(name.to_string()),
            kind: NodeKind::Item,
            item_type: info.map(|i| i.kind.clone()),
            drawing: info.map(|i| i.drawing).unwrap_or(true),
            children,
        }
    }

    // Order: `--query bar` order first, then any items only known from their own query.
    let mut ordered: Vec<&str> = order.iter().map(String::as_str).collect();
    for name in items.keys() {
        if !order.iter().any(|o| o == name) {
            ordered.push(name);
        }
    }

    let positions = [
        Position::Left,
        Position::Center,
        Position::Right,
        Position::Q,
        Position::E,
    ];
    let mut groups = Vec::new();
    for pos in positions {
        let mut children = Vec::new();
        for &name in &ordered {
            let Some(info) = items.get(name) else {
                continue;
            };
            if info.kind == "bracket" || info.position != pos || hosted.contains(name) {
                continue;
            }
            if placed.contains(name) {
                continue;
            }
            children.push(item_node(name, items, &mut placed, 0));
        }
        if !children.is_empty() {
            groups.push(TreeNode::group(pos.group_id(), pos.group_label(), children));
        }
    }

    // Brackets.
    let mut brackets = Vec::new();
    for &name in &ordered {
        let Some(info) = items.get(name) else {
            continue;
        };
        if info.kind != "bracket" {
            continue;
        }
        placed.insert(name);
        let members = info
            .bracket_members
            .iter()
            .map(|m| TreeNode {
                id: format!("bracket:{name}/{m}"),
                label: m.clone(),
                item: Some(m.clone()),
                kind: NodeKind::BracketMember,
                item_type: items.get(m).map(|i| i.kind.clone()),
                drawing: items.get(m).map(|i| i.drawing).unwrap_or(true),
                children: Vec::new(),
            })
            .collect();
        brackets.push(TreeNode {
            id: item_node_id(name),
            label: name.to_string(),
            item: Some(name.to_string()),
            kind: NodeKind::Item,
            item_type: Some(info.kind.clone()),
            drawing: info.drawing,
            children: members,
        });
    }
    if !brackets.is_empty() {
        groups.push(TreeNode::group("group:brackets", "Brackets", brackets));
    }

    // Popup items whose host is unknown, other positions, items whose query failed.
    let mut detached = Vec::new();
    let mut other = Vec::new();
    for &name in &ordered {
        if placed.contains(name) {
            continue;
        }
        match items.get(name) {
            Some(info) if info.position == Position::Popup => {
                detached.push(item_node(name, items, &mut placed, 0))
            }
            _ => other.push(item_node(name, items, &mut placed, 0)),
        }
    }
    if !detached.is_empty() {
        groups.push(TreeNode::group(
            Position::Popup.group_id(),
            Position::Popup.group_label(),
            detached,
        ));
    }
    if !other.is_empty() {
        groups.push(TreeNode::group(
            Position::Unknown.group_id(),
            Position::Unknown.group_label(),
            other,
        ));
    }
    groups
}

/// Finds the first node that selects `item`.
pub fn find_item_node<'a>(nodes: &'a [TreeNode], item: &str) -> Option<&'a TreeNode> {
    let mut found = None;
    for n in nodes {
        n.walk(&mut |node| {
            if found.is_none() && node.item.as_deref() == Some(item) {
                found = Some(node);
            }
        });
    }
    found
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// Which object a property table edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Bar,
    Item(String),
}

/// One row of a property table.
#[derive(Clone, Debug, PartialEq)]
pub struct PropRow {
    /// Dotted path in the query JSON (`icon.background.color`).
    pub path: String,
    pub value: String,
    /// `--set`/`--bar` key, or `None` when the value is read-only.
    pub set_key: Option<String>,
}

/// Flattens a JSON object to `(dotted.path, leaf)` pairs in document order. Arrays of
/// scalars are kept as one leaf; arrays of objects are indexed (`path.0.key`).
pub fn flatten_json(v: &Value) -> Vec<(String, Value)> {
    fn rec(prefix: &str, v: &Value, out: &mut Vec<(String, Value)>) {
        match v {
            Value::Object(map) => {
                if map.is_empty() && !prefix.is_empty() {
                    out.push((prefix.to_string(), Value::String(String::new())));
                }
                for (k, child) in map {
                    let path = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    rec(&path, child, out);
                }
            }
            Value::Array(a) if a.iter().any(|x| x.is_object() || x.is_array()) => {
                for (i, child) in a.iter().enumerate() {
                    rec(&format!("{prefix}.{i}"), child, out);
                }
            }
            leaf => out.push((prefix.to_string(), leaf.clone())),
        }
    }
    let mut out = Vec::new();
    rec("", v, &mut out);
    out
}

/// Maps an item query path to its `--set` key (`geometry.drawing` → `drawing`,
/// `icon.value` → `icon`, `background.image.value` → `background.image`). Returns
/// `None` for read-only values.
pub fn item_set_key(path: &str) -> Option<String> {
    const READ_ONLY: &[&str] = &[
        "name",
        "type",
        "bracket",
        "popup.items",
        "graph.data",
        "scripting.update_mask",
        "geometry.associated_space_mask",
        "geometry.associated_display_mask",
    ];
    if READ_ONLY.contains(&path) || path.starts_with("bounding_rects") {
        return None;
    }
    let mut key = path
        .strip_prefix("geometry.")
        .or_else(|| path.strip_prefix("scripting."))
        .unwrap_or(path)
        .to_string();
    if let Some(stripped) = key.strip_suffix(".value") {
        key = stripped.to_string();
    }
    if key == "value" || key.is_empty() {
        return None;
    }
    Some(key)
}

/// Maps a bar query path to its `--bar` key.
pub fn bar_set_key(path: &str) -> Option<String> {
    // `clip` is serialized with the bar background but `--bar clip=` is always rejected.
    if path == "items" || path.starts_with("items.") || path == "clip" {
        return None;
    }
    let key = path.strip_suffix(".value").unwrap_or(path);
    Some(key.to_string())
}

pub fn property_rows(target: &Target, json: &Value) -> Vec<PropRow> {
    flatten_json(json)
        .into_iter()
        .map(|(path, leaf)| {
            let set_key = match target {
                Target::Bar => bar_set_key(&path),
                Target::Item(_) => item_set_key(&path),
            };
            PropRow {
                value: display_value(&leaf),
                path,
                set_key,
            }
        })
        .collect()
}

/// Case-insensitive filter over path and value.
pub fn filter_rows<'a>(rows: &'a [PropRow], filter: &str) -> Vec<&'a PropRow> {
    let f = filter.trim().to_lowercase();
    rows.iter()
        .filter(|r| {
            f.is_empty()
                || r.path.to_lowercase().contains(&f)
                || r.value.to_lowercase().contains(&f)
        })
        .collect()
}

/// IPC arguments applying `pairs` to `target`.
pub fn set_args(target: &Target, pairs: &[(String, String)]) -> Vec<String> {
    let mut args = Vec::with_capacity(pairs.len() + 2);
    match target {
        Target::Bar => args.push("--bar".to_string()),
        Target::Item(name) => {
            args.push("--set".to_string());
            args.push(name.clone());
        }
    }
    for (k, v) in pairs {
        args.push(format!("{k}={v}"));
    }
    args
}

/// POSIX shell quoting (only when needed).
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '-' | '.' | '/' | ':' | '=' | ',' | '+' | '@' | '%')
        });
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `mbar --set clock label='a b'`.
pub fn cli_command(binary: &str, target: &Target, pairs: &[(String, String)]) -> String {
    let mut parts = vec![binary.to_string()];
    parts.extend(set_args(target, pairs).iter().map(|a| shell_quote(a)));
    parts.join(" ")
}

fn lua_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\{:03}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn lua_value(v: &str) -> String {
    let t = v.trim();
    let is_hex = t.len() > 2
        && (t.starts_with("0x") || t.starts_with("0X"))
        && t[2..].chars().all(|c| c.is_ascii_hexdigit());
    let is_num = !t.is_empty()
        && t == v
        && t.parse::<f64>().is_ok()
        && !t.contains(['e', 'E', 'n', 'N', 'i', 'I']);
    if is_hex || is_num {
        t.to_string()
    } else {
        lua_string(v)
    }
}

fn lua_ident_or_key(k: &str) -> String {
    let ident = !k.is_empty()
        && !k.starts_with(|c: char| c.is_ascii_digit())
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ident {
        k.to_string()
    } else {
        format!("[{}]", lua_string(k))
    }
}

#[derive(Default)]
struct LuaTable {
    leaf: Option<String>,
    children: BTreeMap<String, LuaTable>,
    order: Vec<String>,
}

impl LuaTable {
    fn insert(&mut self, path: &[&str], value: String) {
        match path.split_first() {
            None => self.leaf = Some(value),
            Some((head, rest)) => {
                if !self.children.contains_key(*head) {
                    self.order.push(head.to_string());
                }
                self.children
                    .entry(head.to_string())
                    .or_default()
                    .insert(rest, value)
            }
        }
    }

    fn render(&self) -> String {
        let mut parts = Vec::new();
        for k in &self.order {
            let child = &self.children[k];
            let rendered = if child.children.is_empty() {
                child.leaf.clone().unwrap_or_else(|| "nil".into())
            } else if let Some(leaf) = &child.leaf {
                // `icon=x` and `icon.color=y` together: the positional entry of a nested
                // table is the key's own value (`{ "x", color = y }`, see mbar-lua).
                let mut t = child.render_inner();
                t.insert(0, leaf.clone());
                format!("{{ {} }}", t.join(", "))
            } else {
                child.render()
            };
            parts.push(format!("{} = {}", lua_ident_or_key(k), rendered));
        }
        format!("{{ {} }}", parts.join(", "))
    }

    fn render_inner(&self) -> Vec<String> {
        let s = self.render();
        let inner = s.trim_start_matches("{ ").trim_end_matches(" }");
        if inner.is_empty() || inner == "{}" {
            Vec::new()
        } else {
            vec![inner.to_string()]
        }
    }
}

/// Lua (mbar-lua, SbarLua-style API): `mbar.set("clock", { label = { color = 0xffffffff } })`
/// or `mbar.bar({ height = 32 })`. Dotted keys become nested tables.
pub fn lua_snippet(target: &Target, pairs: &[(String, String)]) -> String {
    let mut table = LuaTable::default();
    for (k, v) in pairs {
        let path: Vec<&str> = k.split('.').collect();
        table.insert(&path, lua_value(v));
    }
    match target {
        Target::Bar => format!("mbar.bar({})", table.render()),
        Target::Item(name) => format!("mbar.set({}, {})", lua_string(name), table.render()),
    }
}

/// Editable `(key, value)` pairs of a property table.
pub fn editable_pairs(rows: &[PropRow]) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|r| r.set_key.as_ref().map(|k| (k.clone(), r.value.clone())))
        .collect()
}

/// Pairs for "copy everything": the editable pairs minus values that do not round-trip
/// through `--set`. Unset strings print as `(null)`; text `width` prints the last fixed
/// width even while the width is dynamic; an unset popup `height` prints `-1`; and
/// `position=popup` without a host would detach a popup item.
pub fn export_pairs(rows: &[PropRow]) -> Vec<(String, String)> {
    editable_pairs(rows)
        .into_iter()
        .filter(|(k, v)| {
            let text_width = matches!(
                k.as_str(),
                "icon.width" | "label.width" | "slider.knob.width"
            );
            !(v == "(null)"
                || text_width
                || (k == "popup.height" && v == "-1")
                || (k == "position" && v == "popup"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timing {
    pub avg: f64,
    pub p95: f64,
    pub max: f64,
}

impl Timing {
    fn from(v: &Value, key: &str) -> Timing {
        Timing {
            avg: get_f64(v, &[key, "avg"]),
            p95: get_f64(v, &[key, "p95"]),
            max: get_f64(v, &[key, "max"]),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptItemStats {
    pub name: String,
    pub runs: u64,
    pub avg_ms: f64,
    pub max_ms: f64,
}

impl ScriptItemStats {
    pub fn total_ms(&self) -> f64 {
        self.runs as f64 * self.avg_ms
    }
}

/// `--query stats` (see docs/EXTENSIONS.md). Missing fields read as zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub uptime_s: f64,
    pub items: u64,
    pub windows: u64,
    pub frames: u64,
    /// Microseconds.
    pub frame_time_us: Timing,
    /// Microseconds.
    pub layout_time_us: Timing,
    /// Sorted by redraw count, descending.
    pub redraws_by_window: Vec<(String, u64)>,
    pub scripts_spawned: u64,
    pub scripts_running: u64,
    pub scripts_avg_ms: f64,
    pub scripts_max_ms: f64,
    /// Sorted by total time (runs × avg), descending.
    pub scripts_by_item: Vec<ScriptItemStats>,
    pub lua_callbacks: u64,
    pub lua_avg_us: f64,
    pub lua_max_us: f64,
    /// Sorted by count, descending.
    pub events: Vec<(String, u64)>,
    pub ipc_messages: u64,
}

fn count_map(v: Option<&Value>) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = match v {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0).max(0.0) as u64))
            .collect(),
        _ => Vec::new(),
    };
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

impl Stats {
    pub fn parse(text: &str) -> Result<Stats, String> {
        let v = parse_json_lenient(text)?;
        Stats::from_json(&v)
    }

    pub fn from_json(v: &Value) -> Result<Stats, String> {
        if !v.is_object() {
            return Err("stats is not an object".into());
        }
        let u = |path: &[&str]| get_f64(v, path).max(0.0) as u64;
        let mut by_item: Vec<ScriptItemStats> =
            match v.get("scripts").and_then(|s| s.get("by_item")) {
                Some(Value::Object(m)) => m
                    .iter()
                    .map(|(name, s)| ScriptItemStats {
                        name: name.clone(),
                        runs: get_f64(s, &["runs"]).max(0.0) as u64,
                        avg_ms: get_f64(s, &["avg_ms"]),
                        max_ms: get_f64(s, &["max_ms"]),
                    })
                    .collect(),
                _ => Vec::new(),
            };
        by_item.sort_by(|a, b| {
            b.total_ms()
                .partial_cmp(&a.total_ms())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(Stats {
            uptime_s: get_f64(v, &["uptime_s"]),
            items: u(&["items"]),
            windows: u(&["windows"]),
            frames: u(&["frames"]),
            frame_time_us: Timing::from(v, "frame_time_us"),
            layout_time_us: Timing::from(v, "layout_time_us"),
            redraws_by_window: count_map(v.get("redraws_by_window")),
            scripts_spawned: u(&["scripts", "spawned"]),
            scripts_running: u(&["scripts", "running"]),
            scripts_avg_ms: get_f64(v, &["scripts", "avg_ms"]),
            scripts_max_ms: get_f64(v, &["scripts", "max_ms"]),
            scripts_by_item: by_item,
            lua_callbacks: u(&["lua", "callbacks"]),
            lua_avg_us: get_f64(v, &["lua", "avg_us"]),
            lua_max_us: get_f64(v, &["lua", "max_us"]),
            events: count_map(v.get("events")),
            ipc_messages: u(&["ipc_messages"]),
        })
    }
}

/// One point of the frame-time sparkline.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameSample {
    /// Seconds since the first sample (label of the x axis).
    pub t: String,
    pub avg_us: f64,
    pub p95_us: f64,
    /// Frames rendered since the previous sample.
    pub frames: u64,
}

/// Ring buffer of stats samples for the sparkline.
#[derive(Clone, Debug, Default)]
pub struct StatsHistory {
    pub samples: VecDeque<FrameSample>,
    last_frames: Option<u64>,
    count: u64,
}

impl StatsHistory {
    pub const CAPACITY: usize = 120;

    pub fn push(&mut self, s: &Stats) {
        let frames = match self.last_frames {
            Some(prev) if s.frames >= prev => s.frames - prev,
            _ => 0,
        };
        self.last_frames = Some(s.frames);
        self.samples.push_back(FrameSample {
            t: format!("{}s", self.count),
            avg_us: s.frame_time_us.avg,
            p95_us: s.frame_time_us.p95,
            frames,
        });
        self.count += 1;
        while self.samples.len() > Self::CAPACITY {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        *self = StatsHistory::default();
    }
}

// ---------------------------------------------------------------------------
// Events (`--monitor`)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct EventRecord {
    pub name: String,
    pub sender: String,
    /// `INFO` as text (JSON values are re-serialized compactly).
    pub info: String,
    /// Items whose handlers ran.
    pub items: Vec<String>,
    pub ts_ms: u64,
}

impl EventRecord {
    pub fn matches(&self, filter_lower: &str) -> bool {
        filter_lower.is_empty()
            || self.name.to_lowercase().contains(filter_lower)
            || self.sender.to_lowercase().contains(filter_lower)
            || self.info.to_lowercase().contains(filter_lower)
            || self
                .items
                .iter()
                .any(|i| i.to_lowercase().contains(filter_lower))
    }

    /// `HH:MM:SS.mmm` (UTC wall clock of `ts_ms`).
    pub fn time_label(&self) -> String {
        let ms = self.ts_ms % 1000;
        let secs = self.ts_ms / 1000;
        let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
        format!("{h:02}:{m:02}:{s:02}.{ms:03}")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MonitorMessage {
    Event(EventRecord),
    Stats(Box<Stats>),
    /// A non-JSON or unknown line, kept verbatim.
    Other(String),
}

pub fn parse_monitor_line(line: &str) -> Option<MonitorMessage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let Ok(v) = parse_json_lenient(line) else {
        return Some(MonitorMessage::Other(line.to_string()));
    };
    match v.get("type").and_then(Value::as_str) {
        Some("event") => {
            let info = match v.get("info") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
            };
            Some(MonitorMessage::Event(EventRecord {
                name: get_str(&v, &["name"]).unwrap_or("").to_string(),
                sender: get_str(&v, &["sender"]).unwrap_or("").to_string(),
                info,
                items: string_list(v.get("items")),
                ts_ms: get_f64(&v, &["ts_ms"]).max(0.0) as u64,
            }))
        }
        Some("stats") => Stats::from_json(&v)
            .ok()
            .map(|s| MonitorMessage::Stats(Box::new(s))),
        _ => Some(MonitorMessage::Other(line.to_string())),
    }
}

/// Bounded event log.
#[derive(Clone, Debug)]
pub struct EventLog {
    entries: VecDeque<EventRecord>,
    capacity: usize,
    /// Total events received (including evicted ones).
    pub received: u64,
}

impl Default for EventLog {
    fn default() -> Self {
        EventLog::new(5000)
    }
}

impl EventLog {
    pub fn new(capacity: usize) -> EventLog {
        EventLog {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
            received: 0,
        }
    }

    pub fn push(&mut self, e: EventRecord) {
        self.received += 1;
        self.entries.push_back(e);
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Indices of matching entries, newest first.
    pub fn filtered(&self, filter: &str) -> Vec<usize> {
        let f = filter.trim().to_lowercase();
        (0..self.entries.len())
            .rev()
            .filter(|&i| self.entries[i].matches(&f))
            .collect()
    }

    pub fn get(&self, ix: usize) -> Option<&EventRecord> {
        self.entries.get(ix)
    }
}

/// Splits a line into words with shell-like quoting (`'…'`, `"…"`, `\`).
pub fn split_words(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unterminated ' quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c) => cur.push(c),
                            None => return Err("trailing backslash".into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("unterminated \" quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(c) => cur.push(c),
                    None => return Err("trailing backslash".into()),
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// `--trigger <event> KEY=VAL ...` from the trigger form.
pub fn trigger_args(event: &str, vars: &str) -> Result<Vec<String>, String> {
    let event = event.trim();
    if event.is_empty() {
        return Err("event name is empty".into());
    }
    if event.chars().any(char::is_whitespace) {
        return Err("event name must not contain whitespace".into());
    }
    let mut args = vec!["--trigger".to_string(), event.to_string()];
    for w in split_words(vars)? {
        match w.split_once('=') {
            Some((k, _)) if !k.is_empty() => args.push(w),
            _ => return Err(format!("expected KEY=VALUE, got '{w}'")),
        }
    }
    Ok(args)
}

/// Helper for the per-window redraw table: `HashMap` → sorted rows.
pub fn sorted_counts(map: &HashMap<String, u64>) -> Vec<(String, u64)> {
    let mut v: Vec<_> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Parsed command line of `mbar-ui`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    pub bar_name: String,
    /// Opened by the daemon to show the Sparkle update dialog only.
    pub update: bool,
}

/// Command line of `mbar-ui`: `[--bar-name <name>] [--update]`. Returns the parsed
/// arguments, or `Err` with a message (usage for `--help`).
pub fn parse_cli_args<I: IntoIterator<Item = String>>(args: I) -> Result<CliArgs, String> {
    let usage =
        "usage: mbar-ui [--bar-name <name>] [--update]\n\nManagement app for the mbar status bar. \
         --bar-name selects the daemon instance (default: mbar).";
    let mut bar_name = mbar_ipc::DEFAULT_BAR_NAME.to_string();
    let mut update = false;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "-h" || arg == "--help" {
            return Err(usage.to_string());
        } else if arg == "--bar-name" {
            // A following flag (`--bar-name --update`) is a missing value, not a name.
            bar_name = it
                .next()
                .filter(|v| !v.is_empty() && !v.starts_with('-'))
                .ok_or_else(|| format!("--bar-name needs a value\n\n{usage}"))?;
        } else if let Some(v) = arg.strip_prefix("--bar-name=") {
            if v.is_empty() {
                return Err(format!("--bar-name needs a value\n\n{usage}"));
            }
            bar_name = v.to_string();
        } else if arg == "--update" {
            update = true;
        } else if arg.starts_with("-psn_") {
            // Finder passes a process serial number to apps it launches.
        } else {
            return Err(format!("unknown argument '{arg}'\n\n{usage}"));
        }
    }
    Ok(CliArgs { bar_name, update })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM_FOO: &str = "{\n\t\"name\": \"foo\",\n\t\"type\": \"item\",\n\t\"geometry\": {\n\t\t\"drawing\": \"on\",\n\t\t\"position\": \"left\",\n\t\t\"associated_space_mask\": 0,\n\t\t\"width\": -1,\n\t\t\"background\": {\n\t\t\t\"drawing\": \"off\",\n\t\t\t\"color\": \"0x0\",\n\t\t\t\"clip\": 0.000000,\n\t\t\t\"image\": {\n\t\t\t\t\"value\": \"(null)\",\n\t\t\t\t\"drawing\": \"off\",\n\t\t\t\t\"scale\": 1.000000\n\t\t\t}\n\t\t}\n\t},\n\t\"icon\": {\n\t\t\"value\": \"\",\n\t\t\"color\": \"0xffffffff\",\n\t\t\"font\": \"Hack Nerd Font:Bold:14.00\"\n\t},\n\t\"label\": {\n\t\t\"value\": \"say \"hi\" \\o/\",\n\t\t\"drawing\": \"on\"\n\t},\n\t\"scripting\": {\n\t\t\"script\": \"echo \\\"x\\\"\\nexit\",\n\t\t\"click_script\": \"(null)\",\n\t\t\"update_freq\": 0,\n\t\t\"update_mask\": 0,\n\t\t\"updates\": \"on\"\n\t},\n\t\"bounding_rects\": {\n\t\t\"display-1\": {\n\t\t\t\"origin\": [ 120.000000, 900.000000 ],\n\t\t\t\"size\": [ 2.000000, 25.000000 ]\n\t\t}\n\t}\n}\n";

    fn item(name: &str, kind: &str, pos: &str, popup: &[&str], members: &[&str]) -> ItemInfo {
        let mut json = serde_json::json!({
            "name": name,
            "type": kind,
            "geometry": { "drawing": "on", "position": pos },
        });
        if !popup.is_empty() {
            json["popup"] = serde_json::json!({ "drawing": "off", "items": popup });
        }
        if !members.is_empty() {
            json["bracket"] = serde_json::json!(members);
        }
        ItemInfo::from_json(name, json)
    }

    #[test]
    fn lenient_parse_repairs_unescaped_strings() {
        assert!(serde_json::from_str::<Value>(ITEM_FOO).is_err());
        let v = parse_json_lenient(ITEM_FOO).unwrap();
        assert_eq!(v["label"]["value"], "say \"hi\" \\o/");
        assert_eq!(v["scripting"]["script"], "echo \"x\"\nexit");
        assert_eq!(v["geometry"]["background"]["clip"], 0.0);
        let info = ItemInfo::parse("x", ITEM_FOO).unwrap();
        assert_eq!(info.name, "foo");
        assert_eq!(info.position, Position::Left);
        assert!(info.drawing);
        assert!(parse_json_lenient("").is_err());
        assert!(parse_json_lenient("[!] Query: Item 'x' not found\n").is_err());
    }

    #[test]
    fn lenient_parse_arrays() {
        let bar = "{\n\t\"position\": \"top\",\n\t\"height\": 25,\n\t\"items\": [\n\t\t \"a\",\n\t\t \"we\"ird\"\n\t]\n}\n";
        let b = BarInfo::parse(bar).unwrap();
        assert_eq!(b.items, vec!["a".to_string(), "we\"ird".to_string()]);
        let empty = "{\n\t\"position\": \"top\",\n\t\"items\": [\n\n\t]\n}\n";
        assert!(BarInfo::parse(empty).unwrap().items.is_empty());
    }

    #[test]
    fn display_values() {
        assert_eq!(display_value(&serde_json::json!(1.0)), "1");
        assert_eq!(display_value(&serde_json::json!(0.5)), "0.5");
        assert_eq!(display_value(&serde_json::json!(-1)), "-1");
        assert_eq!(display_value(&serde_json::json!("x")), "x");
        assert_eq!(display_value(&serde_json::json!([1.0, 2.5])), "1, 2.5");
        assert_eq!(display_value(&serde_json::json!(true)), "on");
    }

    #[test]
    fn tree_groups_popups_brackets() {
        let mut items = BTreeMap::new();
        for i in [
            item("apple", "item", "left", &["apple.prefs", "apple.lock"], &[]),
            item("apple.prefs", "item", "popup", &[], &[]),
            item("apple.lock", "item", "popup", &[], &[]),
            item("clock", "item", "right", &[], &[]),
            item("cpu", "graph", "right", &[], &[]),
            item("title", "item", "center", &[], &[]),
            item("notch_l", "item", "q", &[], &[]),
            item("notch_r", "item", "e", &[], &[]),
            item("status", "bracket", "right", &[], &["clock", "cpu"]),
            item("orphan", "item", "popup", &[], &[]),
        ] {
            items.insert(i.name.clone(), i);
        }
        let order: Vec<String> = [
            "apple",
            "apple.prefs",
            "apple.lock",
            "title",
            "cpu",
            "clock",
            "notch_l",
            "notch_r",
            "status",
            "orphan",
            "ghost",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let tree = build_tree(&order, &items);
        let ids: Vec<&str> = tree.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "group:left",
                "group:center",
                "group:right",
                "group:q",
                "group:e",
                "group:brackets",
                "group:popup",
                "group:other"
            ]
        );
        let left = &tree[0];
        assert_eq!(left.label, "Left (1)");
        assert_eq!(left.children[0].id, "item:apple");
        let popup_ids: Vec<&str> = left.children[0]
            .children
            .iter()
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(popup_ids, vec!["item:apple.prefs", "item:apple.lock"]);
        // Right keeps bar order.
        let right: Vec<&str> = tree[2].children.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(right, vec!["cpu", "clock"]);
        assert_eq!(tree[2].children[0].item_type.as_deref(), Some("graph"));
        // Bracket with members selecting the member item.
        let br = &tree[5].children[0];
        assert_eq!(br.item.as_deref(), Some("status"));
        assert_eq!(br.children[1].id, "bracket:status/cpu");
        assert_eq!(br.children[1].item.as_deref(), Some("cpu"));
        assert_eq!(br.children[1].kind, NodeKind::BracketMember);
        assert_eq!(tree[6].children[0].label, "orphan");
        // Item listed by the bar but whose query failed.
        assert_eq!(tree[7].children[0].label, "ghost");
        // Every id is unique.
        let mut seen = HashSet::new();
        for n in &tree {
            n.walk(&mut |node| assert!(seen.insert(node.id.clone()), "{}", node.id));
        }
        assert_eq!(
            find_item_node(&tree, "cpu").map(|n| n.id.as_str()),
            Some("item:cpu")
        );
    }

    #[test]
    fn set_key_mapping() {
        assert_eq!(item_set_key("geometry.drawing").as_deref(), Some("drawing"));
        assert_eq!(
            item_set_key("geometry.background.color").as_deref(),
            Some("background.color")
        );
        assert_eq!(
            item_set_key("geometry.background.image.value").as_deref(),
            Some("background.image")
        );
        assert_eq!(item_set_key("icon.value").as_deref(), Some("icon"));
        assert_eq!(item_set_key("label.font").as_deref(), Some("label.font"));
        assert_eq!(
            item_set_key("icon.background.shadow.angle").as_deref(),
            Some("icon.background.shadow.angle")
        );
        assert_eq!(item_set_key("scripting.script").as_deref(), Some("script"));
        assert_eq!(
            item_set_key("slider.knob.value").as_deref(),
            Some("slider.knob")
        );
        assert_eq!(item_set_key("popup.align").as_deref(), Some("popup.align"));
        for ro in [
            "name",
            "type",
            "popup.items",
            "bracket",
            "graph.data",
            "scripting.update_mask",
            "bounding_rects.display-1.origin",
            "geometry.associated_space_mask",
        ] {
            assert_eq!(item_set_key(ro), None, "{ro}");
        }
        assert_eq!(bar_set_key("height").as_deref(), Some("height"));
        assert_eq!(bar_set_key("image.value").as_deref(), Some("image"));
        assert_eq!(bar_set_key("items"), None);
    }

    #[test]
    fn rows_from_item() {
        let v = parse_json_lenient(ITEM_FOO).unwrap();
        let rows = property_rows(&Target::Item("foo".into()), &v);
        let row = |p: &str| rows.iter().find(|r| r.path == p).unwrap().clone();
        assert_eq!(row("geometry.width").value, "-1");
        assert_eq!(row("geometry.background.clip").value, "0");
        assert_eq!(row("label.value").set_key.as_deref(), Some("label"));
        assert_eq!(row("bounding_rects.display-1.origin").value, "120, 900");
        assert_eq!(row("bounding_rects.display-1.origin").set_key, None);
        // Document order is preserved.
        assert_eq!(rows[0].path, "name");
        let filtered = filter_rows(&rows, "FONT");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "icon.font");
        let pairs = editable_pairs(&rows);
        assert!(pairs.iter().all(|(k, _)| k != "name"));
        assert!(pairs.contains(&("icon.font".into(), "Hack Nerd Font:Bold:14.00".into())));
        assert!(pairs.contains(&("script".into(), "echo \"x\"\nexit".into())));
        assert!(pairs.contains(&("click_script".into(), "(null)".into())));
        let export = export_pairs(&rows);
        assert!(export.contains(&("width".into(), "-1".into())));
        assert!(export.contains(&("script".into(), "echo \"x\"\nexit".into())));
        assert!(!export
            .iter()
            .any(|(k, _)| k == "click_script" || k == "background.image"));
        let popup_row = |k: &str, v: &str| PropRow {
            path: k.into(),
            value: v.into(),
            set_key: item_set_key(k),
        };
        let rows2 = vec![
            popup_row("icon.width", "0"),
            popup_row("popup.height", "-1"),
            popup_row("geometry.position", "popup"),
            popup_row("popup.align", "left"),
            popup_row("slider.width", "100"),
        ];
        assert_eq!(
            export_pairs(&rows2),
            vec![
                ("popup.align".into(), "left".into()),
                ("slider.width".into(), "100".into())
            ]
        );
        assert_eq!(bar_set_key("clip"), None);
    }

    #[test]
    fn cli_and_lua() {
        let t = Target::Item("clock".into());
        let pairs = vec![
            ("label".to_string(), "it's 5".to_string()),
            ("icon.color".to_string(), "0xffff0000".to_string()),
            ("width".to_string(), "-1".to_string()),
        ];
        assert_eq!(
            set_args(&t, &pairs),
            vec![
                "--set",
                "clock",
                "label=it's 5",
                "icon.color=0xffff0000",
                "width=-1"
            ]
        );
        assert_eq!(
            cli_command("mbar", &t, &pairs),
            "mbar --set clock 'label=it'\\''s 5' icon.color=0xffff0000 width=-1"
        );
        assert_eq!(
            lua_snippet(&t, &pairs),
            "mbar.set(\"clock\", { label = \"it's 5\", icon = { color = 0xffff0000 }, width = -1 })"
        );
        let bar = vec![
            ("height".to_string(), "32".to_string()),
            ("position".to_string(), "top".to_string()),
            ("shadow.color".to_string(), "0x0".to_string()),
        ];
        assert_eq!(
            cli_command("mbar", &Target::Bar, &bar),
            "mbar --bar height=32 position=top shadow.color=0x0"
        );
        assert_eq!(
            lua_snippet(&Target::Bar, &bar),
            "mbar.bar({ height = 32, position = \"top\", shadow = { color = 0x0 } })"
        );
        // `icon` and `icon.color` together.
        let mixed = vec![
            ("icon".to_string(), "A".to_string()),
            ("icon.color".to_string(), "0x1".to_string()),
        ];
        assert_eq!(
            lua_snippet(&t, &mixed),
            "mbar.set(\"clock\", { icon = { \"A\", color = 0x1 } })"
        );
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(lua_value("nan"), "\"nan\"");
        assert_eq!(lua_value("1.5"), "1.5");
        assert_eq!(lua_value("on"), "\"on\"");
    }

    #[test]
    fn stats_parse() {
        let text = r#"{
  "uptime_s": 1234.5, "items": 42, "windows": 3, "frames": 1200,
  "frame_time_us": { "avg": 310, "p95": 640, "max": 2100 },
  "layout_time_us": { "avg": 45, "p95": 90, "max": 400 },
  "redraws_by_window": { "bar:1": 300, "popup:apple:1": 4 },
  "scripts": { "spawned": 812, "running": 0, "avg_ms": 6.1, "max_ms": 80.0,
               "by_item": { "clock": { "runs": 120, "avg_ms": 3.2, "max_ms": 9.0 },
                            "cpu": { "runs": 10, "avg_ms": 50.0, "max_ms": 80.0 } } },
  "lua": { "callbacks": 400, "avg_us": 35, "max_us": 900 },
  "events": { "front_app_switched": 31, "routine": 600 },
  "ipc_messages": 950
}"#;
        let s = Stats::parse(text).unwrap();
        assert_eq!(s.items, 42);
        assert_eq!(s.frame_time_us.p95, 640.0);
        assert_eq!(s.layout_time_us.max, 400.0);
        assert_eq!(s.redraws_by_window[0], ("bar:1".to_string(), 300));
        assert_eq!(s.scripts_by_item[0].name, "cpu"); // 500 ms total > 384 ms
        assert_eq!(s.scripts_by_item[1].name, "clock");
        assert!((s.scripts_by_item[1].total_ms() - 384.0).abs() < 1e-9);
        assert_eq!(s.lua_max_us, 900.0);
        assert_eq!(s.events[0].0, "routine");
        assert_eq!(s.ipc_messages, 950);
        // Partial objects are fine.
        let p = Stats::parse("{\"frames\": 5}").unwrap();
        assert_eq!(p.frames, 5);
        assert!(p.scripts_by_item.is_empty());
        assert!(Stats::parse("[!] Unknown query").is_err());

        let mut h = StatsHistory::default();
        h.push(&s);
        let mut s2 = s.clone();
        s2.frames = 1260;
        h.push(&s2);
        assert_eq!(h.samples.len(), 2);
        assert_eq!(h.samples[1].frames, 60);
        assert_eq!(h.samples[1].t, "1s");
        for _ in 0..200 {
            h.push(&s2);
        }
        assert_eq!(h.samples.len(), StatsHistory::CAPACITY);
    }

    #[test]
    fn monitor_lines_and_log() {
        let line = r#"{"type":"event","name":"front_app_switched","sender":"front_app_switched","info":"Safari","items":["front_app"],"ts_ms":3723004}"#;
        let Some(MonitorMessage::Event(e)) = parse_monitor_line(line) else {
            panic!("not an event");
        };
        assert_eq!(e.name, "front_app_switched");
        assert_eq!(e.items, vec!["front_app".to_string()]);
        assert_eq!(e.time_label(), "01:02:03.004");
        let json_info = r#"{"type":"event","name":"menus_change","sender":"system","info":["File","Edit"],"items":[],"ts_ms":0}"#;
        let Some(MonitorMessage::Event(e2)) = parse_monitor_line(json_info) else {
            panic!("not an event");
        };
        assert_eq!(e2.info, "[\"File\",\"Edit\"]");
        assert!(matches!(
            parse_monitor_line(r#"{"type":"stats","frames":3}"#),
            Some(MonitorMessage::Stats(s)) if s.frames == 3
        ));
        assert!(matches!(
            parse_monitor_line("garbage"),
            Some(MonitorMessage::Other(_))
        ));
        assert_eq!(parse_monitor_line("  "), None);

        let mut log = EventLog::new(3);
        for i in 0..5 {
            let mut ev = e.clone();
            ev.name = format!("ev{i}");
            log.push(ev);
        }
        assert_eq!(log.len(), 3);
        assert_eq!(log.received, 5);
        let all = log.filtered("");
        assert_eq!(log.get(all[0]).unwrap().name, "ev4"); // newest first
        assert_eq!(log.filtered("EV3").len(), 1);
        assert_eq!(log.filtered("front_app").len(), 3); // matches items
        assert_eq!(log.filtered("nothing").len(), 0);
        log.clear();
        assert!(log.is_empty());
    }

    #[test]
    fn trigger_form() {
        assert_eq!(
            trigger_args("my_event", "FOO=1 BAR='a b' BAZ=\"x\\\"y\"").unwrap(),
            vec!["--trigger", "my_event", "FOO=1", "BAR=a b", "BAZ=x\"y"]
        );
        assert_eq!(trigger_args(" e ", "").unwrap(), vec!["--trigger", "e"]);
        assert!(trigger_args("", "").is_err());
        assert!(trigger_args("a b", "").is_err());
        assert!(trigger_args("e", "NOVALUE").is_err());
        assert!(trigger_args("e", "=x").is_err());
        assert!(trigger_args("e", "A='x").is_err());
        assert_eq!(split_words("a\\ b c").unwrap(), vec!["a b", "c"]);
        assert_eq!(split_words("''").unwrap(), vec![""]);
    }

    #[test]
    fn cli_args() {
        let a = |v: &[&str]| parse_cli_args(v.iter().map(|s| s.to_string()));
        assert_eq!(a(&[]).unwrap().bar_name, "mbar");
        assert_eq!(a(&["--bar-name", "bottom"]).unwrap().bar_name, "bottom");
        assert_eq!(a(&["--bar-name=top"]).unwrap().bar_name, "top");
        assert_eq!(a(&["-psn_0_1234"]).unwrap().bar_name, "mbar");
        assert!(a(&["--bar-name"]).is_err());
        assert!(a(&["--bar-name="]).is_err());
        assert!(a(&["--help"]).unwrap_err().starts_with("usage"));
        assert!(a(&["x"]).is_err());
    }

    #[test]
    fn cli_update_flag() {
        let a = parse_cli_args(["--update".to_string()]).unwrap();
        assert!(a.update);
        assert_eq!(a.bar_name, "mbar");
        let a = parse_cli_args(Vec::<String>::new()).unwrap();
        assert!(!a.update);
        let a = parse_cli_args(["-psn_0_123".into(), "--update".into()]).unwrap();
        assert!(a.update);
        let a = parse_cli_args(["--update".into(), "--bar-name".into(), "top".into()]).unwrap();
        assert!(a.update);
        assert_eq!(a.bar_name, "top");
        assert!(parse_cli_args(["--bar-name".into(), "--update".into()]).is_err());
        assert!(parse_cli_args(["--updates".into()]).is_err());
    }

    #[test]
    fn counts_sorted() {
        let mut m = HashMap::new();
        m.insert("b".to_string(), 1);
        m.insert("a".to_string(), 1);
        m.insert("c".to_string(), 5);
        assert_eq!(
            sorted_counts(&m),
            vec![("c".into(), 5), ("a".into(), 1), ("b".into(), 1)]
        );
    }
}
