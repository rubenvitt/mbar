//! Drawable components and their property parsers (`docs/spec/components.md`).
//!
//! Every component has the same shape:
//!
//! * a struct with all fields and SketchyBar's defaults (`Default`/`new`);
//! * `set_prop(key, value, &mut PropCx) -> PropResult` covering every property and
//!   sub-domain, with C's dispatch order and exact error messages;
//! * `anim_set(path, AnimValue, &mut PropEffects) -> bool` for every animatable setter path
//!   (see `props.rs` for the path convention);
//! * `write_json(&self, out, indent)` producing the `--query` fragment byte-for-byte
//!   (tabs, `0x%x` colours, `%f` floats, no trailing newline; strings JSON-escaped per D13);
//! * inheritance helpers used by `--default` inheritance and `--clone`.
//!
//! Layout-owned fields (`bounds`, `rtl`, …) are written by `layout.rs` (WP-A) only. They use
//! SketchyBar's item-local drawing coordinates (origin bottom-left, y up) so the C formulas
//! can be transcribed directly; `layout.rs` converts to top-left scene coordinates.

pub mod alias;
pub mod app_menu;
pub mod background;
pub mod font;
pub mod graph;
pub mod image;
pub mod shadow;
pub mod slider;
pub mod text;

pub use alias::Alias;
pub use app_menu::AppMenu;
pub use background::Background;
pub use font::FontSpec;
pub use graph::Graph;
pub use image::{Image, ImageSource};
pub use shadow::Shadow;
pub use slider::Slider;
pub use text::Text;

use crate::color::Color;
use crate::props::{AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value;

/// Colour sub-domain `<owner>.<color>.<prop>` (`color.c:color_parse_sub_domain`,
/// `components.md` §1.2). Never runs the owner's plain `color=` side effects.
pub fn color_set_prop(color: &mut Color, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
    let c = color.hex;
    Ok(match key {
        "hex" => cx.animate(
            "hex",
            AnimValue::Color(c),
            AnimValue::Color(value::parse_int(v) as u32),
            |a, _| color.set_hex(a.as_u32()),
        ),
        "alpha" => cx.animate(
            "alpha",
            AnimValue::Float(color.a),
            AnimValue::Float(value::parse_float(v)),
            |a, _| color.set_alpha(a.as_f32()),
        ),
        "red" => cx.animate(
            "red",
            AnimValue::Float(color.r),
            AnimValue::Float(value::parse_float(v)),
            |a, _| color.set_red(a.as_f32()),
        ),
        "green" => cx.animate(
            "green",
            AnimValue::Float(color.g),
            AnimValue::Float(value::parse_float(v)),
            |a, _| color.set_green(a.as_f32()),
        ),
        "blue" => cx.animate(
            "blue",
            AnimValue::Float(color.b),
            AnimValue::Float(value::parse_float(v)),
            |a, _| color.set_blue(a.as_f32()),
        ),
        _ => {
            return Err(PropError::ColorInvalidProperty(
                value::display_key(key).to_string(),
            ))
        }
    })
}

/// Animation frame for a colour sub-domain path (`hex`, `alpha`, `red`, `green`, `blue`).
pub fn color_anim_set(color: &mut Color, key: &str, v: AnimValue, _fx: &mut PropEffects) -> bool {
    match key {
        "hex" => color.set_hex(v.as_u32()),
        "alpha" => color.set_alpha(v.as_f32()),
        "red" => color.set_red(v.as_f32()),
        "green" => color.set_green(v.as_f32()),
        "blue" => color.set_blue(v.as_f32()),
        _ => false,
    }
}

/// `"0x%x"` of a colour (lowercase, unpadded).
pub fn hex(c: u32) -> String {
    format!("0x{c:x}")
}

/// Maps an `align` byte to its query name (`text.c:text_serialize`, `popup_serialize`).
pub fn align_name(a: u8) -> &'static str {
    match a {
        b'l' => "left",
        b'r' => "right",
        b'c' => "center",
        b'b' => "bottom",
        b't' => "top",
        _ => "invalid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn color_sub_domain() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut c = Color::from_hex(0xffffffff);
        assert!(color_set_prop(&mut c, "alpha", "0.5", &mut cx).unwrap());
        assert_eq!(c.hex, 0x7fffffff);
        assert!(color_set_prop(&mut c, "hex", "0xff000000", &mut cx).unwrap());
        assert_eq!(c.hex, 0xff000000);
        let e = color_set_prop(&mut c, "foo", "1", &mut cx).unwrap_err();
        assert_eq!(e.to_string(), "[?] Color: Invalid property 'foo'\n");
        assert_eq!(hex(0), "0x0");
        assert_eq!(hex(0x44000000), "0x44000000");
    }
}
