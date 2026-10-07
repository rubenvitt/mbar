//! Item popups (`popup.c`, `docs/spec/item.md` §3.5/§6, `docs/spec/cli.md` §6.11.6).
//!
//! Data and properties only; anchoring and popup layout are WP-A (`layout.rs`), membership
//! changes (`popup_add_item`/`popup_remove_item`) are WP-C (`runtime.rs`).

use crate::components::{align_name, Background};
use crate::geometry::{Point, Rect};
use crate::item::ItemId;
use crate::props::{set_bool, set_i32, set_u32, AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value::{self, split_key, KeySplit};
use std::fmt::Write;

/// `struct popup` (one per item; the host is the owning item).
#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    /// Default off. Every change resets `adid` (re-anchoring); off closes the window.
    pub drawing: bool,
    /// Row instead of column.
    pub horizontal: bool,
    /// Raw first byte of `align=` (default `l`): `c` centred, `l` left, else right.
    pub align: u8,
    /// Cell height; default 30, recomputed from the host window while not overridden.
    pub cell_size: u32,
    /// Set permanently by `height=` (no way back to automatic).
    pub overrides_cell_size: bool,
    /// Added to `anchor.y` (screen coordinates, positive = down).
    pub y_offset: i32,
    pub blur_radius: u32,
    /// Default **on**: popup-menu window level; off: backstop + 1.
    pub topmost: bool,
    /// Drawing off; preset `color=0x44000000`, `border_color=0xffff0000`. Its shadow is never
    /// drawn.
    pub background: Background,
    /// Member items in popup order.
    pub items: Vec<ItemId>,
    /// Hover state for `mouse.entered.global`/`mouse.exited.global`.
    pub mouse_over: bool,
    /// Window z-order must be refreshed.
    pub needs_ordering: bool,
    // --- layout state (WP-A) ---
    /// Screen anchor (top-left of the popup window) incl. `y_offset`.
    pub anchor: Point,
    /// Display the popup is anchored on; 0 = not anchored.
    pub adid: u32,
    /// Popup window frame (screen), `None` when the window is closed.
    pub frame: Option<Rect>,
}

impl Default for Popup {
    fn default() -> Self {
        Popup {
            drawing: false,
            horizontal: false,
            align: b'l',
            cell_size: 30,
            overrides_cell_size: false,
            y_offset: 0,
            blur_radius: 0,
            topmost: true,
            background: Background::popup_default(),
            items: Vec::new(),
            mouse_over: false,
            needs_ordering: false,
            anchor: Point::default(),
            adid: 0,
            frame: None,
        }
    }
}

impl Popup {
    /// `popup_set_drawing`: changing it closes the window when turning off and always resets
    /// `adid` to 0.
    pub fn set_drawing(&mut self, on: bool) -> bool {
        if self.drawing == on {
            return false;
        }
        if !on {
            self.frame = None;
        }
        self.drawing = on;
        self.adid = 0;
        true
    }

    /// `popup_set_cell_size`.
    pub fn set_cell_size(&mut self, size: i32) -> bool {
        if self.cell_size == size as u32 && self.overrides_cell_size {
            return false;
        }
        self.overrides_cell_size = true;
        self.cell_size = size as u32;
        true
    }

    /// `popup_set_topmost`: a change requires window re-ordering.
    pub fn set_topmost(&mut self, on: bool) -> bool {
        if !set_bool(&mut self.topmost, on) {
            return false;
        }
        self.needs_ordering = true;
        true
    }

    /// `popup_parse_sub_domain` (`cli.md` §6.11.6).
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "y_offset" => cx.animate(
                "y_offset",
                AnimValue::Int(self.y_offset),
                AnimValue::Int(value::parse_int(v)),
                |a, _| set_i32(&mut self.y_offset, a.as_i32()),
            ),
            "drawing" => {
                let on = value::parse_bool(v, self.drawing);
                self.set_drawing(on)
            }
            "horizontal" => {
                self.horizontal = value::parse_bool(v, self.horizontal);
                true
            }
            "align" => {
                self.align = value::first_byte(v);
                true
            }
            "height" => cx.animate(
                "height",
                AnimValue::Int(self.cell_size as i32),
                AnimValue::Int(value::parse_int(v)),
                |a, _| self.set_cell_size(a.as_i32()),
            ),
            "blur_radius" => {
                cx.animate(
                    "blur_radius",
                    AnimValue::Int(self.blur_radius as i32),
                    AnimValue::Int(value::parse_int(v)),
                    |a, _| {
                        set_u32(&mut self.blur_radius, a.as_u32());
                        false
                    },
                );
                false
            }
            "topmost" => {
                let on = value::parse_bool(v, self.topmost);
                self.set_topmost(on)
            }
            _ => match split_key(key) {
                KeySplit::Sub("background", rest) => {
                    return cx.scoped("background", |cx| self.background.set_prop(rest, v, cx))
                }
                KeySplit::Sub(sub, _) => return Err(PropError::PopupInvalidSubdomain(sub.to_string())),
                _ => return Err(PropError::PopupInvalidProperty(value::display_key(key).to_string())),
            },
        })
    }

    /// Animation paths: `y_offset`, `height`, `blur_radius` (applies, returns false),
    /// `background.<p>`.
    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match path {
            "y_offset" => set_i32(&mut self.y_offset, v.as_i32()),
            "height" => self.set_cell_size(v.as_i32()),
            "blur_radius" => {
                set_u32(&mut self.blur_radius, v.as_u32());
                false
            }
            _ => match split_key(path) {
                KeySplit::Sub("background", rest) => self.background.anim_set(rest, v, fx),
                _ => false,
            },
        }
    }

    /// Copy for `--clone`/inheritance (`popup_clear_pointers`): items, window and host are
    /// cleared; scalars (incl. `drawing`) and the background (incl. its image path, shared in
    /// C) are copied.
    pub fn inherited(&self) -> Popup {
        let mut p = self.clone();
        p.items.clear();
        p.frame = None;
        p
    }

    /// `popup_serialize` at indent `i` (`item.md` §11 POPUP). `name_of` resolves member
    /// names (`(null)` for unnamed).
    pub fn write_json(&self, out: &mut String, i: &str, name_of: &dyn Fn(ItemId) -> Option<String>) {
        let _ = write!(
            out,
            "{i}\"drawing\": \"{}\",\n{i}\"horizontal\": \"{}\",\n{i}\"height\": {},\n\
             {i}\"blur_radius\": {},\n{i}\"y_offset\": {},\n{i}\"align\": \"{}\",\n{i}\"background\": {{\n",
            value::format_bool(self.drawing),
            value::format_bool(self.horizontal),
            if self.overrides_cell_size { self.cell_size as i32 } else { -1 },
            self.blur_radius,
            self.y_offset,
            align_name(self.align),
        );
        let deeper = format!("{i}\t");
        self.background.write_json(out, &deeper, true);
        let _ = write!(out, "\n{i}}},\n{i}\"items\": [\n");
        for (n, id) in self.items.iter().enumerate() {
            let _ = write!(out, "{i}\t \"{}\"", value::json_opt(name_of(*id).as_deref()));
            if n + 1 < self.items.len() {
                out.push_str(",\n");
            }
        }
        let _ = write!(out, "\n{i}]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn props_and_json() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut p = Popup::default();
        assert!(p.set_prop("drawing", "on", &mut cx).unwrap());
        assert!(p.set_prop("align", "center", &mut cx).unwrap());
        assert!(p.set_prop("horizontal", "off", &mut cx).unwrap(), "always refreshes");
        assert!(!p.set_prop("blur_radius", "5", &mut cx).unwrap());
        assert_eq!(p.blur_radius, 5);
        assert!(p.set_prop("height", "40", &mut cx).unwrap());
        assert!(p.overrides_cell_size);
        assert!(p.set_prop("background.color", "0xff000000", &mut cx).unwrap());
        assert_eq!(
            p.set_prop("x.y", "1", &mut cx).unwrap_err().to_string(),
            "[!] Popup: Invalid subdomain 'x'\n"
        );
        p.items = vec![ItemId(1), ItemId(2)];
        let mut out = String::new();
        p.write_json(&mut out, "\t\t", &|id| Some(format!("i{}", id.0)));
        assert!(out.starts_with("\t\t\"drawing\": \"on\",\n\t\t\"horizontal\": \"off\",\n\t\t\"height\": 40,\n"));
        assert!(out.contains("\t\t\"align\": \"center\",\n"));
        assert!(out.ends_with("\t\t\"items\": [\n\t\t\t \"i1\",\n\t\t\t \"i2\"\n\t\t]"));
    }
}
