//! `app_menu` item component (mbar extension, `docs/EXTENSIONS.md` "Item type `app_menu`").
//!
//! `--add app_menu <name> <position>` draws the front application's menu bar natively: the
//! app name (in `app_font`), then each top-level menu title (in `font`), separated by
//! `spacing`. Clicking a title opens the real menu (platform, Accessibility API). Titles
//! arrive through `Input::MenuTitles`; the event `menus_change` is registered on first use
//! (it is appended like a custom event so plain SketchyBar configs keep their event bits).
//!
//! Properties (`app_menu.<p>`):
//!
//! | property | default | |
//! |---|---|---|
//! | `font` | the item's label font | `Family:Style:Size` |
//! | `app_font` | `font` with style `Bold` | `Family:Style:Size` |
//! | `color` | `0xffffffff` | title colour (animatable, bytes) |
//! | `highlight_color` | `0x33ffffff` | background of the hovered / open title (animatable) |
//! | `spacing` | `14` | gap between titles (animatable, int) |
//! | `apple` | `off` | draw the Apple menu first |
//! | `max_titles` | `0` (all) | limit of titles |
//! | `corner_radius` | `5` | highlight corner radius (animatable, int) |

use super::{hex, FontSpec};
use crate::color::Color;
use crate::geometry::Rect;
use crate::props::{set_i32, set_u32, AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq)]
pub struct AppMenu {
    /// `None` = use the item's label font.
    pub font: Option<FontSpec>,
    /// `None` = `font` (resolved) with style `Bold`.
    pub app_font: Option<FontSpec>,
    pub color: Color,
    pub highlight_color: Color,
    pub spacing: i32,
    pub apple: bool,
    pub max_titles: u32,
    pub corner_radius: u32,
    // --- runtime state ---
    /// Front application name (drawn first).
    pub app_name: String,
    /// Top-level menu titles of the front application (index 0 = Apple menu).
    pub titles: Vec<String>,
    /// Title under the mouse / currently open (index into `titles`).
    pub hovered: Option<usize>,
    /// Layout output: one rect per drawn title (app name first), item-local coordinates.
    pub title_bounds: Vec<Rect>,
}

impl Default for AppMenu {
    fn default() -> Self {
        AppMenu {
            font: None,
            app_font: None,
            color: Color::from_hex(0xffffffff),
            highlight_color: Color::from_hex(0x33ffffff),
            spacing: 14,
            apple: false,
            max_titles: 0,
            corner_radius: 5,
            app_name: String::new(),
            titles: Vec::new(),
            hovered: None,
            title_bounds: Vec::new(),
        }
    }
}

impl AppMenu {
    /// The title font, falling back to `label_font`.
    pub fn title_font(&self, label_font: &FontSpec) -> FontSpec {
        let mut f = self.font.clone().unwrap_or_else(|| label_font.clone());
        f.features = None;
        f
    }

    /// The app-name font: `app_font`, else the title font with style `Bold`.
    pub fn app_name_font(&self, label_font: &FontSpec) -> FontSpec {
        self.app_font.clone().unwrap_or_else(|| {
            let mut f = self.title_font(label_font);
            f.style = "Bold".to_string();
            f
        })
    }

    /// Titles to draw: the Apple menu (index 0) only with `apple=on`, limited by
    /// `max_titles` (0 = all). Returns `(index into titles, title)`.
    pub fn visible_titles(&self) -> Vec<(usize, &str)> {
        let start = if self.apple { 0 } else { 1 };
        let iter = self
            .titles
            .iter()
            .enumerate()
            .skip(start)
            .map(|(i, t)| (i, t.as_str()));
        if self.max_titles == 0 {
            iter.collect()
        } else {
            iter.take(self.max_titles as usize).collect()
        }
    }

    fn font_with(base: Option<&FontSpec>, v: &str) -> FontSpec {
        let mut f = base.cloned().unwrap_or_default();
        f.set_from_string(v);
        f
    }

    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "font" => {
                let f = Self::font_with(self.font.as_ref(), v);
                let changed = self.font.as_ref() != Some(&f);
                self.font = Some(f);
                changed
            }
            "app_font" => {
                let f = Self::font_with(self.app_font.as_ref(), v);
                let changed = self.app_font.as_ref() != Some(&f);
                self.app_font = Some(f);
                changed
            }
            "color" => cx.animate(
                "color",
                AnimValue::Color(self.color.hex),
                AnimValue::Color(value::parse_int(v) as u32),
                |a, _| self.color.set_hex(a.as_u32()),
            ),
            "highlight_color" => cx.animate(
                "highlight_color",
                AnimValue::Color(self.highlight_color.hex),
                AnimValue::Color(value::parse_int(v) as u32),
                |a, _| self.highlight_color.set_hex(a.as_u32()),
            ),
            "spacing" => cx.animate(
                "spacing",
                AnimValue::Int(self.spacing),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_i32(&mut self.spacing, a.as_i32()),
            ),
            "corner_radius" => cx.animate(
                "corner_radius",
                AnimValue::Int(self.corner_radius as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_u32(&mut self.corner_radius, a.as_u32()),
            ),
            "apple" => {
                let on = value::parse_bool(v, self.apple);
                let changed = on != self.apple;
                self.apple = on;
                changed
            }
            "max_titles" => {
                let n = value::parse_u32(v);
                set_u32(&mut self.max_titles, n)
            }
            _ => {
                return Err(PropError::AppMenuInvalidProperty(
                    value::display_key(key).to_string(),
                ))
            }
        })
    }

    pub fn anim_set(&mut self, path: &str, v: AnimValue, _fx: &mut PropEffects) -> bool {
        match path {
            "color" => self.color.set_hex(v.as_u32()),
            "highlight_color" => self.highlight_color.set_hex(v.as_u32()),
            "spacing" => set_i32(&mut self.spacing, v.as_i32()),
            "corner_radius" => set_u32(&mut self.corner_radius, v.as_u32()),
            _ => false,
        }
    }

    /// Extension query block (appended as `"app_menu"` to the item JSON of `app_menu`
    /// items), same conventions as the SketchyBar fragments.
    pub fn write_json(&self, out: &mut String, i: &str, label_font: &FontSpec) {
        let _ = write!(
            out,
            "{i}\"font\": \"{}\",\n{i}\"app_font\": \"{}\",\n{i}\"color\": \"{}\",\n\
             {i}\"highlight_color\": \"{}\",\n{i}\"spacing\": {},\n{i}\"apple\": \"{}\",\n\
             {i}\"max_titles\": {},\n{i}\"corner_radius\": {},\n{i}\"app\": \"{}\",\n{i}\"titles\": [\n",
            self.title_font(label_font).query_string(),
            self.app_name_font(label_font).query_string(),
            hex(self.color.hex),
            hex(self.highlight_color.hex),
            self.spacing,
            value::format_bool(self.apple),
            self.max_titles,
            self.corner_radius,
            value::json_escape(&self.app_name),
        );
        for (n, t) in self.titles.iter().enumerate() {
            if n > 0 {
                out.push_str(",\n");
            }
            let _ = write!(out, "{i}\t\"{}\"", value::json_escape(t));
        }
        let _ = write!(out, "\n{i}]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_titles_and_fonts() {
        let m = AppMenu {
            titles: vec!["Apple".into(), "File".into(), "Edit".into(), "View".into()],
            max_titles: 2,
            ..AppMenu::default()
        };
        assert_eq!(m.visible_titles(), vec![(1, "File"), (2, "Edit")]);
        let label = FontSpec {
            style: "Regular".into(),
            ..FontSpec::default()
        };
        assert_eq!(m.title_font(&label).style, "Regular");
        assert_eq!(m.app_name_font(&label).style, "Bold");
    }
}
