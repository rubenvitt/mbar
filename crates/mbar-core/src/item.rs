//! Bar items (`bar_item.c`, `docs/spec/item.md`).
//!
//! [`BarItem`] holds every field of `struct bar_item` with SketchyBar's defaults (§1),
//! the item-level property parser (§3.2), lengths/heights used by layout and width
//! animations (§4.1), default-item inheritance and `--clone` semantics (§1.7, §10.1), and
//! the `--query <item>` serializer (§11).
//!
//! Per-item *windows* do not exist in mbar (one window per bar per display): `frames`
//! holds the item's virtual window frame per display (screen coordinates), written by
//! layout and used for `bounding_rects`, hit testing and bracket bounds.

use crate::components::{Alias, AppMenu, Background, Graph, Slider, Text};
use crate::event::{EventKind, EventMask};
use crate::geometry::{Point, Rect};
use crate::platform::Resources;
use crate::popup::Popup;
use crate::props::{
    set_bool, set_i32, set_u32, AnimValue, PropCx, PropEffects, PropError, PropRequest, PropResult,
};
use crate::provider::ProviderConfig;
use crate::script::{EnvVars, DEFAULT_SPACE_SCRIPT};
use crate::value::{self, split_key, KeySplit};
use std::fmt::Write;

/// Stable item identity (survives reorder/rename; never reused within a runtime).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

/// `bar_item.type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemType {
    /// `'i'`
    Item,
    /// `'s'`
    Space,
    /// `'a'`
    Alias,
    /// `'b'` (group)
    Bracket,
    /// `'g'`
    Graph,
    /// `'t'`
    Slider,
    /// Extension: front-app menus (`docs/EXTENSIONS.md`).
    AppMenu,
}

impl ItemType {
    /// `bar_item_set_type` matching of the `--add` type token. Returns the type and whether
    /// the token was recognised (anything else becomes `item`; `[?] Add <name>: Invalid
    /// type '<type>', assuming 'item'\n` unless the token was literally `item`).
    pub fn from_add_token(t: &str) -> (ItemType, bool) {
        match t {
            "item" => (ItemType::Item, true),
            "space" => (ItemType::Space, true),
            "alias" => (ItemType::Alias, true),
            "bracket" => (ItemType::Bracket, true),
            "graph" => (ItemType::Graph, true),
            "slider" => (ItemType::Slider, true),
            "app_menu" => (ItemType::AppMenu, true),
            _ => (ItemType::Item, false),
        }
    }

    /// `"type"` in `--query`.
    pub fn query_name(self) -> &'static str {
        match self {
            ItemType::Item => "item",
            ItemType::Space => "space",
            ItemType::Alias => "alias",
            ItemType::Bracket => "bracket",
            ItemType::Graph => "graph",
            ItemType::Slider => "slider",
            ItemType::AppMenu => "app_menu",
        }
    }
}

/// Item position, chosen by the **first character** of the value (§2.1/§2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Position {
    /// `l`
    Left,
    /// `q`: left of the notch, grows leftwards.
    CenterLeft,
    /// `c`
    Center,
    /// `e`: right of the notch, grows rightwards.
    CenterRight,
    /// `r`
    Right,
    /// `p`: popup member.
    Popup,
}

impl Position {
    pub fn from_byte(b: u8) -> Option<Position> {
        Some(match b {
            b'l' => Position::Left,
            b'q' => Position::CenterLeft,
            b'c' => Position::Center,
            b'e' => Position::CenterRight,
            b'r' => Position::Right,
            b'p' => Position::Popup,
            _ => return None,
        })
    }

    /// First character of a position value (`left`, `lol`, `popup.x`, …).
    pub fn parse(v: &str) -> Option<Position> {
        v.as_bytes().first().and_then(|b| Position::from_byte(*b))
    }

    pub fn as_byte(self) -> u8 {
        match self {
            Position::Left => b'l',
            Position::CenterLeft => b'q',
            Position::Center => b'c',
            Position::CenterRight => b'e',
            Position::Right => b'r',
            Position::Popup => b'p',
        }
    }

    /// `"position"` in `--query`.
    pub fn query_name(self) -> &'static str {
        match self {
            Position::Left => "left",
            Position::Right => "right",
            Position::Center => "center",
            Position::CenterLeft => "q",
            Position::CenterRight => "e",
            Position::Popup => "popup",
        }
    }

    /// `r` and `q` fill right-to-left (and draw graphs RTL).
    pub fn is_rtl(self) -> bool {
        matches!(self, Position::Right | Position::CenterLeft)
    }
}

/// `struct bar_item` (§1.1).
#[derive(Debug, Clone, PartialEq)]
pub struct BarItem {
    pub id: ItemId,
    /// `None` after `reset` on the default item (prints `(null)`).
    pub name: Option<String>,
    pub item_type: ItemType,
    /// Tick counter for `update_freq` and `scroll_texts`.
    pub counter: u32,
    /// Dirty flag (redraw).
    pub needs_update: bool,
    /// `updates=on/off`; space items manage it themselves.
    pub updates: bool,
    /// `updates=when_shown`.
    pub updates_only_when_shown: bool,
    /// Space items: selected.
    pub selected: bool,
    /// Hover state (`mouse.entered` de-duplication).
    pub mouse_over: bool,
    pub ignore_association: bool,
    /// Space items: `display=` was given explicitly.
    pub overrides_association: bool,
    pub drawing: bool,
    /// macOS window shadow of the item window (stored; mbar has no item windows).
    pub shadow: bool,
    pub has_const_width: bool,
    pub custom_width: u32,
    pub scroll_texts: bool,
    /// Raw align byte; set to the position char on every non-popup position change.
    pub align: u8,
    /// Blur behind the item (scene `BlurRegion`).
    pub blur_radius: u32,
    /// `display=active`.
    pub associated_to_active_display: bool,
    /// Bit `adid-1`: drawn on that bar in the last draw pass (`is_shown`).
    pub associated_bar: u32,
    /// Bit `n` = arrangement display `n`; 0 = all.
    pub associated_display: u32,
    /// Bit `n` = mission-control space `n`; 0 = all.
    pub associated_space: u32,
    /// `update_freq` in 1 s ticks; 0 disables routine updates.
    pub update_frequency: u32,
    /// After `~` expansion; `Some("")` disables but prints `""`.
    pub script: Option<String>,
    pub click_script: Option<String>,
    /// Persistent env (`signal_args.env_vars`): `NAME`, `SENDER` (Q3), `SELECTED`, `SID`,
    /// `DID`, `PERCENTAGE`.
    pub env: EnvVars,
    pub position: Position,
    pub y_offset: i32,
    /// Item padding lives in `background.padding_left/right`.
    pub background: Background,
    pub icon: Text,
    pub label: Text,
    pub graph: Graph,
    pub alias: Alias,
    pub slider: Slider,
    pub app_menu: AppMenu,
    /// The bracket this item was most recently added to (`item.group`).
    pub group: Option<ItemId>,
    /// Bracket items: their members (`group.members[1..]`), in insertion order.
    pub bracket_members: Vec<ItemId>,
    /// Subscribed event bits.
    pub update_mask: EventMask,
    /// Virtual window frame per display (index `adid-1`), screen coordinates; `None` = no
    /// window on that display yet. Hidden windows sit at (-9999, -9999).
    pub frames: Vec<Option<Rect>>,
    pub popup: Popup,
    /// Popup host when this item is a popup member.
    pub parent: Option<ItemId>,
    /// `mach_helper=<bootstrap name>`.
    pub mach_helper: Option<String>,
    /// Native provider (extension).
    pub provider: ProviderConfig,
    /// In-process Lua handler (extension; `script` then reads `lua:<id>`).
    pub lua_handler: Option<u64>,
    /// This is `g_bar_manager.default_item` (`graph.*`/`alias.*`/`slider.*` accepted).
    pub is_default: bool,
}

impl BarItem {
    /// `bar_item_create` + `bar_item_init(item, NULL)`: factory defaults (§1), no name.
    pub fn new(id: ItemId) -> Self {
        BarItem {
            id,
            name: None,
            item_type: ItemType::Item,
            counter: 0,
            needs_update: true,
            updates: true,
            updates_only_when_shown: false,
            selected: false,
            mouse_over: false,
            ignore_association: false,
            overrides_association: false,
            drawing: true,
            shadow: false,
            has_const_width: false,
            custom_width: 0,
            scroll_texts: false,
            align: b'l',
            blur_radius: 0,
            associated_to_active_display: false,
            associated_bar: 0,
            associated_display: 0,
            associated_space: 0,
            update_frequency: 0,
            script: None,
            click_script: None,
            env: EnvVars::new(),
            position: Position::Left,
            y_offset: 0,
            background: Background::default(),
            icon: Text::default(),
            label: Text::default(),
            graph: Graph::default(),
            alias: Alias::default(),
            slider: Slider::default(),
            app_menu: AppMenu::default(),
            group: None,
            bracket_members: Vec::new(),
            update_mask: EventMask::default(),
            frames: Vec::new(),
            popup: Popup::default(),
            parent: None,
            mach_helper: None,
            provider: ProviderConfig::default(),
            lua_handler: None,
            is_default: false,
        }
    }

    /// The `--default` template item, named `"defaults"`.
    pub fn new_default() -> Self {
        let mut d = BarItem::new(ItemId(0));
        d.name = Some("defaults".to_string());
        d.is_default = true;
        d
    }

    /// `reset=…` on any `--set`/`--default`: `bar_item_init(&default_item, NULL)` — the name
    /// becomes NULL (quirk 18). `update_mask` survives (not reset by `bar_item_init`).
    pub fn reset_default(&mut self) {
        let mask = self.update_mask;
        *self = BarItem::new(self.id);
        self.is_default = true;
        self.update_mask = mask;
    }

    /// `bar_manager_create_item`: a new item initialised from the default item
    /// (`bar_item_init(item, &default_item)` → `bar_item_inherit_from_item`).
    pub fn from_default(id: ItemId, default: &BarItem, res: &mut dyn Resources) -> Self {
        let mut item = BarItem::new(id);
        item.inherit_from(default, res);
        item
    }

    /// `bar_item_inherit_from_item(self, ancestor)` (§1.7, §10.1): a struct copy of the
    /// ancestor, then pointers cleared and selectively deep-copied:
    ///
    /// * kept from `self`: `id`, `name` (NULL at creation; the clone path sets it later),
    ///   `is_default` (false);
    /// * cleared: `group`, env, `frames`, popup items/window (scalars incl. `drawing` copied);
    /// * icon/label/knob: `text_copy` (string, font family/style/size/typographical_width;
    ///   **features lost**), forced relayout; icon/label/item background pictures copied
    ///   (`image_copy`), their paths lost (`(null)`); knob/slider pictures dropped;
    /// * script/click_script copied;
    /// * graph samples deep-copied (D6); bracket member list copied (D7);
    /// * space items: env `SELECTED=false`, `SID=<ancestor DID>` (B1, kept), `DID=<ancestor DID>`.
    pub fn inherit_from(&mut self, ancestor: &BarItem, res: &mut dyn Resources) {
        let id = self.id;
        let name = self.name.take();
        let is_default = self.is_default;
        let mut item = ancestor.clone();
        item.id = id;
        item.name = name;
        item.is_default = is_default;
        item.group = None;
        item.env = EnvVars::new();
        item.frames = Vec::new();
        item.popup = ancestor.popup.inherited();
        item.background = ancestor.background.inherited(true);
        item.icon = ancestor.icon.inherited(true, res);
        item.label = ancestor.label.inherited(true, res);
        item.slider = ancestor.slider.inherited(res);
        item.script = ancestor.script.clone();
        item.click_script = ancestor.click_script.clone();
        if item.item_type == ItemType::Space {
            let did = ancestor.env.get("DID").unwrap_or("0").to_string();
            item.env.set("SELECTED", "false");
            item.env.set("SID", did.clone());
            item.env.set("DID", did);
        }
        *self = item;
    }

    /// `bar_item_set_name` (§2.1 step 5): same name → true; empty → false (rejected);
    /// otherwise store and set env `NAME`.
    pub fn set_name(&mut self, name: &str) -> bool {
        if self.name.as_deref() == Some(name) {
            return true;
        }
        if name.is_empty() {
            return false;
        }
        self.name = Some(name.to_string());
        self.env.set("NAME", name);
        true
    }

    /// `bar_item_set_type` (§1.6, §2.1 step 3) for an already-parsed type. Space items get
    /// the default highlight script if none was inherited, `space_change` subscription,
    /// `updates=off` and env `SELECTED=false`, `SID=0`, `DID=0`.
    pub fn set_type(&mut self, t: ItemType, home: &str) {
        self.item_type = t;
        if t == ItemType::Space {
            if self.script.is_none() {
                self.set_script(DEFAULT_SPACE_SCRIPT, home);
            }
            self.update_mask.insert(EventKind::SpaceChange.bit());
            self.updates = false;
            self.updates_only_when_shown = false;
            self.env.set("SELECTED", "false");
            self.env.set("SID", "0");
            self.env.set("DID", "0");
        }
    }

    pub fn has_graph(&self) -> bool {
        self.item_type == ItemType::Graph
    }
    pub fn has_slider(&self) -> bool {
        self.item_type == ItemType::Slider
    }
    pub fn has_alias(&self) -> bool {
        self.item_type == ItemType::Alias
    }
    pub fn is_bracket(&self) -> bool {
        self.item_type == ItemType::Bracket
    }

    /// `bar_item_set_position` (§2.3): rejects an empty value or a first character outside
    /// `l q c e r p` (returns `None`, nothing changes). On success returns the popup parent
    /// the item must be removed from (`popup_remove_item`; `parent` itself is **not**
    /// cleared, C quirk), sets the position and, unless popup, `align`.
    pub fn set_position(&mut self, v: &str) -> Option<Option<ItemId>> {
        let pos = Position::parse(v)?;
        let detach = self.parent;
        self.position = pos;
        if pos != Position::Popup {
            self.align = pos.as_byte();
        }
        Some(detach)
    }

    /// `bar_item_set_script`: identical raw value → no-op; else store `resolve_path(v)`.
    pub fn set_script(&mut self, v: &str, home: &str) {
        if self.script.as_deref() == Some(v) {
            return;
        }
        self.script = Some(value::resolve_path(v, home));
    }

    pub fn set_click_script(&mut self, v: &str, home: &str) {
        if self.click_script.as_deref() == Some(v) {
            return;
        }
        self.click_script = Some(value::resolve_path(v, home));
    }

    /// `bar_item_set_width` (§4.2).
    pub fn set_width(&mut self, w: i32) -> bool {
        if w < 0 {
            return set_bool(&mut self.has_const_width, false);
        }
        if self.custom_width == w as u32 && self.has_const_width {
            return false;
        }
        self.custom_width = w as u32;
        self.has_const_width = true;
        true
    }

    /// `bar_item_append_associated_space(1 << n)` (space items: replace and set `SID`).
    fn append_space_bit(&mut self, n: u32) {
        let bit = 1u32 << n;
        if self.associated_space & bit != 0 {
            return;
        }
        self.associated_space |= bit;
        if self.item_type == ItemType::Space {
            self.associated_space = bit;
            self.env.set("SID", n.to_string());
        }
    }

    /// `bar_item_append_associated_display(1 << n)` (space items: replace, override the
    /// automatic association, set `DID`).
    fn append_display_bit(&mut self, n: u32) {
        let bit = 1u32 << n;
        if self.associated_display & bit != 0 {
            return;
        }
        self.associated_display |= bit;
        if self.item_type == ItemType::Space {
            self.overrides_association = true;
            self.associated_display = bit;
            self.env.set("DID", n.to_string());
        }
    }

    /// Lazy text relayout for icon, label and knob (call before reading lengths).
    pub fn ensure_layout(&mut self, res: &mut dyn Resources) {
        self.icon.ensure_layout(res);
        self.label.ensure_layout(res);
        self.slider.knob.ensure_layout(res);
    }

    /// `bar_item_get_content_length` (§4.1): icon + label + graph/slider/alias, `max(…, 0)`.
    pub fn content_length(&self) -> u32 {
        let mut len = (self.icon.length(false) as i32).wrapping_add(self.label.length(false) as i32);
        if self.has_graph() && self.graph.enabled {
            len = len.wrapping_add(self.graph.width as i32);
        }
        if self.has_slider() {
            len = len.wrapping_add(self.slider.width as i32);
        }
        if self.has_alias() {
            len = len.wrapping_add(self.alias.length() as i32);
        }
        len.max(0) as u32
    }

    /// `bar_item_get_length(ignore_override)` (§4.1): `false` = slot length, `true` =
    /// display length. Item padding is not included.
    pub fn length(&self, ignore_override: bool) -> u32 {
        let mut c = self.content_length();
        if self.background.enabled && self.background.image.enabled {
            let iw = self.background.image.reserved_size().width;
            if iw > c as f32 {
                c = iw.max(0.0) as u32;
            }
        }
        if self.has_const_width && (!ignore_override || self.custom_width > c) {
            return self.custom_width;
        }
        c
    }

    /// `bar_item_get_height` (§4.1). Uses the background height of the current pass (D11:
    /// layout updates `background.height` before calling this).
    pub fn height(&self) -> u32 {
        let text = self.label.height().max(self.icon.height());
        let item = text.max(if self.has_alias() { self.alias.height() } else { 0 });
        let bg = if self.background.enabled {
            let ih = if self.background.image.enabled {
                self.background.image.reserved_size().height.max(0.0) as u32
            } else {
                0
            };
            ih.max(self.background.height)
        } else {
            0
        };
        item.max(bg)
    }

    /// `bar_item_calculate_shadow_offsets` (§4.4): `(left, right)` extents, each the
    /// double sum truncated to int once.
    pub fn shadow_extents(&self) -> (i32, i32) {
        let shadows = [
            &self.background.shadow,
            &self.icon.shadow,
            &self.icon.background.shadow,
            &self.label.background.shadow,
            &self.label.shadow,
        ];
        let mut left = 0.0f64;
        let mut right = 0.0f64;
        for s in shadows {
            if s.enabled {
                left += (-s.offset.x as f64).max(0.0);
                right += (s.offset.x as f64).max(0.0);
            }
        }
        if self.background.enabled {
            left += (-self.background.x_offset as f64).max(0.0);
            right += (self.background.x_offset as f64).max(0.0);
        }
        (left as i32, right as i32)
    }

    /// `bar_item_is_shown`.
    pub fn is_shown(&self) -> bool {
        self.associated_bar != 0
    }

    /// Frame of the virtual window on display `adid` (1-based).
    pub fn frame(&self, adid: u32) -> Option<Rect> {
        if adid == 0 {
            return None;
        }
        self.frames.get(adid as usize - 1).copied().flatten()
    }

    /// Stores the virtual window frame for `adid` (layout), growing the table.
    pub fn set_frame(&mut self, adid: u32, frame: Rect) {
        if adid == 0 {
            return;
        }
        let i = adid as usize - 1;
        if self.frames.len() <= i {
            self.frames.resize(i + 1, None);
        }
        self.frames[i] = Some(frame);
    }

    /// Point inside any virtual window (inclusive on all edges, `cgrect_contains_point`).
    pub fn contains_point(&self, p: Point) -> bool {
        self.frames.iter().flatten().any(|r| {
            p.x >= r.x && p.x <= r.max_x() && p.y >= r.y && p.y <= r.max_y()
        })
    }

    fn err_name(&self) -> Option<String> {
        self.name.clone()
    }

    /// `bar_item_parse_set_message` (§3.2, `cli.md` §6.11.1): the sub-domain split is
    /// checked **first**, then the leaf keys in C's order. Returns C's `needs_refresh`
    /// (the caller sets `needs_update`). Cross-item work is pushed as `PropRequest`s.
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        if let KeySplit::Sub(sub, rest) = split_key(key) {
            return match sub {
                "icon" => cx.scoped("icon", |cx| self.icon.set_prop(rest, v, cx)),
                "label" => cx.scoped("label", |cx| self.label.set_prop(rest, v, cx)),
                "background" => cx.scoped("background", |cx| self.background.set_prop(rest, v, cx)),
                "popup" => cx.scoped("popup", |cx| self.popup.set_prop(rest, v, cx)),
                "graph" => {
                    if self.has_graph() || self.is_default {
                        cx.scoped("graph", |cx| self.graph.set_prop(rest, v, cx))
                    } else {
                        Err(PropError::NotGraph { item: self.err_name() })
                    }
                }
                "alias" => {
                    if self.has_alias() || self.is_default {
                        cx.scoped("alias", |cx| self.alias.set_prop(rest, v, cx))
                    } else {
                        Err(PropError::NotAlias { item: self.err_name() })
                    }
                }
                "slider" => {
                    if self.has_slider() || self.is_default {
                        cx.scoped("slider", |cx| self.slider.set_prop(rest, v, cx))
                    } else {
                        Err(PropError::NotSlider { item: self.err_name() })
                    }
                }
                "app_menu" => {
                    if self.item_type == ItemType::AppMenu || self.is_default {
                        cx.scoped("app_menu", |cx| self.app_menu.set_prop(rest, v, cx))
                    } else {
                        Err(PropError::NotAppMenu { item: self.err_name() })
                    }
                }
                "provider" => {
                    let name = self.name.clone();
                    self.provider.set_prop(rest, v, name.as_deref(), cx)
                }
                _ => Err(PropError::ItemInvalidSubdomain {
                    item: self.err_name(),
                    sub: sub.to_string(),
                }),
            };
        }
        Ok(match key {
            "icon" => return cx.scoped("icon", |cx| self.icon.set_prop("string", v, cx)),
            "label" => return cx.scoped("label", |cx| self.label.set_prop("string", v, cx)),
            "updates" => {
                if v == "when_shown" {
                    self.updates = true;
                    self.updates_only_when_shown = true;
                } else {
                    self.updates = value::parse_bool(v, self.updates);
                    self.updates_only_when_shown = false;
                }
                false
            }
            "drawing" => {
                let on = value::parse_bool(v, self.drawing);
                set_bool(&mut self.drawing, on)
            }
            "scroll_texts" => {
                self.scroll_texts = value::parse_bool(v, self.scroll_texts);
                false
            }
            "width" => {
                self.ensure_layout(cx.res);
                let pads = self.background.padding_left.wrapping_add(self.background.padding_right);
                if v == "dynamic" {
                    let from = AnimValue::Int(self.custom_width as i32);
                    let to = AnimValue::Int((self.length(true) as i32).wrapping_add(pads));
                    let r = cx.animate("width", from, to, |a, _| self.set_width(a.as_i32()));
                    let cw = self.custom_width as i32;
                    cx.queue_chained(
                        "width",
                        AnimValue::Int(cw),
                        AnimValue::Int(-1),
                        0,
                        crate::animation::Curve::Linear,
                    );
                    r
                } else {
                    let extra = if self.has_const_width { 0 } else { pads };
                    let from = AnimValue::Int((self.length(false) as i32).wrapping_add(extra));
                    cx.animate("width", from, AnimValue::Int(value::parse_int(v)), |a, _| {
                        self.set_width(a.as_i32())
                    })
                }
            }
            "script" => {
                self.set_script(v, cx.home);
                false
            }
            "click_script" => {
                self.set_click_script(v, cx.home);
                false
            }
            "update_freq" => {
                self.update_frequency = value::parse_u32(v);
                false
            }
            "position" => {
                if let Some(Some(_old_parent)) = self.set_position(v) {
                    cx.request(PropRequest::RemoveFromParentPopup);
                }
                match split_key(v) {
                    KeySplit::Sub(k, host) if k.starts_with('p') => {
                        cx.request(PropRequest::AddToPopup { host: host.to_string() });
                    }
                    KeySplit::Sub(_, _) => self.parent = None,
                    _ => {}
                }
                true
            }
            "align" => {
                let a = value::first_byte(v);
                let changed = self.align != a;
                self.align = a;
                changed
            }
            "associated_space" | "space" => {
                let prev = self.associated_space;
                self.associated_space = 0;
                for entry in value::split_list(v) {
                    let n = value::parse_u32(entry);
                    if n >= 32 {
                        cx.respond(PropError::ItemInvalidSpace {
                            item: self.err_name(),
                            entry: entry.to_string(),
                        });
                        continue;
                    }
                    self.append_space_bit(n);
                }
                prev != self.associated_space
            }
            "associated_display" | "display" => {
                let prev = self.associated_display;
                self.associated_display = 0;
                self.associated_to_active_display = false;
                for entry in value::split_list(v) {
                    if entry == "active" {
                        self.associated_to_active_display = true;
                        continue;
                    }
                    let n = value::parse_u32(entry);
                    if n >= 32 {
                        cx.respond(PropError::ItemInvalidDisplay {
                            item: self.err_name(),
                            entry: entry.to_string(),
                        });
                        continue;
                    }
                    self.append_display_bit(n);
                }
                prev != self.associated_display
            }
            "y_offset" => cx.animate(
                "y_offset",
                AnimValue::Int(self.y_offset),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_i32(&mut self.y_offset, a.as_i32()),
            ),
            "padding_left" => {
                let bg = &mut self.background;
                cx.scoped("background", |cx| {
                    cx.animate(
                        "padding_left",
                        AnimValue::Int(bg.padding_left),
                        AnimValue::Int(value::parse_int(v)),
                        |a, _| set_i32(&mut bg.padding_left, a.as_i32()),
                    )
                })
            }
            "padding_right" => {
                let bg = &mut self.background;
                cx.scoped("background", |cx| {
                    cx.animate(
                        "padding_right",
                        AnimValue::Int(bg.padding_right),
                        AnimValue::Int(value::parse_int(v)),
                        |a, _| set_i32(&mut bg.padding_right, a.as_i32()),
                    )
                })
            }
            "blur_radius" => cx.animate(
                "blur_radius",
                AnimValue::Int(self.blur_radius as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_u32(&mut self.blur_radius, a.as_u32()),
            ),
            "shadow" => {
                let on = value::parse_bool(v, self.shadow);
                set_bool(&mut self.shadow, on)
            }
            "ignore_association" => {
                self.ignore_association = value::parse_bool(v, self.ignore_association);
                true
            }
            "reset" => {
                cx.request(PropRequest::ResetDefaultItem);
                false
            }
            "mach_helper" => {
                if !v.is_empty() {
                    self.mach_helper = Some(v.to_string());
                }
                false
            }
            "provider" => {
                let name = self.name.clone();
                return self.provider.set_prop("", v, name.as_deref(), cx);
            }
            _ => {
                return Err(PropError::ItemInvalidProperty {
                    item: self.err_name(),
                    key: value::display_key(key).to_string(),
                })
            }
        })
    }

    /// Animation frame for any animatable item path (`y_offset`, `blur_radius`, `width`,
    /// `icon.…`, `label.…`, `background.…`, `popup.…`, `graph.…`, `alias.…`, `slider.…`,
    /// `app_menu.…`).
    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match path {
            "y_offset" => set_i32(&mut self.y_offset, v.as_i32()),
            "blur_radius" => set_u32(&mut self.blur_radius, v.as_u32()),
            "width" => self.set_width(v.as_i32()),
            _ => match split_key(path) {
                KeySplit::Sub("icon", rest) => self.icon.anim_set(rest, v, fx),
                KeySplit::Sub("label", rest) => self.label.anim_set(rest, v, fx),
                KeySplit::Sub("background", rest) => self.background.anim_set(rest, v, fx),
                KeySplit::Sub("popup", rest) => self.popup.anim_set(rest, v, fx),
                KeySplit::Sub("graph", rest) => self.graph.anim_set(rest, v, fx),
                KeySplit::Sub("alias", rest) => self.alias.anim_set(rest, v, fx),
                KeySplit::Sub("slider", rest) => self.slider.anim_set(rest, v, fx),
                KeySplit::Sub("app_menu", rest) => self.app_menu.anim_set(rest, v, fx),
                _ => false,
            },
        }
    }

    /// `"updates"` in `--query`.
    fn updates_name(&self) -> &'static str {
        if self.updates_only_when_shown {
            "when_shown"
        } else {
            value::format_bool(self.updates)
        }
    }

    /// `bar_item_serialize` (§11), including the final `\n}\n`. `name_of` resolves popup
    /// member and bracket member names.
    pub fn to_json(&self, name_of: &dyn Fn(ItemId) -> Option<String>) -> String {
        let mut out = String::new();
        let o = &mut out;
        let _ = write!(
            o,
            "{{\n\t\"name\": \"{}\",\n\t\"type\": \"{}\",\n\t\"geometry\": {{\n\
             \t\t\"drawing\": \"{}\",\n\t\t\"position\": \"{}\",\n\
             \t\t\"associated_space_mask\": {},\n\t\t\"associated_display_mask\": {},\n\
             \t\t\"ignore_association\": \"{}\",\n\t\t\"y_offset\": {},\n\
             \t\t\"padding_left\": {},\n\t\t\"padding_right\": {},\n\
             \t\t\"scroll_texts\": \"{}\",\n\t\t\"width\": {},\n\t\t\"background\": {{\n",
            value::json_opt(self.name.as_deref()),
            self.item_type.query_name(),
            value::format_bool(self.drawing),
            self.position.query_name(),
            self.associated_space,
            self.associated_display,
            value::format_bool(self.ignore_association),
            self.y_offset,
            self.background.padding_left,
            self.background.padding_right,
            value::format_bool(self.scroll_texts),
            if self.has_const_width { self.custom_width as i32 } else { -1 },
        );
        self.background.write_json(o, "\t\t\t", true);
        o.push_str("\n\t\t}\n\t},\n\t\"icon\": {\n");
        self.icon.write_json(o, "\t\t");
        o.push_str("\n\t},\n\t\"label\": {\n");
        self.label.write_json(o, "\t\t");
        o.push_str("\n\t},\n");
        let script = match self.lua_handler {
            Some(h) => format!("lua:{h}"),
            None => value::json_opt(self.script.as_deref()),
        };
        let _ = write!(
            o,
            "\t\"scripting\": {{\n\t\t\"script\": \"{}\",\n\t\t\"click_script\": \"{}\",\n\
             \t\t\"update_freq\": {},\n\t\t\"update_mask\": {},\n\t\t\"updates\": \"{}\"\n\t}},\n",
            script,
            value::json_opt(self.click_script.as_deref()),
            self.update_frequency,
            self.update_mask.0,
            self.updates_name(),
        );
        o.push_str("\t\"bounding_rects\": {\n");
        let mut n = 0;
        for (i, f) in self.frames.iter().enumerate() {
            let Some(r) = f else { continue };
            if n > 0 {
                o.push_str(",\n");
            }
            n += 1;
            let _ = write!(
                o,
                "\t\t\"display-{}\": {{\n\t\t\t\"origin\": [ {}, {} ],\n\t\t\t\"size\": [ {}, {} ]\n\t\t}}",
                i + 1,
                value::fmt_f(r.x as f64),
                value::fmt_f(r.y as f64),
                value::fmt_f(r.width as f64),
                value::fmt_f(r.height as f64),
            );
        }
        o.push_str("\n\t}");
        if !self.popup.items.is_empty() {
            o.push_str(",\n\t\"popup\": {\n");
            self.popup.write_json(o, "\t\t", name_of);
            o.push_str("\n\t}");
        }
        match self.item_type {
            ItemType::Bracket => {
                o.push_str(",\n\t\"bracket\": [\n");
                for (k, m) in self.bracket_members.iter().enumerate() {
                    if k > 0 {
                        o.push_str(",\n");
                    }
                    let _ = write!(o, "\t\t\"{}\"", value::json_opt(name_of(*m).as_deref()));
                }
                o.push_str("\n\t]");
            }
            ItemType::Graph => {
                o.push_str(",\n\t\"graph\": {\n");
                self.graph.write_json(o, "\t\t");
                o.push_str("\n\t}");
            }
            ItemType::Slider => {
                o.push_str(",\n\t\"slider\": {\n");
                self.slider.write_json(o, "\t\t");
                o.push_str("\n\t}");
            }
            ItemType::AppMenu => {
                o.push_str(",\n\t\"app_menu\": {\n");
                self.app_menu.write_json(o, "\t\t", &self.label.font);
                o.push_str("\n\t}");
            }
            _ => {}
        }
        if self.provider.kind.is_some() {
            o.push_str(",\n\t\"provider\": {\n");
            self.provider.write_json(o, "\t\t");
            o.push_str("\n\t}");
        }
        o.push_str("\n}\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Animator, Curve};
    use crate::platform::HeadlessResources;
    use crate::props::{AnimSpec, AnimTarget};

    fn cx<'a>(res: &'a mut HeadlessResources, an: &'a mut Animator) -> PropCx<'a> {
        let mut c = PropCx::new(res, an, "/home/u");
        c.set_target(AnimTarget::Item(ItemId(1)));
        c
    }

    fn new_item(res: &mut HeadlessResources) -> BarItem {
        let d = BarItem::new_default();
        let mut it = BarItem::from_default(ItemId(1), &d, res);
        it.set_name("foo");
        it
    }

    #[test]
    fn fresh_item_query_matches_cli_9_2() {
        let mut res = HeadlessResources::default();
        let mut it = new_item(&mut res);
        it.set_frame(1, Rect::new(120.0, 900.0, 2.0, 25.0));
        let json = it.to_json(&|_| None);
        let text_block = "\t\t\"value\": \"\",\n\t\t\"drawing\": \"on\",\n\t\t\"highlight\": \"off\",\n\
\t\t\"color\": \"0xffffffff\",\n\t\t\"highlight_color\": \"0xff000000\",\n\t\t\"padding_left\": 0,\n\
\t\t\"padding_right\": 0,\n\t\t\"y_offset\": 0,\n\t\t\"font\": \"Hack Nerd Font:Bold:14.00\",\n\
\t\t\"width\": 0,\n\t\t\"scroll_duration\": 100,\n\t\t\"align\": \"left\",\n\t\t\"background\": {\n\
\t\t\t\"drawing\": \"off\",\n\t\t\t\"color\": \"0x0\",\n\t\t\t\"border_color\": \"0x0\",\n\
\t\t\t\"border_width\": 0,\n\t\t\t\"height\": 0,\n\t\t\t\"corner_radius\": 0,\n\t\t\t\"padding_left\": 0,\n\
\t\t\t\"padding_right\": 0,\n\t\t\t\"x_offset\": 0,\n\t\t\t\"y_offset\": 0,\n\t\t\t\"clip\": 0.000000,\n\
\t\t\t\"image\": {\n\t\t\t\t\"value\": \"(null)\",\n\t\t\t\t\"drawing\": \"off\",\n\t\t\t\t\"scale\": 1.000000\n\t\t\t},\n\
\t\t\t\"shadow\": {\n\t\t\t\t\"drawing\": \"off\",\n\t\t\t\t\"color\": \"0xff000000\",\n\t\t\t\t\"angle\": 30,\n\
\t\t\t\t\"distance\": 5\n\t\t\t}\n\t\t},\n\t\t\"shadow\": {\n\t\t\t\"drawing\": \"off\",\n\
\t\t\t\"color\": \"0xff000000\",\n\t\t\t\"angle\": 30,\n\t\t\t\"distance\": 5\n\t\t}";
        let expected = format!(
            "{{\n\t\"name\": \"foo\",\n\t\"type\": \"item\",\n\t\"geometry\": {{\n\t\t\"drawing\": \"on\",\n\
\t\t\"position\": \"left\",\n\t\t\"associated_space_mask\": 0,\n\t\t\"associated_display_mask\": 0,\n\
\t\t\"ignore_association\": \"off\",\n\t\t\"y_offset\": 0,\n\t\t\"padding_left\": 0,\n\t\t\"padding_right\": 0,\n\
\t\t\"scroll_texts\": \"off\",\n\t\t\"width\": -1,\n\t\t\"background\": {{\n\t\t\t\"drawing\": \"off\",\n\
\t\t\t\"color\": \"0x0\",\n\t\t\t\"border_color\": \"0x0\",\n\t\t\t\"border_width\": 0,\n\t\t\t\"height\": 0,\n\
\t\t\t\"corner_radius\": 0,\n\t\t\t\"padding_left\": 0,\n\t\t\t\"padding_right\": 0,\n\t\t\t\"x_offset\": 0,\n\
\t\t\t\"y_offset\": 0,\n\t\t\t\"clip\": 0.000000,\n\t\t\t\"image\": {{\n\t\t\t\t\"value\": \"(null)\",\n\
\t\t\t\t\"drawing\": \"off\",\n\t\t\t\t\"scale\": 1.000000\n\t\t\t}},\n\t\t\t\"shadow\": {{\n\
\t\t\t\t\"drawing\": \"off\",\n\t\t\t\t\"color\": \"0xff000000\",\n\t\t\t\t\"angle\": 30,\n\
\t\t\t\t\"distance\": 5\n\t\t\t}}\n\t\t}}\n\t}},\n\t\"icon\": {{\n{text_block}\n\t}},\n\t\"label\": {{\n{text_block}\n\t}},\n\
\t\"scripting\": {{\n\t\t\"script\": \"(null)\",\n\t\t\"click_script\": \"(null)\",\n\t\t\"update_freq\": 0,\n\
\t\t\"update_mask\": 0,\n\t\t\"updates\": \"on\"\n\t}},\n\t\"bounding_rects\": {{\n\t\t\"display-1\": {{\n\
\t\t\t\"origin\": [ 120.000000, 900.000000 ],\n\t\t\t\"size\": [ 2.000000, 25.000000 ]\n\t\t}}\n\t}}\n}}\n"
        );
        assert_eq!(json, expected);
        // No windows: empty body.
        let it2 = new_item(&mut res);
        assert!(it2.to_json(&|_| None).contains("\t\"bounding_rects\": {\n\n\t}\n}\n"));
    }

    #[test]
    fn item_props() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut it = new_item(&mut res);
        assert_eq!(it.icon.width, 1, "inherited empty text is laid out");
        let mut c = cx(&mut res, &mut an);
        assert!(it.set_prop("icon", "ab", &mut c).unwrap());
        assert!(it.set_prop("padding_left", "5", &mut c).unwrap());
        assert_eq!(it.background.padding_left, 5);
        assert_eq!(
            it.set_prop("lazy", "on", &mut c).unwrap_err().to_string(),
            "[!] Item (foo): Invalid property 'lazy' \n"
        );
        assert_eq!(
            it.set_prop("icon.", "x", &mut c).unwrap_err().to_string(),
            "[!] Item (foo): Invalid property 'icon' \n"
        );
        assert_eq!(
            it.set_prop("foo.bar", "x", &mut c).unwrap_err().to_string(),
            "[!] Item (foo): Invalid subdomain 'foo'\n"
        );
        assert_eq!(
            it.set_prop("graph.color", "1", &mut c).unwrap_err().to_string(),
            "[!] Item (foo): Trying to set a graph property on a non-graph item\n"
        );
        assert!(!it.set_prop("updates", "when_shown", &mut c).unwrap());
        assert!(it.updates && it.updates_only_when_shown);
        assert!(it.set_prop("space", "1,3", &mut c).unwrap());
        assert_eq!(it.associated_space, 10);
        assert!(!it.set_prop("display", "active", &mut c).unwrap());
        assert!(it.associated_to_active_display);
        assert!(it.set_prop("display", "1,40", &mut c).unwrap());
        assert_eq!(it.associated_display, 2);
        assert_eq!(c.response, "[!] Item (foo): Invalid display '40'\n");
        assert!(it.set_prop("position", "right", &mut c).unwrap());
        assert_eq!((it.position, it.align), (Position::Right, b'r'));
        assert!(it.set_prop("position", "popup.host", &mut c).unwrap());
        assert_eq!(it.align, b'r', "popup keeps align");
        assert_eq!(c.requests, vec![PropRequest::AddToPopup { host: "host".into() }]);
        assert!(it.set_prop("position", "xyz", &mut c).unwrap(), "invalid is silent, still refresh");
        assert_eq!(it.position, Position::Popup);
        it.set_prop("script", "~/s.sh", &mut c).unwrap();
        assert_eq!(it.script.as_deref(), Some("/home/u/s.sh"));
        assert!(!it.set_prop("reset", "1", &mut c).unwrap());
        assert_eq!(c.requests.last(), Some(&PropRequest::ResetDefaultItem));
    }

    #[test]
    fn widths() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut it = new_item(&mut res);
        {
            let mut c = cx(&mut res, &mut an);
            it.set_prop("label", "abcd", &mut c).unwrap();
            it.set_prop("padding_left", "4", &mut c).unwrap();
            it.set_prop("padding_right", "6", &mut c).unwrap();
        }
        // icon 1 + label 35
        assert_eq!(it.content_length(), 36);
        {
            let mut c = cx(&mut res, &mut an);
            c.anim = Some(AnimSpec { curve: Curve::Linear, duration: 10 });
            assert!(!it.set_prop("width", "100", &mut c).unwrap());
        }
        let a = &an.animations()[0];
        assert_eq!((a.path.as_str(), a.from, a.to), ("width", AnimValue::Int(46), AnimValue::Int(100)));
        an.clear();
        {
            let mut c = cx(&mut res, &mut an);
            assert!(it.set_prop("width", "100", &mut c).unwrap());
        }
        assert_eq!(it.length(false), 100);
        assert_eq!(it.length(true), 100);
        {
            let mut c = cx(&mut res, &mut an);
            assert!(it.set_prop("width", "dynamic", &mut c).unwrap());
        }
        assert_eq!(it.custom_width, 36 + 10);
        assert_eq!(an.len(), 1, "chained -1 step");
        assert!(it.anim_set("width", AnimValue::Int(-1), &mut PropEffects::default()));
        assert!(!it.has_const_width);
    }

    #[test]
    fn space_items_and_clone() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let d = BarItem::new_default();
        let mut sp = BarItem::from_default(ItemId(2), &d, &mut res);
        sp.set_type(ItemType::Space, "/h");
        sp.set_name("s1");
        assert_eq!(sp.script.as_deref(), Some(DEFAULT_SPACE_SCRIPT));
        assert!(sp.update_mask.has(EventKind::SpaceChange));
        assert!(!sp.updates);
        {
            let mut c = cx(&mut res, &mut an);
            sp.set_prop("space", "1,3", &mut c).unwrap();
            sp.set_prop("display", "2", &mut c).unwrap();
        }
        assert_eq!(sp.associated_space, 1 << 3);
        assert_eq!(sp.env.get("SID"), Some("3"));
        assert_eq!(sp.env.get("DID"), Some("2"));
        assert!(sp.overrides_association);
        let mut cl = BarItem::from_default(ItemId(3), &d, &mut res);
        cl.inherit_from(&sp, &mut res);
        cl.set_name("s2");
        assert_eq!(cl.env.get("SID"), Some("2"), "B1: SID from parent's DID");
        assert_eq!(cl.env.get("NAME"), Some("s2"));
        assert_eq!(cl.item_type, ItemType::Space);
    }

    #[test]
    fn default_inheritance() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut d = BarItem::new_default();
        {
            let mut c = PropCx::new(&mut res, &mut an, "/h");
            c.set_target(AnimTarget::Default);
            d.set_prop("icon.font.features", "+tnum", &mut c).unwrap();
            d.set_prop("label.color", "0xff00ff00", &mut c).unwrap();
            d.set_prop("graph.color", "0xff112233", &mut c).unwrap();
            d.set_prop("update_freq", "5", &mut c).unwrap();
        }
        let it = BarItem::from_default(ItemId(9), &d, &mut res);
        assert_eq!(it.icon.font.features, None);
        assert_eq!(it.label.color.hex, 0xff00ff00);
        assert_eq!(it.update_frequency, 5);
        assert_eq!(it.name, None);
        assert!(!it.is_default);
        d.reset_default();
        assert_eq!(d.name, None);
        assert!(d.to_json(&|_| None).starts_with("{\n\t\"name\": \"(null)\""));
    }
}
