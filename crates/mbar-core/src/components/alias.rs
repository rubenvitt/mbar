//! Menu-bar extra mirror (`alias.c`, `docs/spec/components.md` §9).
//!
//! Window enumeration and capture are platform work: the runtime asks for captures with
//! `PlatformRequest::CaptureAlias` and receives `Input::AliasImage`. Alias items serialize
//! no alias-specific block (§9.10).

use super::{color_anim_set, color_set_prop, Image};
use crate::color::Color;
use crate::geometry::Rect;
use crate::platform::ImageInfo;
use crate::props::{AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value::{self, split_key, KeySplit};

/// `struct alias`.
#[derive(Debug, Clone, PartialEq)]
pub struct Alias {
    /// Process (owner) name.
    pub owner: String,
    /// Window name; `None` matches only windows with an empty name.
    pub name: Option<String>,
    /// Recapture period in 1 s clock ticks; default 1; 0 = never (even when forced).
    pub update_frequency: u32,
    pub counter: u32,
    /// `alias.color` / `alias.color.*` was set: tint through the captured image's alpha.
    pub color_override: bool,
    /// Default `0xffff0000`.
    pub color: Color,
    /// The captured picture (`alias.scale`, `alias.shadow.*`).
    pub image: Image,
    /// Captured window id (0 = not found) and its screen frame.
    pub window_id: u32,
    pub window_frame: Rect,
}

impl Default for Alias {
    fn default() -> Self {
        Alias {
            owner: String::new(),
            name: None,
            update_frequency: 1,
            counter: 0,
            color_override: false,
            color: Color::from_hex(0xffff0000),
            image: Image::default(),
            window_id: 0,
            window_frame: Rect::ZERO,
        }
    }
}

impl Alias {
    /// `--add alias <spec>` (§9.1): `Owner,Name` splits at the first `,`; `Owner` → no name;
    /// `Owner,` (nothing after the comma) → owner is the whole spec incl. the comma (quirk).
    pub fn parse_spec(spec: &str) -> (String, Option<String>) {
        match spec.split_once(',') {
            Some((owner, name)) if !name.is_empty() => (owner.to_string(), Some(name.to_string())),
            _ => (spec.to_string(), None),
        }
    }

    pub fn setup(&mut self, spec: &str) {
        let (owner, name) = Alias::parse_spec(spec);
        self.owner = owner;
        self.name = name;
    }

    /// `alias_update(forced)` counter logic (§9.7): returns true when a capture is due
    /// (the runtime then requests it from the platform).
    pub fn tick(&mut self, forced: bool) -> bool {
        if self.update_frequency == 0 {
            return false;
        }
        self.counter += 1;
        if forced || self.counter >= self.update_frequency {
            self.counter = 0;
            return true;
        }
        false
    }

    /// Result of a capture (`alias_update_image` tail): `None` with `disabled == false`
    /// drops the window id and the picture; `Some` goes through `image_set_image` (the
    /// logical size is the window frame size). Returns "changed".
    pub fn apply_capture(
        &mut self,
        image: Option<ImageInfo>,
        window_id: u32,
        frame: Rect,
        disabled: bool,
        forced: bool,
    ) -> bool {
        match image {
            None => {
                if !disabled {
                    self.window_id = 0;
                    self.image.destroy();
                }
                false
            }
            Some(mut info) => {
                self.window_id = window_id;
                self.window_frame = frame;
                info.size = frame.size();
                self.image.set_image(Some(info), forced)
            }
        }
    }

    /// Content length: `image_ref ? image.bounds.w : 0` (§9.8).
    pub fn length(&self) -> u32 {
        if self.image.key.is_some() {
            self.image.bounds.width.max(0.0) as u32
        } else {
            0
        }
    }

    /// Content height: `image_ref ? image.bounds.h : 0`.
    pub fn height(&self) -> u32 {
        if self.image.key.is_some() {
            self.image.bounds.height.max(0.0) as u32
        } else {
            0
        }
    }

    /// `alias_parse_sub_domain` (§9.9): sub-domains are checked **before** leaves.
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        match split_key(key) {
            KeySplit::Sub("shadow", rest) => {
                return cx.scoped("shadow", |cx| self.image.shadow.set_prop(rest, v, cx))
            }
            KeySplit::Sub("color", rest) => {
                let changed = !self.color_override;
                self.color_override = true;
                let r = cx.scoped("color", |cx| color_set_prop(&mut self.color, rest, v, cx))?;
                return Ok(r || changed);
            }
            KeySplit::Sub(sub, _) => return Err(PropError::AliasInvalidSubdomain(sub.to_string())),
            _ => {}
        }
        match key {
            "color" => {
                self.color.set_hex(value::parse_u32(v));
                self.color_override = true;
                Ok(true)
            }
            "scale" => Ok(self.image.set_scale(value::parse_float(v))),
            "update_freq" => {
                self.update_frequency = value::parse_u32(v);
                Ok(false)
            }
            _ => Err(PropError::AliasInvalidProperty(
                value::display_key(key).to_string(),
            )),
        }
    }

    /// Animation paths: `shadow.<p>`, `color.<p>`.
    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match split_key(path) {
            KeySplit::Sub("shadow", rest) => self.image.shadow.anim_set(rest, v, fx),
            KeySplit::Sub("color", rest) => color_anim_set(&mut self.color, rest, v, fx),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn spec_and_props() {
        assert_eq!(
            Alias::parse_spec("Control Center,Battery"),
            ("Control Center".into(), Some("Battery".into()))
        );
        assert_eq!(Alias::parse_spec("A,b,c"), ("A".into(), Some("b,c".into())));
        assert_eq!(Alias::parse_spec("Owner"), ("Owner".into(), None));
        assert_eq!(Alias::parse_spec("Owner,"), ("Owner,".into(), None));
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut a = Alias::default();
        assert!(a.set_prop("color.alpha", "0.5", &mut cx).unwrap());
        assert!(a.color_override);
        assert!(!a.set_prop("update_freq", "0", &mut cx).unwrap());
        assert!(!a.tick(true));
        assert_eq!(
            a.set_prop("foo", "1", &mut cx).unwrap_err().to_string(),
            "[!] Alias: Invalid property 'foo' \n"
        );
    }
}
