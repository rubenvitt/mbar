//! Hard drop shadow (`shadow.c`, `docs/spec/components.md` §2).

use super::{color_anim_set, color_set_prop, hex};
use crate::color::Color;
use crate::geometry::Point;
use crate::props::{set_bool, AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value::{self, split_key, KeySplit};
use std::fmt::Write;

/// `struct shadow`. There is no blur: a shadow is the shape translated by `offset`, filled
/// with `color`.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    /// `drawing`; default off.
    pub enabled: bool,
    /// Degrees; default 30.
    pub angle: u32,
    /// Default 5.
    pub distance: u32,
    /// Default `0xff000000`.
    pub color: Color,
    /// Derived: `(distance·cos(angle°), −distance·sin(angle°))` in CG (y-up) coordinates;
    /// defaults to `(4.330127, -2.5)`.
    pub offset: Point,
}

impl Default for Shadow {
    fn default() -> Self {
        let mut s = Shadow {
            enabled: false,
            angle: 30,
            distance: 5,
            color: Color::from_hex(0xff000000),
            offset: Point::default(),
        };
        s.update_offset();
        s
    }
}

impl Shadow {
    fn update_offset(&mut self) {
        let rad = self.angle as f64 * (2.0 * std::f64::consts::PI / 360.0);
        let d = self.distance as f32 as f64;
        self.offset = Point::new((d * rad.cos()) as f32, (-d * rad.sin()) as f32);
    }

    pub fn set_enabled(&mut self, on: bool) -> bool {
        set_bool(&mut self.enabled, on)
    }

    pub fn set_angle(&mut self, angle: u32) -> bool {
        if self.angle == angle {
            return false;
        }
        self.angle = angle;
        self.update_offset();
        true
    }

    pub fn set_distance(&mut self, distance: u32) -> bool {
        if self.distance == distance {
            return false;
        }
        self.distance = distance;
        self.update_offset();
        true
    }

    /// `shadow_set_color`: also enables the shadow.
    pub fn set_color(&mut self, c: u32) -> bool {
        let changed = self.set_enabled(true);
        self.color.set_hex(c) || changed
    }

    /// `shadow_parse_sub_domain` (§2.2).
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "drawing" => {
                let on = value::parse_bool(v, self.enabled);
                self.set_enabled(on)
            }
            "distance" => cx.animate(
                "distance",
                AnimValue::Int(self.distance as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| self.set_distance(a.as_u32()),
            ),
            "angle" => cx.animate(
                "angle",
                AnimValue::Int(self.angle as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| self.set_angle(a.as_u32()),
            ),
            "color" => cx.animate(
                "color",
                AnimValue::Color(self.color.hex),
                AnimValue::Color(value::parse_int(v) as u32),
                |a, _| self.set_color(a.as_u32()),
            ),
            _ => match split_key(key) {
                KeySplit::Sub("color", rest) => {
                    return cx.scoped("color", |cx| color_set_prop(&mut self.color, rest, v, cx))
                }
                KeySplit::Sub(sub, _) => return Err(PropError::ShadowInvalidSubdomain(sub.to_string())),
                _ => return Err(PropError::ShadowInvalidProperty(value::display_key(key).to_string())),
            },
        })
    }

    /// Animation frame for `distance`, `angle`, `color`, `color.<p>`.
    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match path {
            "distance" => self.set_distance(v.as_u32()),
            "angle" => self.set_angle(v.as_u32()),
            "color" => self.set_color(v.as_u32()),
            _ => match split_key(path) {
                KeySplit::Sub("color", rest) => color_anim_set(&mut self.color, rest, v, fx),
                _ => false,
            },
        }
    }

    /// `shadow_serialize` at indent `i` (§2.3), no trailing newline.
    pub fn write_json(&self, out: &mut String, i: &str) {
        let _ = write!(
            out,
            "{i}\"drawing\": \"{}\",\n{i}\"color\": \"{}\",\n{i}\"angle\": {},\n{i}\"distance\": {}",
            value::format_bool(self.enabled),
            hex(self.color.hex),
            self.angle,
            self.distance
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn defaults_and_props() {
        let s = Shadow::default();
        assert!((s.offset.x - 4.330127).abs() < 1e-5);
        assert!((s.offset.y + 2.5).abs() < 1e-5);
        let mut out = String::new();
        s.write_json(&mut out, "\t");
        assert_eq!(
            out,
            "\t\"drawing\": \"off\",\n\t\"color\": \"0xff000000\",\n\t\"angle\": 30,\n\t\"distance\": 5"
        );

        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut s = Shadow::default();
        assert!(s.set_prop("color.alpha", "0.5", &mut cx).unwrap());
        assert!(!s.enabled, "color sub-domain does not enable");
        assert!(s.set_prop("color", "0xff00ff00", &mut cx).unwrap());
        assert!(s.enabled);
        assert!(s.set_prop("angle", "90", &mut cx).unwrap());
        assert!(s.offset.x.abs() < 1e-5 && (s.offset.y + 5.0).abs() < 1e-5);
        assert_eq!(
            s.set_prop("foo.bar", "1", &mut cx).unwrap_err().to_string(),
            "[!] Shadow: Invalid subdomain 'foo'\n"
        );
        assert_eq!(
            s.set_prop("foo", "1", &mut cx).unwrap_err().to_string(),
            "[!] Shadow: Invalid property 'foo'\n"
        );
    }
}
