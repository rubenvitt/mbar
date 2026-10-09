//! Window borders: the JankyBorders take-over (`docs/spec/borders.md`, design in
//! `docs/superpowers/specs/2026-10-09-borders-design.md`).
//!
//! The core owns the borders **configuration** (`--borders`, `--query borders`); the
//! platform owns the live set of tracked windows and draws the border windows. Every change
//! reaches the platform as one [`crate::platform::PlatformRequest::SetBorders`].

mod parse;

pub use parse::{apply_to_window, parse_arg, validate_args, ArgError};

use crate::value::{fmt_f, format_bool, json_escape, parse_bool};
use std::fmt::Write as _;

/// What `borders -v` prints (JankyBorders v1.9.0, the version the spec describes).
pub const BORDERS_VERSION: &str = "borders-v1.9.0";

/// Direction of a two-color gradient (`gradient(top_left=…,bottom_right=…)` or
/// `gradient(top_right=…,bottom_left=…)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientDirection {
    TopLeftToBottomRight,
    TopRightToBottomLeft,
}

/// A border or background color (spec BR-PARSE-05). Colors are `0xAARRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderColor {
    Solid(u32),
    Glow(u32),
    Gradient {
        direction: GradientDirection,
        color1: u32,
        color2: u32,
    },
}

/// `style=` (BR-PARSE-03): any style char other than `s`/`u` draws round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    Round,
    Square,
    Uniform,
}

/// `order=` (BR-PARSE-04): above or below the target window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderOrder {
    Above,
    Below,
}

/// One full set of border settings (`struct settings`, spec §2.2). Used for the global
/// settings and for every `apply-to` override.
#[derive(Debug, Clone, PartialEq)]
pub struct BorderSettings {
    pub active: BorderColor,
    pub inactive: BorderColor,
    pub background: BorderColor,
    /// Whether the background is drawn (BR-PARSE-07).
    pub show_background: bool,
    pub width: f32,
    pub style: BorderStyle,
    pub order: BorderOrder,
    pub hidpi: bool,
    /// `None` = auto: on when mbar is trusted for Accessibility (JankyBorders' default).
    pub ax_focus: Option<bool>,
    /// Exact, case-sensitive BSD process names; empty = disabled.
    pub blacklist: Vec<String>,
    pub whitelist: Vec<String>,
}

impl Default for BorderSettings {
    /// JankyBorders' defaults (`src/main.c:31-46`).
    fn default() -> Self {
        Self {
            active: BorderColor::Solid(0xffe1e3e4),
            inactive: BorderColor::Solid(0x0000_0000),
            background: BorderColor::Solid(0x0000_0000),
            show_background: false,
            width: 4.0,
            style: BorderStyle::Round,
            order: BorderOrder::Below,
            hidpi: false,
            ax_focus: None,
            blacklist: Vec::new(),
            whitelist: Vec::new(),
        }
    }
}

/// Update bits of one message (spec §2.3): what the platform must redraw or recreate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateMask(pub u8);

impl UpdateMask {
    pub const ACTIVE: u8 = 1 << 0;
    pub const INACTIVE: u8 = 1 << 1;
    pub const ALL: u8 = Self::ACTIVE | Self::INACTIVE;
    pub const RECREATE_ALL: u8 = 1 << 2;
    pub const SETTING: u8 = 1 << 3;

    pub fn contains(self, bits: u8) -> bool {
        self.0 & bits == bits
    }

    pub fn intersects(self, bits: u8) -> bool {
        self.0 & bits != 0
    }
}

impl std::ops::BitOr for UpdateMask {
    type Output = UpdateMask;
    fn bitor(self, rhs: UpdateMask) -> UpdateMask {
        UpdateMask(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for UpdateMask {
    fn bitor_assign(&mut self, rhs: UpdateMask) {
        self.0 |= rhs.0;
    }
}

/// The borders configuration held in the [`crate::Model`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BordersState {
    /// Whether borders are drawn (`drawing=`, extension). Off until configured.
    pub drawing: bool,
    /// A `--borders` message was applied since start / the last `--reload`.
    pub configured: bool,
    pub settings: BorderSettings,
    /// `apply-to=<wid>` overrides, in insertion order.
    pub overrides: Vec<(u32, BorderSettings)>,
}

/// What the platform receives with [`crate::platform::PlatformRequest::SetBorders`]: the
/// complete configuration plus what changed.
#[derive(Debug, Clone, PartialEq)]
pub struct BordersUpdate {
    pub drawing: bool,
    pub settings: BorderSettings,
    pub overrides: Vec<(u32, BorderSettings)>,
    pub mask: UpdateMask,
}

impl std::fmt::Display for BorderColor {
    /// The input syntax (BR-PARSE-05) with 8 hex digits: `0xAARRGGBB`, `glow(0x…)`,
    /// `gradient(top_left=0x…,bottom_right=0x…)` or `gradient(top_right=0x…,bottom_left=0x…)`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            BorderColor::Solid(c) => write!(f, "0x{c:08x}"),
            BorderColor::Glow(c) => write!(f, "glow(0x{c:08x})"),
            BorderColor::Gradient {
                direction: GradientDirection::TopLeftToBottomRight,
                color1,
                color2,
            } => write!(
                f,
                "gradient(top_left=0x{color1:08x},bottom_right=0x{color2:08x})"
            ),
            BorderColor::Gradient {
                direction: GradientDirection::TopRightToBottomLeft,
                color1,
                color2,
            } => write!(
                f,
                "gradient(top_right=0x{color1:08x},bottom_left=0x{color2:08x})"
            ),
        }
    }
}

impl BorderStyle {
    /// `--query borders` spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            BorderStyle::Round => "round",
            BorderStyle::Square => "square",
            BorderStyle::Uniform => "uniform",
        }
    }
}

impl BorderOrder {
    /// `--query borders` spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            BorderOrder::Above => "above",
            BorderOrder::Below => "below",
        }
    }
}

impl BorderSettings {
    /// The color of a border whose target window is focused / unfocused (spec §6.1, §7.4).
    pub fn color_for(&self, focused: bool) -> BorderColor {
        if focused {
            self.active
        } else {
            self.inactive
        }
    }

    /// `app_allowed` (spec §5.4, BR-WIN-03 step 3): with a non-empty whitelist the BSD
    /// process name must be in it, and with a non-empty blacklist it must not be in it.
    /// Exact, case-sensitive comparison (BQ6).
    pub fn admits(&self, process_name: &str) -> bool {
        let listed = |list: &[String]| list.iter().any(|n| n == process_name);
        if !self.whitelist.is_empty() && !listed(&self.whitelist) {
            return false;
        }
        self.blacklist.is_empty() || !listed(&self.blacklist)
    }

    /// The keys of one settings object in `--query borders` (no trailing newline).
    fn write_json(&self, out: &mut String, i: &str) {
        let list = |l: &[String]| {
            let names: Vec<String> = l
                .iter()
                .map(|n| format!("\"{}\"", json_escape(n)))
                .collect();
            format!("[{}]", names.join(", "))
        };
        let _ = write!(
            out,
            "{i}\"active_color\": \"{}\",\n\
             {i}\"inactive_color\": \"{}\",\n\
             {i}\"background_color\": \"{}\",\n\
             {i}\"width\": {},\n\
             {i}\"style\": \"{}\",\n\
             {i}\"order\": \"{}\",\n\
             {i}\"hidpi\": \"{}\",\n\
             {i}\"ax_focus\": \"{}\",\n\
             {i}\"blacklist\": {},\n\
             {i}\"whitelist\": {}",
            self.active,
            self.inactive,
            self.background,
            fmt_f(f64::from(self.width)),
            self.style.as_str(),
            self.order.as_str(),
            format_bool(self.hidpi),
            self.ax_focus.map_or("auto", format_bool),
            list(&self.blacklist),
            list(&self.whitelist),
        );
    }
}

impl BordersState {
    /// Applies one `--borders` message (`message_handler`, spec BR-IPC-07/08) and returns
    /// the update for the platform, or `None` when the configuration did not change.
    ///
    /// `pairs` are the message's `key=value` pairs; each is rebuilt as `key=value` and run
    /// through [`parse_arg`] (JankyBorders' grammar), left to right. Errors are appended to
    /// `rsp` in mbar's convention ([`ArgError::mbar_text`]); the valid arguments still
    /// apply.
    ///
    /// - The extension key `drawing=` takes an mbar boolean (`value::parse_bool`). The
    ///   first message after start or `--reload` without `drawing=` turns drawing on.
    /// - `apply-to=<wid>` with `wid > 0` (the last one of the message wins, as the C code
    ///   overwrites `settings.apply_to`) routes the other keys into the override of that
    ///   window, created from the current global settings when it does not exist yet. The
    ///   globals are untouched and `RECREATE_ALL` is dropped from the mask (BR-IPC-09:
    ///   `hidpi`/lists in an override recreate nothing). `apply-to=0` takes the global
    ///   path. The sticky `apply-to` of a primary started with `apply-to=N` (BQ10) does not
    ///   exist: every message starts from `apply_to = 0`.
    /// - A global message also applies its keys to every override (BR-IPC-08), without
    ///   repeating the error lines per override (BQ9, optional). A message whose mask has
    ///   `RECREATE_ALL` then drops every override (JankyBorders loses them on recreate,
    ///   BQ11).
    /// - When `drawing` changes, the mask gets `RECREATE_ALL` so the platform creates or
    ///   destroys every border; overrides are kept.
    pub fn apply(&mut self, pairs: &[(String, String)], rsp: &mut String) -> Option<BordersUpdate> {
        let before_drawing = self.drawing;
        let before_settings = self.settings.clone();
        let before_overrides = self.overrides.clone();

        let mut drawing_given = false;
        let mut args = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            if key == "drawing" {
                self.drawing = parse_bool(value, self.drawing);
                drawing_given = true;
            } else {
                args.push(format!("{key}={value}"));
            }
        }
        if !self.configured && !drawing_given {
            self.drawing = true;
        }
        self.configured = true;

        let target = args
            .iter()
            .rev()
            .find_map(|a| apply_to_window(a))
            .unwrap_or(0);
        let mut mask = UpdateMask::default();
        let mut parse_all = |settings: &mut BorderSettings, rsp: &mut String| {
            for arg in &args {
                match parse_arg(settings, arg) {
                    Ok(m) => mask |= m,
                    Err(e) => rsp.push_str(&e.mbar_text(arg)),
                }
            }
        };
        if target > 0 {
            let idx = match self.overrides.iter().position(|(wid, _)| *wid == target) {
                Some(idx) => idx,
                None => {
                    self.overrides.push((target, self.settings.clone()));
                    self.overrides.len() - 1
                }
            };
            parse_all(&mut self.overrides[idx].1, rsp);
            mask.0 &= !UpdateMask::RECREATE_ALL;
        } else {
            parse_all(&mut self.settings, rsp);
            for (_, settings) in &mut self.overrides {
                for arg in &args {
                    let _ = parse_arg(settings, arg);
                }
            }
            if mask.intersects(UpdateMask::RECREATE_ALL) {
                self.overrides.clear();
            }
        }
        if self.drawing != before_drawing {
            mask |= UpdateMask(UpdateMask::RECREATE_ALL);
        }

        let changed = self.drawing != before_drawing
            || self.settings != before_settings
            || self.overrides != before_overrides;
        changed.then(|| self.update(mask))
    }

    /// The full configuration as a platform update with `mask`.
    pub fn update(&self, mask: UpdateMask) -> BordersUpdate {
        BordersUpdate {
            drawing: self.drawing,
            settings: self.settings.clone(),
            overrides: self.overrides.clone(),
            mask,
        }
    }

    /// `--query borders` (extension): tab-indented JSON like the other queries, colors in
    /// their input syntax, floats as `%f`, ending with `}\n`. `overrides` lists
    /// `{ "window": <wid>, …the same keys }` in insertion order.
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\n");
        let _ = write!(out, "\t\"drawing\": \"{}\",\n", format_bool(self.drawing));
        self.settings.write_json(&mut out, "\t");
        if self.overrides.is_empty() {
            out.push_str(",\n\t\"overrides\": []\n}\n");
            return out;
        }
        out.push_str(",\n\t\"overrides\": [\n");
        for (n, (wid, settings)) in self.overrides.iter().enumerate() {
            if n > 0 {
                out.push_str(",\n");
            }
            let _ = write!(out, "\t\t{{\n\t\t\t\"window\": {wid},\n");
            settings.write_json(&mut out, "\t\t\t");
            out.push_str("\n\t\t}");
        }
        out.push_str("\n\t]\n}\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(args: &[&str]) -> Vec<(String, String)> {
        args.iter()
            .map(|a| {
                let (k, v) = a.split_once('=').unwrap();
                (k.to_string(), v.to_string())
            })
            .collect()
    }

    fn apply(state: &mut BordersState, args: &[&str]) -> (Option<BordersUpdate>, String) {
        let mut rsp = String::new();
        let update = state.apply(&pairs(args), &mut rsp);
        (update, rsp)
    }

    #[test]
    fn colors_print_in_input_syntax() {
        assert_eq!(BorderColor::Solid(0xff00).to_string(), "0x0000ff00");
        assert_eq!(
            BorderColor::Glow(0x80ffffff).to_string(),
            "glow(0x80ffffff)"
        );
        let tl = BorderColor::Gradient {
            direction: GradientDirection::TopLeftToBottomRight,
            color1: 1,
            color2: 0xff000002,
        };
        assert_eq!(
            tl.to_string(),
            "gradient(top_left=0x00000001,bottom_right=0xff000002)"
        );
        let tr = BorderColor::Gradient {
            direction: GradientDirection::TopRightToBottomLeft,
            color1: 3,
            color2: 4,
        };
        assert_eq!(
            tr.to_string(),
            "gradient(top_right=0x00000003,bottom_left=0x00000004)"
        );
        // The printed form parses back to the same color.
        for color in [tl, tr, BorderColor::Glow(7), BorderColor::Solid(9)] {
            let mut s = BorderSettings::default();
            parse_arg(&mut s, &format!("active_color={color}")).unwrap();
            assert_eq!(s.active, color);
        }
    }

    #[test]
    fn color_for_and_admits() {
        let mut s = BorderSettings {
            active: BorderColor::Solid(1),
            inactive: BorderColor::Glow(2),
            ..BorderSettings::default()
        };
        assert_eq!(s.color_for(true), BorderColor::Solid(1));
        assert_eq!(s.color_for(false), BorderColor::Glow(2));

        assert!(s.admits("Safari"));
        s.blacklist = vec!["Safari".into(), " kitty".into()];
        assert!(!s.admits("Safari"));
        assert!(s.admits("safari"), "case-sensitive");
        assert!(s.admits("kitty"), "entries are not trimmed");
        assert!(!s.admits(" kitty"));
        s.whitelist = vec!["Safari".into(), "Code".into()];
        assert!(
            !s.admits("Safari"),
            "blacklist still applies to listed apps"
        );
        assert!(s.admits("Code"));
        assert!(!s.admits("Finder"));
        assert!(!s.admits("Code Helper"), "exact match");
        s.blacklist.clear();
        assert!(s.admits("Safari"));
    }

    #[test]
    fn first_message_turns_drawing_on_and_reports_changes() {
        let mut st = BordersState::default();
        assert!(!st.drawing && !st.configured);
        let (u, rsp) = apply(&mut st, &["width=6.0"]);
        assert_eq!(rsp, "");
        let u = u.unwrap();
        assert!(u.drawing && st.drawing && st.configured);
        assert_eq!(u.settings.width, 6.0);
        assert_eq!(
            u.mask,
            UpdateMask(UpdateMask::ALL | UpdateMask::RECREATE_ALL)
        );
        // Same value again: nothing changed.
        assert_eq!(apply(&mut st, &["width=6"]).0, None);
        // Later messages without drawing= leave it alone.
        st.drawing = false;
        assert_eq!(apply(&mut st, &["width=6"]).0, None);
        assert!(!st.drawing);
        let (u, _) = apply(&mut st, &["drawing=on"]);
        let u = u.unwrap();
        assert!(u.drawing);
        assert_eq!(u.mask, UpdateMask(UpdateMask::RECREATE_ALL));
        let (u, _) = apply(&mut st, &["drawing=toggle", "active_color=0xff00ff00"]);
        let u = u.unwrap();
        assert!(!u.drawing);
        assert_eq!(
            u.mask,
            UpdateMask(UpdateMask::ACTIVE | UpdateMask::RECREATE_ALL)
        );
        assert_eq!(st.settings.active, BorderColor::Solid(0xff00ff00));
    }

    #[test]
    fn first_message_with_drawing_off_stays_off() {
        let mut st = BordersState::default();
        assert_eq!(apply(&mut st, &["drawing=off"]).0, None);
        assert!(st.configured && !st.drawing);
        // Settings still change (and are sent) while not drawing.
        let u = apply(&mut st, &["style=square"]).0.unwrap();
        assert!(!u.drawing);
        assert_eq!(u.settings.style, BorderStyle::Square);
    }

    #[test]
    fn errors_are_reported_and_valid_keys_apply() {
        let mut st = BordersState::default();
        let (u, rsp) = apply(
            &mut st,
            &["glow=1", "width=2", "active_color=red", "hidpi=yes"],
        );
        assert_eq!(
            rsp,
            "[!] Borders: Invalid argument 'glow=1'\n\
             [!] Borders: Invalid color argument color=red\n\
             [!] Borders: Invalid argument 'hidpi=yes'\n"
        );
        assert_eq!(u.unwrap().settings.width, 2.0);
        // Only invalid keys, already drawing: nothing changed.
        let (u, rsp) = apply(&mut st, &["foo=bar"]);
        assert_eq!(u, None);
        assert_eq!(rsp, "[!] Borders: Invalid argument 'foo=bar'\n");
    }

    #[test]
    fn apply_to_routes_into_an_override() {
        let mut st = BordersState::default();
        apply(&mut st, &["width=5", "active_color=0xff111111"]);
        let (u, rsp) = apply(
            &mut st,
            &["active_color=0xffff0000", "hidpi=on", "apply-to=42"],
        );
        assert_eq!(rsp, "");
        let u = u.unwrap();
        // Globals untouched; the override starts from them.
        assert_eq!(u.settings.active, BorderColor::Solid(0xff111111));
        assert!(!u.settings.hidpi);
        assert_eq!(u.overrides.len(), 1);
        let (wid, o) = &u.overrides[0];
        assert_eq!(*wid, 42);
        assert_eq!(o.active, BorderColor::Solid(0xffff0000));
        assert_eq!(o.width, 5.0);
        assert!(o.hidpi);
        // BR-IPC-09: no recreate from an override.
        assert_eq!(u.mask, UpdateMask(UpdateMask::ACTIVE | UpdateMask::SETTING));
        // A second message for the same window updates that override in place.
        apply(&mut st, &["apply-to=42", "width=9"]);
        assert_eq!(st.overrides.len(), 1);
        assert_eq!(st.overrides[0].1.width, 9.0);
        assert_eq!(st.overrides[0].1.active, BorderColor::Solid(0xffff0000));
        // Another window gets its own override, in insertion order.
        apply(&mut st, &["apply-to=7", "style=square"]);
        assert_eq!(
            st.overrides.iter().map(|(w, _)| *w).collect::<Vec<_>>(),
            vec![42, 7]
        );
        // Unchanged override: nothing to send.
        assert_eq!(apply(&mut st, &["apply-to=7", "style=s"]).0, None);
    }

    #[test]
    fn global_messages_reach_overrides_and_recreate_clears_them() {
        let mut st = BordersState::default();
        apply(&mut st, &["apply-to=3", "width=10", "style=uniform"]);
        // apply-to=0 (and later in the message) takes the global path.
        let u = apply(&mut st, &["apply-to=3", "width=2", "apply-to=0"])
            .0
            .unwrap();
        assert_eq!(u.settings.width, 2.0);
        // BR-IPC-08: the global keys also apply to the override; its own keys stay.
        assert_eq!(st.overrides[0].1.width, 2.0);
        assert_eq!(st.overrides[0].1.style, BorderStyle::Uniform);
        assert_eq!(st.settings.style, BorderStyle::Round);
        // RECREATE_ALL drops every override.
        let u = apply(&mut st, &["blacklist=Safari"]).0.unwrap();
        assert!(u.overrides.is_empty() && st.overrides.is_empty());
        assert!(u.mask.contains(UpdateMask::RECREATE_ALL));
        assert_eq!(u.settings.blacklist, vec!["Safari".to_string()]);
    }

    #[test]
    fn query_json_defaults_and_overrides() {
        let st = BordersState::default();
        assert_eq!(
            st.to_json(),
            "{\n\
             \t\"drawing\": \"off\",\n\
             \t\"active_color\": \"0xffe1e3e4\",\n\
             \t\"inactive_color\": \"0x00000000\",\n\
             \t\"background_color\": \"0x00000000\",\n\
             \t\"width\": 4.000000,\n\
             \t\"style\": \"round\",\n\
             \t\"order\": \"below\",\n\
             \t\"hidpi\": \"off\",\n\
             \t\"ax_focus\": \"auto\",\n\
             \t\"blacklist\": [],\n\
             \t\"whitelist\": [],\n\
             \t\"overrides\": []\n\
             }\n"
        );
        let mut st = BordersState::default();
        apply(
            &mut st,
            &[
                "blacklist=Safari,\"x\"",
                "ax_focus=off",
                "order=above",
                "background_color=glow(0x80ffffff)",
            ],
        );
        apply(&mut st, &["apply-to=12", "style=square", "ax_focus=on"]);
        let json = st.to_json();
        assert!(json.contains("\t\"blacklist\": [\"Safari\", \"\\\"x\\\"\"],\n"));
        assert!(json.contains("\t\"ax_focus\": \"off\",\n"));
        assert!(json.contains("\t\"background_color\": \"glow(0x80ffffff)\",\n"));
        assert!(json.ends_with(
            "\t\"overrides\": [\n\
             \t\t{\n\
             \t\t\t\"window\": 12,\n\
             \t\t\t\"active_color\": \"0xffe1e3e4\",\n\
             \t\t\t\"inactive_color\": \"0x00000000\",\n\
             \t\t\t\"background_color\": \"glow(0x80ffffff)\",\n\
             \t\t\t\"width\": 4.000000,\n\
             \t\t\t\"style\": \"square\",\n\
             \t\t\t\"order\": \"above\",\n\
             \t\t\t\"hidpi\": \"off\",\n\
             \t\t\t\"ax_focus\": \"on\",\n\
             \t\t\t\"blacklist\": [\"Safari\", \"\\\"x\\\"\"],\n\
             \t\t\t\"whitelist\": []\n\
             \t\t}\n\
             \t]\n\
             }\n"
        ));
    }
}
