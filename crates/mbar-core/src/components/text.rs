//! Text cells: `icon`, `label`, `slider.knob` (`text.c`, `docs/spec/components.md` §4).
//!
//! Line metrics are computed at set time through [`Resources::text_metrics`] because width
//! animations start from `text_get_length`. Font changes only set [`Text::font_changed`];
//! the relayout happens lazily in [`Text::ensure_layout`] (C: inside `text_get_length`),
//! which layout and the length-reading setters call first.

use super::{align_name, color_anim_set, color_set_prop, hex, Background, FontSpec, Shadow};
use crate::animation::Curve;
use crate::color::Color;
use crate::geometry::Rect;
use crate::platform::{Resources, TextKey};
use crate::props::{set_bool, set_i32, AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value::{self, split_key, KeySplit};
use std::fmt::Write;

/// `struct text`.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// Default `""`.
    pub string: String,
    /// Default on.
    pub drawing: bool,
    /// Default off; drawing uses `highlight_color` while on.
    pub highlight: bool,
    /// Default `0xffffffff`.
    pub color: Color,
    /// Default `0xff000000`.
    pub highlight_color: Color,
    pub padding_left: i32,
    pub padding_right: i32,
    /// Glyph (and glyph shadow) draw offset only; not the text background.
    pub y_offset: i32,
    pub font: FontSpec,
    /// `width=<n>` set (`has_const_width`).
    pub has_const_width: bool,
    /// Last constant width (printed by `--query` even when dynamic; quirk 22).
    pub custom_width: u32,
    /// Raw first byte of `align=` (`l`/`c`/`r` meaningful; default `l`).
    pub align: u8,
    /// Truncation in code points; 0 = unlimited.
    pub max_chars: u32,
    /// Marquee duration in 60 Hz frames; default 100.
    pub scroll_duration: u32,
    /// Marquee offset (animated as float path `scroll`), subtracted from the pen x.
    pub scroll: f32,
    pub shadow: Shadow,
    pub background: Background,

    // --- derived line state (text_prepare_line) ---
    /// Font description changed since the last line preparation (`font.font_changed`).
    pub font_changed: bool,
    /// Measured width (ink `+1.5` truncated, typographic `+0.5`, or truncated-prefix ink).
    pub width: u32,
    /// Ink box: `w`/`h` are `(u32)(ink + 1.5)`, `x`/`y` `(i32)(ink + 0.5)` after preparation;
    /// `x`/`y` are overwritten by layout (`text_calculate_bounds`, item-local CG coords).
    pub bounds: Rect,
    /// Typographic ascent / descent (font-wide, descent positive).
    pub ascent: f32,
    pub descent: f32,
    /// Rendered line handle; `None` until the first preparation (`text_init` never lays out).
    pub line: Option<TextKey>,
}

impl Default for Text {
    fn default() -> Self {
        Text {
            string: String::new(),
            drawing: true,
            highlight: false,
            color: Color::from_hex(0xffffffff),
            highlight_color: Color::from_hex(0xff000000),
            padding_left: 0,
            padding_right: 0,
            y_offset: 0,
            font: FontSpec::default(),
            has_const_width: false,
            custom_width: 0,
            align: b'l',
            max_chars: 0,
            scroll_duration: 100,
            scroll: 0.0,
            shadow: Shadow::default(),
            background: Background::default(),
            font_changed: false,
            width: 0,
            bounds: Rect::ZERO,
            ascent: 0.0,
            descent: 0.0,
            line: None,
        }
    }
}

/// The byte prefix holding the first `n` code points (`text_calculate_truncated_width`).
pub fn truncate_chars(s: &str, n: u32) -> &str {
    match s.char_indices().nth(n as usize) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

impl Text {
    /// `text_prepare_line` (§4.3).
    pub fn prepare_line(&mut self, res: &mut dyn Resources) {
        self.font_changed = false;
        let m = res.text_metrics(&self.font, &self.string);
        self.bounds = Rect::new(
            (m.ink.x as f64 + 0.5) as i32 as f32,
            (m.ink.y as f64 + 0.5) as i32 as f32,
            (m.ink.width as f64 + 1.5) as u32 as f32,
            (m.ink.height as f64 + 1.5) as u32 as f32,
        );
        self.width = if self.font.typographical_width {
            (m.typographic_width as f64 + 0.5) as u32
        } else {
            self.bounds.width as u32
        };
        self.ascent = m.ascent;
        self.descent = m.descent;
        self.line = Some(m.key);
        if self.max_chars > 0 {
            let prefix = truncate_chars(&self.string, self.max_chars).to_string();
            let t = res.text_metrics(&self.font, &prefix);
            self.width = (t.ink.width as f64 + 1.5) as u32;
        }
    }

    /// The lazy relayout of `text_get_length` (font changed since the last preparation).
    pub fn ensure_layout(&mut self, res: &mut dyn Resources) {
        if self.font_changed {
            self.prepare_line(res);
        }
    }

    /// `text_set_string(s, forced)`: no-op unless forced or different; else replace and
    /// prepare the line.
    pub fn set_string(&mut self, s: &str, forced: bool, res: &mut dyn Resources) -> bool {
        if !forced && self.string == s {
            return false;
        }
        self.string = s.to_string();
        self.prepare_line(res);
        true
    }

    /// `text_get_length(text, override)` (§4.4). Call [`ensure_layout`](Self::ensure_layout)
    /// first.
    pub fn length(&self, override_width: bool) -> u32 {
        if !self.drawing {
            return 0;
        }
        let len = (self.width as i32)
            .wrapping_add(self.padding_left)
            .wrapping_add(self.padding_right);
        if (!self.has_const_width || override_width)
            && self.background.enabled
            && self.background.image.enabled
        {
            let iw = self.background.image.reserved_size().width;
            if iw > len as f32 {
                return iw.max(0.0) as u32;
            }
        }
        if self.has_const_width && !override_width {
            return self.custom_width;
        }
        len.max(0) as u32
    }

    /// `text_get_height`: `drawing ? bounds.h : 0`.
    pub fn height(&self) -> u32 {
        if self.drawing {
            self.bounds.height as u32
        } else {
            0
        }
    }

    /// `text_set_width` (§4.5): `w < 0` → dynamic.
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

    pub fn set_color(&mut self, c: u32) -> bool {
        self.color.set_hex(c)
    }

    pub fn set_highlight_color(&mut self, c: u32) -> bool {
        self.highlight_color.set_hex(c)
    }

    /// `text_set_max_chars` (§4.7) incl. the "no relayout when raising" quirk (`strlen`
    /// counts bytes).
    pub fn set_max_chars(&mut self, n: u32, res: &mut dyn Resources) -> bool {
        if self.max_chars == n {
            return false;
        }
        self.max_chars = n;
        let longer = self.string.len() as u64 > n as u64;
        if longer {
            let s = self.string.clone();
            self.set_string(&s, true, res);
        }
        longer
    }

    fn mark_font(&mut self, changed: bool) -> bool {
        if changed {
            self.font_changed = true;
        }
        changed
    }

    /// `text_parse_sub_domain` (§4.2), checked in C's order.
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "color" => cx.animate(
                "color",
                AnimValue::Color(self.color.hex),
                AnimValue::Color(value::parse_int(v) as u32),
                |a, _| self.set_color(a.as_u32()),
            ),
            "highlight" => {
                let on = value::parse_bool(v, self.highlight);
                if cx.is_animating() {
                    if self.highlight && !on {
                        for f in cx.cancel("color") {
                            self.set_color(f.as_u32());
                        }
                        let target = self.color.hex;
                        let hc = self.highlight_color.hex;
                        self.set_color(hc);
                        cx.animate("color", AnimValue::Color(hc), AnimValue::Color(target), |a, _| {
                            self.set_color(a.as_u32())
                        });
                    } else if !self.highlight && on {
                        for f in cx.cancel("highlight_color") {
                            self.set_highlight_color(f.as_u32());
                        }
                        let target = self.highlight_color.hex;
                        let c = self.color.hex;
                        self.set_highlight_color(c);
                        cx.animate(
                            "highlight_color",
                            AnimValue::Color(c),
                            AnimValue::Color(target),
                            |a, _| self.set_highlight_color(a.as_u32()),
                        );
                    }
                }
                set_bool(&mut self.highlight, on)
            }
            "font" => {
                let changed = self.font.set_from_string(v);
                self.mark_font(changed)
            }
            "highlight_color" => cx.animate(
                "highlight_color",
                AnimValue::Color(self.highlight_color.hex),
                AnimValue::Color(value::parse_int(v) as u32),
                |a, _| self.set_highlight_color(a.as_u32()),
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
            "y_offset" => cx.animate(
                "y_offset",
                AnimValue::Int(self.y_offset),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_i32(&mut self.y_offset, a.as_i32()),
            ),
            "scroll_duration" => {
                let d = value::parse_int(v);
                if d >= 0 {
                    self.scroll_duration = d as u32;
                }
                false
            }
            "width" => {
                self.ensure_layout(cx.res);
                if v == "dynamic" {
                    let from = AnimValue::Int(self.custom_width as i32);
                    let to = AnimValue::Int(self.length(true) as i32);
                    let r = cx.animate("width", from, to, |a, _| self.set_width(a.as_i32()));
                    let cw = self.custom_width as i32;
                    cx.queue_chained("width", AnimValue::Int(cw), AnimValue::Int(-1), 0, Curve::Linear);
                    r
                } else {
                    let from = AnimValue::Int(self.length(false) as i32);
                    cx.animate("width", from, AnimValue::Int(value::parse_int(v)), |a, _| {
                        self.set_width(a.as_i32())
                    })
                }
            }
            "drawing" => {
                let on = value::parse_bool(v, self.drawing);
                set_bool(&mut self.drawing, on)
            }
            "align" => {
                let a = value::first_byte(v);
                let changed = self.align != a;
                self.align = a;
                changed
            }
            "string" => return Ok(self.set_string_prop(v, cx)),
            "max_chars" => self.set_max_chars(value::parse_int(v) as u32, cx.res),
            _ => match split_key(key) {
                KeySplit::Sub("background", rest) => {
                    return cx.scoped("background", |cx| self.background.set_prop(rest, v, cx))
                }
                KeySplit::Sub("shadow", rest) => {
                    return cx.scoped("shadow", |cx| self.shadow.set_prop(rest, v, cx))
                }
                KeySplit::Sub("font", rest) => {
                    let r = cx.scoped("font", |cx| self.font.set_prop(rest, v, cx))?;
                    return Ok(self.mark_font(r));
                }
                KeySplit::Sub("color", rest) => {
                    return cx.scoped("color", |cx| color_set_prop(&mut self.color, rest, v, cx))
                }
                KeySplit::Sub("highlight_color", rest) => {
                    return cx.scoped("highlight_color", |cx| {
                        color_set_prop(&mut self.highlight_color, rest, v, cx)
                    })
                }
                KeySplit::Sub(sub, _) => return Err(PropError::TextInvalidSubdomain(sub.to_string())),
                _ => return Err(PropError::TextInvalidProperty(value::display_key(key).to_string())),
            },
        })
    }

    /// `string=` incl. the animated width transition (§4.5, `events.md` §10.9).
    fn set_string_prop(&mut self, v: &str, cx: &mut PropCx) -> bool {
        self.ensure_layout(cx.res);
        let pre = self.length(false);
        let changed = self.set_string(v, false, cx.res);
        if changed && cx.is_animating() {
            let post = self.length(false);
            if post != pre {
                self.set_width(pre as i32);
                cx.animate("width", AnimValue::Int(pre as i32), AnimValue::Int(post as i32), |a, _| {
                    self.set_width(a.as_i32())
                });
                let cw = self.custom_width as i32;
                cx.queue_chained("width", AnimValue::Int(cw), AnimValue::Int(-1), 0, Curve::Linear);
            }
        }
        changed
    }

    /// Animation frame for every animatable text path (`color`, `highlight_color`,
    /// `padding_*`, `y_offset`, `width`, `scroll`, `font.size`, `color.*`,
    /// `highlight_color.*`, `background.*`, `shadow.*`).
    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match path {
            "color" => self.set_color(v.as_u32()),
            "highlight_color" => self.set_highlight_color(v.as_u32()),
            "padding_left" => set_i32(&mut self.padding_left, v.as_i32()),
            "padding_right" => set_i32(&mut self.padding_right, v.as_i32()),
            "y_offset" => set_i32(&mut self.y_offset, v.as_i32()),
            "width" => self.set_width(v.as_i32()),
            "scroll" => {
                // text_set_scroll
                let s = v.as_f32();
                if self.scroll == s {
                    false
                } else {
                    self.scroll = s;
                    true
                }
            }
            _ => match split_key(path) {
                KeySplit::Sub("font", "size") => {
                    let c = self.font.set_size(v.as_f32());
                    self.mark_font(c)
                }
                KeySplit::Sub("color", rest) => color_anim_set(&mut self.color, rest, v, fx),
                KeySplit::Sub("highlight_color", rest) => color_anim_set(&mut self.highlight_color, rest, v, fx),
                KeySplit::Sub("background", rest) => self.background.anim_set(rest, v, fx),
                KeySplit::Sub("shadow", rest) => self.shadow.anim_set(rest, v, fx),
                _ => false,
            },
        }
    }

    /// `text_copy` after the struct `memcpy` (§4.12): all scalars copied, `font.features`
    /// lost, background picture kept only if `keep_bg_picture` (`image_copy` for item icon /
    /// label backgrounds), path lost; the line is force-prepared.
    pub fn inherited(&self, keep_bg_picture: bool, res: &mut dyn Resources) -> Text {
        let mut t = self.clone();
        t.font.features = None;
        t.background = self.background.inherited(keep_bg_picture);
        t.prepare_line(res);
        t
    }

    /// `text_serialize` at indent `i` (§4.11), no trailing newline.
    pub fn write_json(&self, out: &mut String, i: &str) {
        let _ = write!(
            out,
            "{i}\"value\": \"{}\",\n\
             {i}\"drawing\": \"{}\",\n\
             {i}\"highlight\": \"{}\",\n\
             {i}\"color\": \"{}\",\n\
             {i}\"highlight_color\": \"{}\",\n\
             {i}\"padding_left\": {},\n\
             {i}\"padding_right\": {},\n\
             {i}\"y_offset\": {},\n\
             {i}\"font\": \"{}\",\n\
             {i}\"width\": {},\n\
             {i}\"scroll_duration\": {},\n\
             {i}\"align\": \"{}\",\n\
             {i}\"background\": {{\n",
            value::json_escape(&self.string),
            value::format_bool(self.drawing),
            value::format_bool(self.highlight),
            hex(self.color.hex),
            hex(self.highlight_color.hex),
            self.padding_left,
            self.padding_right,
            self.y_offset,
            self.font.query_string(),
            self.custom_width as i32,
            self.scroll_duration as i32,
            align_name(self.align),
        );
        let deeper = format!("{i}\t");
        self.background.write_json(out, &deeper, true);
        let _ = write!(out, "\n{i}}},\n{i}\"shadow\": {{\n");
        self.shadow.write_json(out, &deeper);
        let _ = write!(out, "\n{i}}}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;
    use crate::props::AnimSpec;

    #[test]
    fn lengths_and_layout() {
        let mut res = HeadlessResources::default();
        let mut t = Text::default();
        assert_eq!(t.width, 0, "text_init does not lay out");
        t.set_string("", true, &mut res);
        assert_eq!(t.width, 1, "empty ink box -> 1 (Q7)");
        assert_eq!(t.height(), 1);
        t.set_string("abcd", false, &mut res);
        // headless: 0.6 * 14 * 4 = 33.6 -> (u32)(33.6 + 1.5) = 35
        assert_eq!(t.width, 35);
        t.padding_left = 3;
        assert_eq!(t.length(false), 38);
        t.set_width(10);
        assert_eq!(t.length(false), 10);
        assert_eq!(t.length(true), 38);
        assert!(t.set_max_chars(2, &mut res));
        assert_eq!(t.width, (0.6f32 * 14.0 * 2.0 + 1.5) as u32);
        assert_eq!(truncate_chars("äbc", 1), "ä");
    }

    #[test]
    fn props() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut t = Text::default();
        {
            let mut cx = PropCx::new(&mut res, &mut an, "/h");
            assert!(t.set_prop("string", "hi", &mut cx).unwrap());
            assert!(!t.set_prop("string", "hi", &mut cx).unwrap());
            assert!(t.set_prop("font", "Menlo", &mut cx).unwrap());
            assert_eq!(t.font.size, 10.0);
            assert!(t.font_changed);
            assert!(t.set_prop("font.size", "12", &mut cx).unwrap());
            assert!(!t.set_prop("scroll_duration", "-5", &mut cx).unwrap());
            assert_eq!(t.scroll_duration, 100);
            assert!(t.set_prop("align", "center", &mut cx).unwrap());
            assert_eq!(t.align, b'c');
            assert_eq!(
                t.set_prop("foo.bar", "1", &mut cx).unwrap_err().to_string(),
                "[!] Text: Invalid subdomain 'foo' \n"
            );
            assert_eq!(
                t.set_prop("font.bogus", "1", &mut cx).unwrap_err().to_string(),
                "[!] Text: Invalid property 'bogus'\n"
            );
            assert!(t.set_prop("width", "dynamic", &mut cx).unwrap());
        }
        // width=dynamic: const now, chained -1 step queued.
        assert!(t.has_const_width);
        assert_eq!(an.len(), 1);
        assert_eq!(an.animations()[0].to, AnimValue::Int(-1));

        // Animated string change: width pinned, animation + chained -1 queued.
        let mut an = Animator::new();
        let mut t = Text::default();
        t.set_string("a", true, &mut res);
        {
            let mut cx = PropCx::new(&mut res, &mut an, "/h");
            cx.anim = Some(AnimSpec { curve: Curve::Linear, duration: 20 });
            assert!(t.set_prop("string", "abcdef", &mut cx).unwrap());
        }
        assert!(t.has_const_width);
        assert_eq!(an.len(), 2);
        assert!(an.animations()[1].waiting);

        // Highlight cross-fade.
        let mut an = Animator::new();
        let mut t = Text::default();
        {
            let mut cx = PropCx::new(&mut res, &mut an, "/h");
            cx.anim = Some(AnimSpec { curve: Curve::Linear, duration: 20 });
            assert!(t.set_prop("highlight", "on", &mut cx).unwrap());
        }
        assert_eq!(t.highlight_color.hex, 0xffffffff);
        assert_eq!(an.animations()[0].path, "highlight_color");
        assert_eq!(an.animations()[0].to, AnimValue::Color(0xff000000));
    }

    #[test]
    fn json() {
        let mut out = String::new();
        Text::default().write_json(&mut out, "\t\t");
        assert!(out.starts_with("\t\t\"value\": \"\",\n\t\t\"drawing\": \"on\",\n"));
        assert!(out.contains("\t\t\"font\": \"Hack Nerd Font:Bold:14.00\",\n\t\t\"width\": 0,\n"));
        assert!(out.contains("\t\t\"align\": \"left\",\n\t\t\"background\": {\n\t\t\t\"drawing\": \"off\""));
        assert!(out.ends_with("\t\t\t\"distance\": 5\n\t\t}"));
    }
}
