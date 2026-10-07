//! Rounded-rect backgrounds (`background.c`, `docs/spec/components.md` §5).
//!
//! Used by items (where the item's `padding_left/right` live), icon/label/knob texts,
//! slider track and fill, brackets, popups and the bar itself.

use super::{color_anim_set, color_set_prop, hex, Image, Shadow};
use crate::color::Color;
use crate::geometry::Rect;
use crate::props::{set_i32, set_u32, AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value::{self, split_key, KeySplit};
use std::fmt::Write;

/// `struct background`.
#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    /// `drawing`; default off. `color=` and `clip>0` turn it on.
    pub enabled: bool,
    /// Default `0x00000000`.
    pub color: Color,
    /// Default `0x00000000`.
    pub border_color: Color,
    pub border_width: u32,
    /// C `bounds.size.height`: the `height=` value while `overrides_height`, otherwise the
    /// height computed by the last layout (layout writes it back, like C does; the setter's
    /// change detection compares against it).
    pub height: u32,
    /// `height != 0` was set explicitly.
    pub overrides_height: bool,
    pub corner_radius: u32,
    /// Only read by item/bar/popup layout; stored but without effect on text, knob and
    /// slider backgrounds.
    pub padding_left: i32,
    pub padding_right: i32,
    pub x_offset: i32,
    pub y_offset: i32,
    /// Opacity of the hole punched into the bar background (not clamped).
    pub clip: f32,
    pub shadow: Shadow,
    pub image: Image,
    /// Layout output (`background_calculate_bounds`), item-local drawing coordinates.
    pub bounds: Rect,
}

impl Default for Background {
    fn default() -> Self {
        Background {
            enabled: false,
            color: Color::from_hex(0),
            border_color: Color::from_hex(0),
            border_width: 0,
            height: 0,
            overrides_height: false,
            corner_radius: 0,
            padding_left: 0,
            padding_right: 0,
            x_offset: 0,
            y_offset: 0,
            clip: 0.0,
            shadow: Shadow::default(),
            image: Image::default(),
            bounds: Rect::ZERO,
        }
    }
}

impl Background {
    /// Popup background preset (`popup_init`): colours set directly, drawing stays off.
    pub fn popup_default() -> Self {
        let mut b = Background::default();
        b.border_color.set_hex(0xffff0000);
        b.color.set_hex(0x44000000);
        b
    }

    /// True while this background punches a hole into the bar (`background_clips_bar`).
    pub fn clips_bar(&self) -> bool {
        self.enabled && self.clip > 0.0
    }

    /// `background_set_enabled`: disabling/enabling a clipping background forces a bar
    /// redraw (its clip snapshots are reset).
    pub fn set_enabled(&mut self, on: bool, fx: &mut PropEffects) -> bool {
        if self.enabled == on {
            return false;
        }
        if self.clips_bar() {
            fx.bar_needs_update = true;
        }
        self.enabled = on;
        true
    }

    /// `background_set_color`: also enables the background (even for alpha 0).
    pub fn set_color(&mut self, c: u32, fx: &mut PropEffects) -> bool {
        let changed = self.set_enabled(true, fx);
        self.color.set_hex(c) || changed
    }

    pub fn set_border_color(&mut self, c: u32) -> bool {
        self.border_color.set_hex(c)
    }

    /// `background_set_height`.
    pub fn set_height(&mut self, h: u32) -> bool {
        if self.height == h {
            return false;
        }
        self.height = h;
        self.overrides_height = h != 0;
        true
    }

    /// `background_set_clip`.
    pub fn set_clip(&mut self, clip: f32, fx: &mut PropEffects) -> bool {
        if self.clip == clip {
            return false;
        }
        self.clip = clip;
        fx.bar_needs_update = true;
        fx.might_need_clipping = true;
        if clip > 0.0 {
            self.set_enabled(true, fx);
        }
        true
    }

    /// `background_parse_sub_domain` (§5.2), checked in C's order.
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "drawing" => {
                let on = value::parse_bool(v, self.enabled);
                self.set_enabled(on, &mut cx.fx)
            }
            "clip" => cx.animate(
                "clip",
                AnimValue::Float(self.clip),
                AnimValue::Float(value::parse_float(v)),
                |a, fx| self.set_clip(a.as_f32(), fx),
            ),
            "height" => cx.animate(
                "height",
                AnimValue::Int(self.height as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| self.set_height(a.as_u32()),
            ),
            "corner_radius" => cx.animate(
                "corner_radius",
                AnimValue::Int(self.corner_radius as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_u32(&mut self.corner_radius, a.as_u32()),
            ),
            "border_width" => cx.animate(
                "border_width",
                AnimValue::Int(self.border_width as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_u32(&mut self.border_width, a.as_u32()),
            ),
            "color" => cx.animate(
                "color",
                AnimValue::Color(self.color.hex),
                AnimValue::Color(value::parse_int(v) as u32),
                |a, fx| self.set_color(a.as_u32(), fx),
            ),
            "border_color" => cx.animate(
                "border_color",
                AnimValue::Color(self.border_color.hex),
                AnimValue::Color(value::parse_int(v) as u32),
                |a, _| self.set_border_color(a.as_u32()),
            ),
            "padding_left" => cx.animate(
                "padding_left",
                AnimValue::Int(self.padding_left),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_i32(&mut self.padding_left, a.as_i32()),
            ),
            "padding_right" => cx.animate(
                "padding_right",
                AnimValue::Int(self.padding_right),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_i32(&mut self.padding_right, a.as_i32()),
            ),
            "x_offset" => cx.animate(
                "x_offset",
                AnimValue::Int(self.x_offset),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_i32(&mut self.x_offset, a.as_i32()),
            ),
            "y_offset" => cx.animate(
                "y_offset",
                AnimValue::Int(self.y_offset),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_i32(&mut self.y_offset, a.as_i32()),
            ),
            "image" => return self.image.load(v, cx),
            _ => match split_key(key) {
                KeySplit::Sub("shadow", rest) => {
                    return cx.scoped("shadow", |cx| self.shadow.set_prop(rest, v, cx))
                }
                KeySplit::Sub("image", rest) => {
                    return cx.scoped("image", |cx| self.image.set_prop(rest, v, cx))
                }
                KeySplit::Sub("color", rest) => {
                    return cx.scoped("color", |cx| color_set_prop(&mut self.color, rest, v, cx))
                }
                KeySplit::Sub("border_color", rest) => {
                    return cx.scoped("border_color", |cx| color_set_prop(&mut self.border_color, rest, v, cx))
                }
                KeySplit::Sub(sub, _) => {
                    return Err(PropError::BackgroundInvalidSubdomain(sub.to_string()))
                }
                _ => {
                    return Err(PropError::BackgroundInvalidProperty(
                        value::display_key(key).to_string(),
                    ))
                }
            },
        })
    }

    /// Animation frame for every animatable background path.
    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match path {
            "clip" => self.set_clip(v.as_f32(), fx),
            "height" => self.set_height(v.as_u32()),
            "corner_radius" => set_u32(&mut self.corner_radius, v.as_u32()),
            "border_width" => set_u32(&mut self.border_width, v.as_u32()),
            "color" => self.set_color(v.as_u32(), fx),
            "border_color" => self.set_border_color(v.as_u32()),
            "padding_left" => set_i32(&mut self.padding_left, v.as_i32()),
            "padding_right" => set_i32(&mut self.padding_right, v.as_i32()),
            "x_offset" => set_i32(&mut self.x_offset, v.as_i32()),
            "y_offset" => set_i32(&mut self.y_offset, v.as_i32()),
            _ => match split_key(path) {
                KeySplit::Sub("shadow", rest) => self.shadow.anim_set(rest, v, fx),
                KeySplit::Sub("image", rest) => self.image.anim_set(rest, v, fx),
                KeySplit::Sub("color", rest) => color_anim_set(&mut self.color, rest, v, fx),
                KeySplit::Sub("border_color", rest) => color_anim_set(&mut self.border_color, rest, v, fx),
                _ => false,
            },
        }
    }

    /// Copy for inheritance/clone (`background_clear_pointers` + optional `image_copy`).
    pub fn inherited(&self, keep_picture: bool) -> Background {
        let mut b = self.clone();
        b.image = self.image.inherited(keep_picture);
        b
    }

    /// `"height"` as printed by `background_serialize`: `overrides ? (int)h : 0` via `%u`.
    pub fn query_height(&self) -> u32 {
        if self.overrides_height {
            self.height.min(i32::MAX as u32)
        } else {
            0
        }
    }

    /// `background_serialize(bg, i, detailed)` (§5.7), no trailing newline.
    pub fn write_json(&self, out: &mut String, i: &str, detailed: bool) {
        let _ = write!(
            out,
            "{i}\"drawing\": \"{}\",\n\
             {i}\"color\": \"{}\",\n\
             {i}\"border_color\": \"{}\",\n\
             {i}\"border_width\": {},\n\
             {i}\"height\": {},\n\
             {i}\"corner_radius\": {},\n\
             {i}\"padding_left\": {},\n\
             {i}\"padding_right\": {},\n\
             {i}\"x_offset\": {},\n\
             {i}\"y_offset\": {},\n\
             {i}\"clip\": {},\n",
            value::format_bool(self.enabled),
            hex(self.color.hex),
            hex(self.border_color.hex),
            self.border_width,
            self.query_height(),
            self.corner_radius,
            self.padding_left,
            self.padding_right,
            self.x_offset,
            self.y_offset,
            value::fmt_f(self.clip as f64),
        );
        let deeper = format!("{i}\t");
        let _ = write!(out, "{i}\"image\": {{\n");
        self.image.write_json(out, &deeper);
        let _ = write!(out, "\n{i}}}");
        if detailed {
            let _ = write!(out, ",\n{i}\"shadow\": {{\n");
            self.shadow.write_json(out, &deeper);
            let _ = write!(out, "\n{i}}}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn color_enables_and_json() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut bg = Background::default();
        assert!(bg.set_prop("border_color", "0xffffffff", &mut cx).unwrap());
        assert!(!bg.enabled);
        assert!(bg.set_prop("color.alpha", "1", &mut cx).unwrap());
        assert!(!bg.enabled);
        assert!(bg.set_prop("color", "0x00000000", &mut cx).unwrap());
        assert!(bg.enabled);
        assert!(bg.set_prop("height", "20", &mut cx).unwrap());
        assert!(bg.overrides_height);
        assert!(bg.set_prop("clip", "0.5", &mut cx).unwrap());
        assert!(cx.fx.might_need_clipping);
        assert_eq!(
            bg.set_prop("nope", "1", &mut cx).unwrap_err().to_string(),
            "[!] Background: Invalid property 'nope'\n"
        );
        assert_eq!(
            bg.set_prop("nope.x", "1", &mut cx).unwrap_err().to_string(),
            "[!] Background: Invalid subdomain 'nope'\n"
        );
        let mut out = String::new();
        Background::default().write_json(&mut out, "\t\t\t", true);
        let expected = "\t\t\t\"drawing\": \"off\",\n\t\t\t\"color\": \"0x0\",\n\t\t\t\"border_color\": \"0x0\",\n\
\t\t\t\"border_width\": 0,\n\t\t\t\"height\": 0,\n\t\t\t\"corner_radius\": 0,\n\t\t\t\"padding_left\": 0,\n\
\t\t\t\"padding_right\": 0,\n\t\t\t\"x_offset\": 0,\n\t\t\t\"y_offset\": 0,\n\t\t\t\"clip\": 0.000000,\n\
\t\t\t\"image\": {\n\t\t\t\t\"value\": \"(null)\",\n\t\t\t\t\"drawing\": \"off\",\n\t\t\t\t\"scale\": 1.000000\n\t\t\t},\n\
\t\t\t\"shadow\": {\n\t\t\t\t\"drawing\": \"off\",\n\t\t\t\t\"color\": \"0xff000000\",\n\t\t\t\t\"angle\": 30,\n\
\t\t\t\t\"distance\": 5\n\t\t\t}";
        assert_eq!(out, expected);
    }
}
