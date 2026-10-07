//! Slider (`slider.c`, `docs/spec/components.md` §7).
//!
//! Animation paths: `percentage`, `highlight_color`, `width`, `background.<p>` (track),
//! `foreground.<p>` (fill; `slider.background.<p>` is applied to the fill first under this
//! path, then to the track), `knob.<p>`.

use super::{hex, Background, Text};
use crate::geometry::Point;
use crate::props::{set_u32, AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value::{self, split_key, KeySplit};
use std::fmt::Write;

/// `struct slider`.
#[derive(Debug, Clone, PartialEq)]
pub struct Slider {
    /// 0..=100.
    pub percentage: u32,
    /// `slider.highlight_color`, default `0xff0000ff`; also the fill colour.
    pub foreground_color: u32,
    /// Track (`slider->background`): colour `0xff000000` (enabled by `slider_init`), height 0.
    pub track: Background,
    /// Fill (`slider->foreground`): colour = `foreground_color` (enabled).
    pub fill: Background,
    /// Track width (C: `background.bounds.size.width`); 100 by init, overwritten by
    /// `--add slider <name> <pos> <width>` (missing → 0).
    pub width: u32,
    pub knob: Text,
    /// Set while the mouse drags the knob; `percentage`/`width` sets are ignored meanwhile.
    pub is_dragged: bool,
}

impl Default for Slider {
    fn default() -> Self {
        let mut fx = PropEffects::default();
        let mut track = Background::default();
        let mut fill = Background::default();
        track.set_color(0xff000000, &mut fx);
        fill.set_color(0xff0000ff, &mut fx);
        Slider {
            percentage: 0,
            foreground_color: 0xff0000ff,
            track,
            fill,
            width: 100,
            knob: Text::default(),
            is_dragged: false,
        }
    }
}

impl Slider {
    /// `slider_setup(width)`: track width and both backgrounds enabled.
    pub fn setup(&mut self, width: u32) {
        let mut fx = PropEffects::default();
        self.width = width;
        self.track.set_enabled(true, &mut fx);
        self.fill.set_enabled(true, &mut fx);
    }

    /// `slider_set_percentage`: compares the raw value, stores `min(p, 100)`.
    pub fn set_percentage(&mut self, p: u32) -> bool {
        if p == self.percentage {
            return false;
        }
        self.percentage = p.min(100);
        true
    }

    /// `slider_set_foreground_color`: also sets (and enables) the fill background colour.
    pub fn set_foreground_color(&mut self, c: u32, fx: &mut PropEffects) -> bool {
        if self.foreground_color == c {
            return false;
        }
        self.foreground_color = c;
        self.fill.set_color(c, fx)
    }

    /// `slider_get_percentage_for_point` (§7.6), `p` in the track's coordinate space (D2:
    /// layout provides a consistent space).
    pub fn percentage_for_point(&self, p: Point) -> u32 {
        let d = (p.x - self.track.bounds.x).max(0.0) as f64;
        let w = self.width as f64;
        // Rust float->int casts saturate like arm64 (D10): d/0 -> inf -> 100, 0/0 -> NaN -> 0.
        let pct = (d / w * 100.0 + 0.5) as u32;
        pct.min(100)
    }

    /// `slider_handle_drag`: sets `is_dragged` and the percentage; returns "changed".
    pub fn handle_drag(&mut self, p: Point) -> bool {
        self.is_dragged = true;
        let pct = self.percentage_for_point(p);
        self.set_percentage(pct)
    }

    /// `slider_parse_sub_domain` (§7.5).
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "percentage" => {
                if self.is_dragged {
                    false
                } else {
                    cx.animate(
                        "percentage",
                        AnimValue::Int(self.percentage as i32),
                        AnimValue::Int(value::parse_u32(v) as i32),
                        |a, _| self.set_percentage(a.as_u32()),
                    )
                }
            }
            "highlight_color" => cx.animate(
                "highlight_color",
                AnimValue::Color(self.foreground_color),
                AnimValue::Color(value::parse_u32(v)),
                |a, fx| self.set_foreground_color(a.as_u32(), fx),
            ),
            "width" => {
                if self.is_dragged {
                    false
                } else {
                    cx.animate(
                        "width",
                        AnimValue::Int(self.width as i32),
                        AnimValue::Int(value::parse_u32(v) as i32),
                        |a, _| set_u32(&mut self.width, a.as_u32()),
                    )
                }
            }
            "knob" => return cx.scoped("knob", |cx| self.knob.set_prop("string", v, cx)),
            _ => match split_key(key) {
                KeySplit::Sub("background", rest) => {
                    // Fill first (result ignored), restore the fill colour, then the track.
                    let _ = cx.scoped("foreground", |cx| self.fill.set_prop(rest, v, cx));
                    let fg = self.foreground_color;
                    self.fill.set_color(fg, &mut cx.fx);
                    return cx.scoped("background", |cx| self.track.set_prop(rest, v, cx));
                }
                KeySplit::Sub("knob", rest) => {
                    return cx.scoped("knob", |cx| self.knob.set_prop(rest, v, cx))
                }
                KeySplit::Sub(sub, _) => return Err(PropError::SliderInvalidSubdomain(sub.to_string())),
                _ => return Err(PropError::SliderInvalidProperty(value::display_key(key).to_string())),
            },
        })
    }

    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match path {
            "percentage" => self.set_percentage(v.as_u32()),
            "highlight_color" => self.set_foreground_color(v.as_u32(), fx),
            "width" => set_u32(&mut self.width, v.as_u32()),
            _ => match split_key(path) {
                KeySplit::Sub("background", rest) => self.track.anim_set(rest, v, fx),
                KeySplit::Sub("foreground", rest) => self.fill.anim_set(rest, v, fx),
                KeySplit::Sub("knob", rest) => self.knob.anim_set(rest, v, fx),
                _ => false,
            },
        }
    }

    /// Copy for inheritance/clone: track/fill pictures and the knob background picture are
    /// dropped (`slider_clear_pointers`), knob relaid out (`text_copy`).
    pub fn inherited(&self, res: &mut dyn crate::platform::Resources) -> Slider {
        let mut s = self.clone();
        s.track = self.track.inherited(false);
        s.fill = self.fill.inherited(false);
        s.knob = self.knob.inherited(false, res);
        s
    }

    /// `slider_serialize` at indent `s` (§7.8).
    pub fn write_json(&self, out: &mut String, s: &str) {
        let _ = write!(
            out,
            "{s}\"highlight_color\": \"{}\",\n{s}\"percentage\": \"{}\",\n{s}\"width\": \"{}\",\n{s}\"background\": {{\n",
            hex(self.foreground_color),
            self.percentage as i32,
            self.width as i32
        );
        let deeper = format!("{s}\t");
        self.track.write_json(out, &deeper, false);
        let _ = write!(out, "\n{s}}},\n{s}\"knob\": {{\n");
        self.knob.write_json(out, &deeper);
        let _ = write!(out, "\n{s}}}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn props() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut s = Slider::default();
        assert!(s.track.enabled && s.fill.enabled);
        assert!(s.set_prop("percentage", "150", &mut cx).unwrap());
        assert_eq!(s.percentage, 100);
        assert!(s.set_prop("percentage", "150", &mut cx).unwrap(), "raw compare quirk");
        assert!(s.set_prop("background.color", "0xff00ff00", &mut cx).unwrap());
        assert_eq!(s.track.color.hex, 0xff00ff00);
        assert_eq!(s.fill.color.hex, 0xff0000ff);
        assert!(s.set_prop("background.height", "5", &mut cx).unwrap());
        assert_eq!((s.track.height, s.fill.height), (5, 5));
        assert!(s.set_prop("knob", "o", &mut cx).unwrap());
        assert_eq!(s.knob.string, "o");
        s.is_dragged = true;
        assert!(!s.set_prop("width", "10", &mut cx).unwrap());
        assert_eq!(s.width, 100);
        assert_eq!(
            s.set_prop("x.y", "1", &mut cx).unwrap_err().to_string(),
            "[!] Slider: Invalid subdomain 'x' \n"
        );
        s.track.bounds.x = 10.0;
        assert_eq!(s.percentage_for_point(Point::new(60.0, 0.0)), 50);
        assert_eq!(s.percentage_for_point(Point::new(0.0, 0.0)), 0);
        assert_eq!(s.percentage_for_point(Point::new(500.0, 0.0)), 100);
    }
}
