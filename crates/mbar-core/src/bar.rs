//! Global bar properties (`--bar`, `bar_manager.c`, `docs/spec/bar.md` §1–2, §9.1) and the
//! per-display bar state (`struct bar`).

use crate::components::Background;
use crate::geometry::Rect;
use crate::platform::level;
use crate::props::{
    set_bool, set_i32, set_u32, AnimValue, HiddenRequest, PropCx, PropEffects, PropError,
    PropRequest, PropResult,
};
use crate::value;
use std::fmt::Write;

/// `DISPLAY_ALL_PATTERN`.
pub const DISPLAY_ALL: u32 = u32::MAX;
/// `DISPLAY_MAIN_PATTERN`: one bar on the main display.
pub const DISPLAY_MAIN: u32 = 0;

/// The `--bar` settings (`g_bar_manager` fields, `bar.md` §1).
#[derive(Debug, Clone, PartialEq)]
pub struct BarProps {
    /// Window system shadow; default **off**. A change recreates all bars.
    pub shadow: bool,
    /// Default off; `window` and `on` both report `on`.
    pub topmost: bool,
    /// Default **on**: windows on all spaces.
    pub sticky: bool,
    pub font_smoothing: bool,
    /// Result of the last `hidden=` (query `hidden`).
    pub any_bar_hidden: bool,
    pub show_in_fullscreen: bool,
    /// Bit `n-1` = arrangement display `n`; `0` = main only; default all.
    pub displays: u32,
    /// Raw first byte: `t` top (default), `b` bottom, `l`/`r` vertical, anything else = top.
    pub position: u8,
    pub margin: i32,
    pub blur_radius: u32,
    /// Default 200; only on built-in displays.
    pub notch_width: u32,
    pub notch_offset: u32,
    pub notch_display_height: u32,
    /// `kCGBackstopMenuLevel` (default), `kCGFloatingWindowLevel` (`topmost=window`),
    /// `kCGStatusWindowLevel` (`topmost=on`).
    pub window_level: i32,
    /// Bar background: height 25 (overriding), padding 20/20, `border_color 0xffff0000`,
    /// `color 0x44000000`, **disabled** (query `drawing off`; drawing always forces it on).
    pub background: Background,
    /// Extension `hide_menubar=on|off`.
    pub hide_menubar: bool,
}

impl Default for BarProps {
    fn default() -> Self {
        let mut background = Background::default();
        background.height = 25;
        background.overrides_height = true;
        background.padding_left = 20;
        background.padding_right = 20;
        background.border_color.set_hex(0xffff0000);
        background.color.set_hex(0x44000000);
        BarProps {
            shadow: false,
            topmost: false,
            sticky: true,
            font_smoothing: false,
            any_bar_hidden: false,
            show_in_fullscreen: false,
            displays: DISPLAY_ALL,
            position: b't',
            margin: 0,
            blur_radius: 0,
            notch_width: 200,
            notch_offset: 0,
            notch_display_height: 0,
            window_level: level::BACKSTOP_MENU,
            background,
            hide_menubar: false,
        }
    }
}

/// `display=` value → pattern (`bar.md` §2.5): `all` → all, `main` → 0 (both overwrite),
/// `n` → `|= 1 << (n-1)`. Entries `0`, non-numeric or > 32 are rejected (mbar, D9) and
/// returned for an error response.
pub fn parse_display_pattern(v: &str) -> (u32, Vec<String>) {
    let mut pattern = 0u32;
    let mut bad = Vec::new();
    for entry in value::split_list(v) {
        match entry {
            "all" => pattern = DISPLAY_ALL,
            "main" => pattern = DISPLAY_MAIN,
            _ => {
                let n = value::parse_u32(entry);
                if n == 0 || n > 32 {
                    bad.push(entry.to_string());
                } else {
                    pattern |= 1u32 << (n - 1);
                }
            }
        }
    }
    (pattern, bad)
}

impl BarProps {
    /// True for `position` `l`/`r`.
    pub fn is_vertical(&self) -> bool {
        matches!(self.position, b'l' | b'r')
    }

    /// Bar thickness / height (`background.bounds.size.height`).
    pub fn height(&self) -> u32 {
        self.background.height
    }

    pub fn set_margin(&mut self, m: i32, fx: &mut PropEffects) -> bool {
        if !set_i32(&mut self.margin, m) {
            return false;
        }
        fx.bar_needs_resize = true;
        true
    }

    /// `bar_manager_set_y_offset`: writes `background.y_offset`.
    pub fn set_y_offset(&mut self, y: i32, fx: &mut PropEffects) -> bool {
        if !set_i32(&mut self.background.y_offset, y) {
            return false;
        }
        fx.bar_needs_resize = true;
        true
    }

    /// `bar_manager_set_background_blur`: applies immediately, always reports false.
    pub fn set_blur_radius(&mut self, r: u32) -> bool {
        set_u32(&mut self.blur_radius, r);
        false
    }

    pub fn set_notch_offset(&mut self, v: u32, fx: &mut PropEffects) -> bool {
        if !set_u32(&mut self.notch_offset, v) {
            return false;
        }
        fx.bar_needs_resize = true;
        true
    }

    pub fn set_notch_display_height(&mut self, v: u32, fx: &mut PropEffects) -> bool {
        if !set_u32(&mut self.notch_display_height, v) {
            return false;
        }
        fx.bar_needs_resize = true;
        true
    }

    /// `bar_manager_set_bar_height`: `resize |= background_set_height(h)`; returns the
    /// current resize flag (may be true even if unchanged).
    pub fn set_height(&mut self, h: u32, fx: &mut PropEffects) -> bool {
        fx.bar_needs_resize |= self.background.set_height(h);
        fx.bar_needs_resize
    }

    /// `handle_domain_bar` (`bar.md` §2.2), checked in C's order; unknown keys fall through
    /// to the bar background (§2.6).
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "margin" => cx.animate(
                "margin",
                AnimValue::Int(self.margin),
                AnimValue::Int(value::parse_int(v)),
                |a, fx| self.set_margin(a.as_i32(), fx),
            ),
            "y_offset" => cx.animate(
                "y_offset",
                AnimValue::Int(self.background.y_offset),
                AnimValue::Int(value::parse_int(v)),
                |a, fx| self.set_y_offset(a.as_i32(), fx),
            ),
            "blur_radius" => cx.animate(
                "blur_radius",
                AnimValue::Int(self.blur_radius as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| self.set_blur_radius(a.as_u32()),
            ),
            "font_smoothing" => {
                let on = value::parse_bool(v, self.font_smoothing);
                set_bool(&mut self.font_smoothing, on)
            }
            "shadow" => {
                let on = value::parse_bool(v, self.shadow);
                let changed = set_bool(&mut self.shadow, on);
                if changed {
                    cx.request(PropRequest::ResetBars);
                }
                changed
            }
            "notch_width" => cx.animate(
                "notch_width",
                AnimValue::Int(self.notch_width as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_u32(&mut self.notch_width, a.as_u32()),
            ),
            "notch_offset" => cx.animate(
                "notch_offset",
                AnimValue::Int(self.notch_offset as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, fx| self.set_notch_offset(a.as_u32(), fx),
            ),
            "notch_display_height" => cx.animate(
                "notch_display_height",
                AnimValue::Int(self.notch_display_height as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, fx| self.set_notch_display_height(a.as_u32(), fx),
            ),
            "hidden" => {
                // The runtime applies it (needs the bars and the active display), marks
                // `bar_needs_update` and returns true (unless `current` fails: false + log).
                let req = if v == "current" {
                    HiddenRequest::Current
                } else {
                    HiddenRequest::All(value::parse_bool(v, self.any_bar_hidden))
                };
                cx.request(PropRequest::BarHidden(req));
                false
            }
            "topmost" => {
                if v == "window" {
                    self.window_level = level::FLOATING;
                    self.topmost = true;
                } else {
                    let on = value::parse_bool(v, self.topmost);
                    self.window_level = if on { level::STATUS } else { level::BACKSTOP_MENU };
                    self.topmost = on;
                }
                // C resets *before* updating `topmost`, so the recreated windows use the old
                // value for the menu-bar offset until the next resize (bar.md §2.4 quirk 9).
                // mbar's reset runs after this setter and sees the new value (resolved in
                // docs/IMPLEMENTATION-PLAN.md "Spec ambiguities").
                cx.request(PropRequest::ResetBars);
                true
            }
            "sticky" => {
                let on = value::parse_bool(v, self.sticky);
                let changed = set_bool(&mut self.sticky, on);
                if changed {
                    cx.request(PropRequest::ResetBars);
                }
                changed
            }
            "display" => {
                let (pattern, bad) = parse_display_pattern(v);
                for b in bad {
                    cx.respond(PropError::BarInvalidDisplay(b));
                }
                let changed = set_u32(&mut self.displays, pattern);
                if changed {
                    cx.request(PropRequest::ResetBars);
                }
                changed
            }
            "position" => {
                if v.is_empty() {
                    false
                } else {
                    let p = value::first_byte(v);
                    if self.position == p {
                        false
                    } else {
                        self.position = p;
                        cx.fx.bar_needs_resize = true;
                        true
                    }
                }
            }
            "clip" => return Err(PropError::BarInvalidClip),
            "height" => cx.animate(
                "height",
                AnimValue::Int(self.background.height as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, fx| self.set_height(a.as_u32(), fx),
            ),
            "show_in_fullscreen" => {
                let on = value::parse_bool(v, self.show_in_fullscreen);
                set_bool(&mut self.show_in_fullscreen, on)
            }
            "hide_menubar" => {
                let on = value::parse_bool(v, self.hide_menubar);
                let changed = set_bool(&mut self.hide_menubar, on);
                if changed {
                    cx.request(PropRequest::MenuBarHidden(on));
                }
                changed
            }
            _ => return self.background.set_prop(key, v, cx),
        })
    }

    /// Animation frame for bar paths (`margin`, `y_offset`, `blur_radius`, `notch_*`,
    /// `height`, then background paths).
    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match path {
            "margin" => self.set_margin(v.as_i32(), fx),
            "y_offset" => self.set_y_offset(v.as_i32(), fx),
            "blur_radius" => self.set_blur_radius(v.as_u32()),
            "notch_width" => set_u32(&mut self.notch_width, v.as_u32()),
            "notch_offset" => self.set_notch_offset(v.as_u32(), fx),
            "notch_display_height" => self.set_notch_display_height(v.as_u32(), fx),
            "height" => self.set_height(v.as_u32(), fx),
            _ => self.background.anim_set(path, v, fx),
        }
    }

    /// `bar_manager_serialize` (`bar.md` §9.1), including the final `}\n`. `items` are all
    /// item names in global order (`None` → `(null)`).
    pub fn to_json(&self, items: &[Option<&str>]) -> String {
        let mut out = String::new();
        let _ = write!(
            out,
            "{{\n\t\"position\": \"{}\",\n\t\"topmost\": \"{}\",\n\t\"sticky\": \"{}\",\n\
             \t\"hidden\": \"{}\",\n\t\"shadow\": \"{}\",\n\t\"font_smoothing\": \"{}\",\n\
             \t\"show_in_fullscreen\": \"{}\",\n\t\"blur_radius\": {},\n\t\"margin\": {},\n",
            if self.position == b'b' { "bottom" } else { "top" },
            value::format_bool(self.topmost),
            value::format_bool(self.sticky),
            value::format_bool(self.any_bar_hidden),
            value::format_bool(self.shadow),
            value::format_bool(self.font_smoothing),
            value::format_bool(self.show_in_fullscreen),
            self.blur_radius,
            self.margin,
        );
        self.background.write_json(&mut out, "\t", false);
        out.push_str(",\n\t\"items\": [\n");
        for (n, name) in items.iter().enumerate() {
            if n > 0 {
                out.push_str(",\n");
            }
            let _ = write!(out, "\t\t \"{}\"", value::json_opt(*name));
        }
        out.push_str("\n\t]\n}\n");
        out
    }
}

/// Per-display bar state (`struct bar`, `bar.md` §1).
#[derive(Debug, Clone, PartialEq)]
pub struct BarState {
    /// Display id (`did`).
    pub display: u32,
    /// Arrangement id (1-based); 0 = unknown (never drawn).
    pub adid: u32,
    /// Current space id of the display.
    pub dsid: u64,
    /// Mission-control index of `dsid`; 0 = unknown (never drawn).
    pub sid: u32,
    /// False on native fullscreen spaces unless `show_in_fullscreen` (recomputed only on
    /// space change, quirk).
    pub shown: bool,
    /// `--bar hidden` for this bar.
    pub hidden: bool,
    /// For `mouse.entered.global`/`mouse.exited.global`.
    pub mouse_over: bool,
    /// Bar window frame (screen), from `layout::bar_frame`.
    pub frame: Rect,
}

impl BarState {
    pub fn new(display: u32, adid: u32) -> Self {
        BarState {
            display,
            adid,
            dsid: 0,
            sid: 0,
            shown: true,
            hidden: false,
            mouse_over: false,
            frame: Rect::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn default_query() {
        let b = BarProps::default();
        let expected = "{\n\t\"position\": \"top\",\n\t\"topmost\": \"off\",\n\t\"sticky\": \"on\",\n\
\t\"hidden\": \"off\",\n\t\"shadow\": \"off\",\n\t\"font_smoothing\": \"off\",\n\
\t\"show_in_fullscreen\": \"off\",\n\t\"blur_radius\": 0,\n\t\"margin\": 0,\n\t\"drawing\": \"off\",\n\
\t\"color\": \"0x44000000\",\n\t\"border_color\": \"0xffff0000\",\n\t\"border_width\": 0,\n\
\t\"height\": 25,\n\t\"corner_radius\": 0,\n\t\"padding_left\": 20,\n\t\"padding_right\": 20,\n\
\t\"x_offset\": 0,\n\t\"y_offset\": 0,\n\t\"clip\": 0.000000,\n\t\"image\": {\n\
\t\t\"value\": \"(null)\",\n\t\t\"drawing\": \"off\",\n\t\t\"scale\": 1.000000\n\t},\n\
\t\"items\": [\n\n\t]\n}\n";
        assert_eq!(b.to_json(&[]), expected);
        assert!(b.to_json(&[Some("a"), Some("b")]).ends_with("\t\"items\": [\n\t\t \"a\",\n\t\t \"b\"\n\t]\n}\n"));
    }

    #[test]
    fn props() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut b = BarProps::default();
        assert!(b.set_prop("color", "0xff1e1e2e", &mut cx).unwrap());
        assert!(b.background.enabled);
        assert!(b.set_prop("height", "30", &mut cx).unwrap());
        assert!(cx.fx.bar_needs_resize);
        assert!(!b.set_prop("blur_radius", "20", &mut cx).unwrap());
        assert_eq!(b.blur_radius, 20);
        assert_eq!(b.set_prop("clip", "1", &mut cx).unwrap_err().to_string(), "[!] Bar: Invalid property 'clip'\n");
        assert!(b.set_prop("position", "bottom", &mut cx).unwrap());
        assert_eq!(b.position, b'b');
        assert!(!b.set_prop("position", "", &mut cx).unwrap());
        assert!(b.set_prop("topmost", "window", &mut cx).unwrap());
        assert_eq!(b.window_level, level::FLOATING);
        assert!(b.set_prop("display", "main,2", &mut cx).unwrap());
        assert_eq!(b.displays, 0b10);
        assert!(b.set_prop("display", "", &mut cx).unwrap());
        assert_eq!(b.displays, DISPLAY_MAIN);
        assert_eq!(parse_display_pattern("2,main").0, 0);
        assert_eq!(parse_display_pattern("0").1, vec!["0".to_string()]);
        assert!(!b.set_prop("hidden", "toggle", &mut cx).unwrap());
        assert_eq!(cx.requests.last(), Some(&PropRequest::BarHidden(HiddenRequest::All(true))));
        assert_eq!(
            b.set_prop("nope", "1", &mut cx).unwrap_err().to_string(),
            "[!] Background: Invalid property 'nope'\n"
        );
    }
}
