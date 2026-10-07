//! Background images (`image.c`, `docs/spec/components.md` §6).
//!
//! The core parses the source and keeps the model; decoding/capturing is the platform's
//! job through [`Resources::load_image`](crate::platform::Resources::load_image), which
//! returns an [`ImageInfo`] (opaque key, logical size, content hash) or an [`ImageError`].

use super::{color_anim_set, color_set_prop, Shadow};
use crate::color::Color;
use crate::geometry::{Rect, Size};
use crate::platform::{ImageError, ImageInfo, ImageKey};
use crate::props::{set_f32, set_i32, set_u32, AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value::{self, split_key, KeySplit};
use std::fmt::Write;

/// What `image_load` decided to load (§6.2), before talking to the platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImageSource {
    /// `app.<bundle id or running app name>`: the app icon, logical 32×32 pt.
    App(String),
    /// `space.<n>`: one-shot capture of Mission Control space `atoi(n)` (1-based).
    /// `raw` is the text after `space.` (used in the error message).
    Space { raw: String, index: u32 },
    /// `media.artwork`: link to the now-playing artwork (no platform load).
    MediaArtwork,
    /// A file path after `~` expansion. The platform reports `NotFound` for missing paths
    /// and directories, `InvalidFormat` when the data provider fails, `DecodeFailed` when
    /// PNG/JPEG decoding fails (PNG iff the path ends in `.png`, case-sensitive).
    File(String),
    /// Empty resolved path: destroy the picture.
    Empty,
}

/// C `atoi`: leading whitespace, sign, decimal digits; garbage → 0.
fn atoi(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for c in digits.chars() {
        let Some(d) = c.to_digit(10) else { break };
        v = (v * 10 + d as i64).min(i64::from(i32::MAX) + 1);
    }
    (if neg { -v } else { v }) as i32
}

impl ImageSource {
    /// Dispatch of `image_load` (§6.2): the first `.`-segment decides `app`/`space`
    /// (quirk: a relative file `app.png` is looked up as application `png`), then
    /// `media.artwork`, then a file path (`~` → `$HOME`), then empty.
    pub fn parse(v: &str, home: &str) -> ImageSource {
        if let KeySplit::Sub(key, val) = split_key(v) {
            if key == "app" {
                return ImageSource::App(val.to_string());
            }
            if key == "space" {
                return ImageSource::Space {
                    raw: val.to_string(),
                    index: atoi(val) as u32,
                };
            }
        }
        if v == "media.artwork" {
            return ImageSource::MediaArtwork;
        }
        let res = value::resolve_path(v, home);
        if res.is_empty() {
            ImageSource::Empty
        } else {
            ImageSource::File(res)
        }
    }
}

/// `struct image`.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// `drawing`; default off; set by every successful load.
    pub enabled: bool,
    /// Raw value as given (before `~` expansion); `None` prints `(null)`.
    pub path: Option<String>,
    /// Default 1.0.
    pub scale: f32,
    /// Logical image size before `scale`.
    pub size: Size,
    /// `size * scale`; origin written by layout (`image_calculate_bounds`).
    pub bounds: Rect,
    pub corner_radius: u32,
    pub border_width: f32,
    /// Default `0xcccccccc`.
    pub border_color: Color,
    pub padding_left: i32,
    pub padding_right: i32,
    pub y_offset: i32,
    pub shadow: Shadow,
    /// Linked to the shared now-playing artwork (`media.artwork`).
    pub link: bool,
    /// Decoded picture (`image_ref`); `None` = nothing to draw.
    pub key: Option<ImageKey>,
    /// Content hash of the picture (`data_ref` byte comparison).
    pub hash: u64,
}

impl Default for Image {
    fn default() -> Self {
        Image {
            enabled: false,
            path: None,
            scale: 1.0,
            size: Size::default(),
            bounds: Rect::ZERO,
            corner_radius: 0,
            border_width: 0.0,
            border_color: Color::from_hex(0xcccccccc),
            padding_left: 0,
            padding_right: 0,
            y_offset: 0,
            shadow: Shadow::default(),
            link: false,
            key: None,
            hash: 0,
        }
    }
}

impl Image {
    pub fn set_enabled(&mut self, on: bool) -> bool {
        if self.enabled == on {
            return false;
        }
        self.enabled = on;
        true
    }

    /// `image_set_link`: linking enables the image.
    pub fn set_link(&mut self, link: bool) -> bool {
        if self.link == link {
            return false;
        }
        self.link = link;
        if link {
            self.enabled = true;
        }
        true
    }

    /// `image_set_image` (§6.3): `None` releases the picture (enabled untouched). A real
    /// picture breaks the media link, is skipped when unchanged (`!forced`, same content
    /// hash and size), else replaces the picture and enables the image.
    pub fn set_image(&mut self, info: Option<ImageInfo>, forced: bool) -> bool {
        let Some(info) = info else {
            self.key = None;
            self.hash = 0;
            return false;
        };
        if self.link {
            self.set_link(false);
        }
        if !forced && self.key.is_some() && self.hash == info.hash && self.size == info.size {
            return false;
        }
        self.size = info.size;
        self.bounds = Rect::new(0.0, 0.0, info.size.width * self.scale, info.size.height * self.scale);
        self.key = Some(info.key);
        self.hash = info.hash;
        self.enabled = true;
        true
    }

    /// `image_destroy` for `image=`: release the picture and the path. `enabled`, `size`,
    /// `bounds` and `link` are kept (quirk).
    pub fn destroy(&mut self) {
        self.key = None;
        self.hash = 0;
        self.path = None;
    }

    /// `image_load` (§6.2). The path is stored first, unconditionally.
    pub fn load(&mut self, v: &str, cx: &mut PropCx) -> PropResult {
        self.path = Some(v.to_string());
        let src = ImageSource::parse(v, cx.home);
        match &src {
            ImageSource::MediaArtwork => {
                cx.fx.begin_media_events = true;
                return Ok(self.set_link(true));
            }
            ImageSource::Empty => {
                self.destroy();
                return Ok(false);
            }
            _ => {}
        }
        match cx.res.load_image(&src) {
            Ok(info) => {
                self.set_image(Some(info), true);
                Ok(true)
            }
            Err(err) => match (&src, err) {
                (ImageSource::App(name), _) => Err(PropError::ImageInvalidAppName(name.clone())),
                (ImageSource::Space { raw, .. }, _) => Err(PropError::ImageInvalidSpaceId(raw.clone())),
                (ImageSource::File(_), ImageError::InvalidFormat) => {
                    let after_dot = match split_key(v) {
                        KeySplit::Sub(_, rest) => Some(rest.to_string()),
                        _ => None,
                    };
                    Err(PropError::ImageInvalidFormat(after_dot))
                }
                (ImageSource::File(path), ImageError::DecodeFailed) => {
                    cx.respond(format!("Could not open image file at: {path}\n"));
                    Ok(true)
                }
                (ImageSource::File(path), _) => Err(PropError::ImageFileNotFound(path.clone())),
                _ => Ok(false),
            },
        }
    }

    /// `image_set_scale`: keeps the origin, recomputes the size.
    pub fn set_scale(&mut self, scale: f32) -> bool {
        if !set_f32(&mut self.scale, scale) {
            return false;
        }
        self.bounds.width = self.size.width * self.scale;
        self.bounds.height = self.size.height * self.scale;
        true
    }

    pub fn set_border_color(&mut self, c: u32) -> bool {
        self.border_color.set_hex(c)
    }

    /// `image_get_size` (§6.4): the space an image reserves in its owner.
    pub fn reserved_size(&self) -> Size {
        let sx = if self.shadow.enabled { self.shadow.offset.x } else { 0.0 };
        Size::new(
            self.bounds.width + self.padding_left as f32 + self.padding_right as f32 + sx,
            self.bounds.height + 2.0 * (self.y_offset as f32).abs(),
        )
    }

    /// `image_parse_sub_domain` (§6.3).
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "string" => return self.load(v, cx),
            "drawing" => {
                let on = value::parse_bool(v, self.enabled);
                self.set_enabled(on)
            }
            "scale" => cx.animate(
                "scale",
                AnimValue::Float(self.scale),
                AnimValue::Float(value::parse_float(v)),
                |a, _| self.set_scale(a.as_f32()),
            ),
            "corner_radius" => cx.animate(
                "corner_radius",
                AnimValue::Int(self.corner_radius as i32),
                AnimValue::Int(value::parse_u32(v) as i32),
                |a, _| set_u32(&mut self.corner_radius, a.as_u32()),
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
            "border_width" => cx.animate(
                "border_width",
                AnimValue::Float(self.border_width),
                AnimValue::Float(value::parse_float(v)),
                |a, _| set_f32(&mut self.border_width, a.as_f32()),
            ),
            "border_color" => cx.animate(
                "border_color",
                AnimValue::Color(self.border_color.hex),
                AnimValue::Color(value::parse_int(v) as u32),
                |a, _| self.set_border_color(a.as_u32()),
            ),
            _ => match split_key(key) {
                KeySplit::Sub("border_color", rest) => {
                    return cx.scoped("border_color", |cx| color_set_prop(&mut self.border_color, rest, v, cx))
                }
                KeySplit::Sub("shadow", rest) => {
                    return cx.scoped("shadow", |cx| self.shadow.set_prop(rest, v, cx))
                }
                KeySplit::Sub(sub, _) => return Err(PropError::ImageInvalidSubdomain(sub.to_string())),
                _ => return Err(PropError::ImageUnknownProperty(value::display_key(key).to_string())),
            },
        })
    }

    /// Animation frame for the animatable image paths.
    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match path {
            "scale" => self.set_scale(v.as_f32()),
            "corner_radius" => set_u32(&mut self.corner_radius, v.as_u32()),
            "padding_left" => set_i32(&mut self.padding_left, v.as_i32()),
            "padding_right" => set_i32(&mut self.padding_right, v.as_i32()),
            "y_offset" => set_i32(&mut self.y_offset, v.as_i32()),
            "border_width" => set_f32(&mut self.border_width, v.as_f32()),
            "border_color" => self.set_border_color(v.as_u32()),
            _ => match split_key(path) {
                KeySplit::Sub("border_color", rest) => color_anim_set(&mut self.border_color, rest, v, fx),
                KeySplit::Sub("shadow", rest) => self.shadow.anim_set(rest, v, fx),
                _ => false,
            },
        }
    }

    /// Copy used by `--default` inheritance / `--clone` (`text_copy`/`image_copy`, §4.12):
    /// all scalars are copied, the path is **lost** (`(null)`), the picture is kept only
    /// when `keep_picture` (item/icon/label backgrounds get `CGImageCreateCopy`; knob,
    /// slider and popup backgrounds do not).
    pub fn inherited(&self, keep_picture: bool) -> Image {
        let mut img = self.clone();
        img.path = None;
        if !keep_picture {
            img.key = None;
            img.hash = 0;
        }
        img
    }

    /// `image_serialize` at indent `i` (§6.6).
    pub fn write_json(&self, out: &mut String, i: &str) {
        let _ = write!(
            out,
            "{i}\"value\": \"{}\",\n{i}\"drawing\": \"{}\",\n{i}\"scale\": {}",
            value::json_opt(self.path.as_deref()),
            value::format_bool(self.enabled),
            value::fmt_f(self.scale as f64)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn source_parsing() {
        assert_eq!(ImageSource::parse("app.Safari", "/h"), ImageSource::App("Safari".into()));
        assert_eq!(ImageSource::parse("app.png", "/h"), ImageSource::App("png".into()));
        assert_eq!(
            ImageSource::parse("space.2", "/h"),
            ImageSource::Space { raw: "2".into(), index: 2 }
        );
        assert_eq!(ImageSource::parse("media.artwork", "/h"), ImageSource::MediaArtwork);
        assert_eq!(ImageSource::parse("~/a.png", "/h"), ImageSource::File("/h/a.png".into()));
        assert_eq!(ImageSource::parse("", "/h"), ImageSource::Empty);
        assert_eq!(ImageSource::parse("app.", "/h"), ImageSource::File("app.".into()));
    }

    #[test]
    fn loading() {
        let mut res = HeadlessResources::default();
        res.files.insert("/h/a.png".into(), Ok(Size::new(20.0, 10.0)));
        res.files.insert("/h/bad.gif".into(), Err(ImageError::DecodeFailed));
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut img = Image::default();
        assert!(img.set_prop("string", "~/a.png", &mut cx).unwrap());
        assert!(img.enabled);
        assert_eq!(img.bounds.size(), Size::new(20.0, 10.0));
        assert!(img.set_prop("scale", "2", &mut cx).unwrap());
        assert_eq!(img.bounds.size(), Size::new(40.0, 20.0));
        assert_eq!(
            img.load("/nope", &mut cx).unwrap_err().to_string(),
            "[!] Image: File '/nope' not found\n"
        );
        assert_eq!(img.path.as_deref(), Some("/nope"));
        assert!(img.key.is_some(), "failed load keeps the picture");
        assert!(img.load("~/bad.gif", &mut cx).unwrap());
        assert_eq!(cx.response, "Could not open image file at: /h/bad.gif\n");
        assert_eq!(
            img.load("app.Nope", &mut cx).unwrap_err().to_string(),
            "[!] Image: Invalid application name: 'Nope'\n"
        );
        assert!(!img.load("", &mut cx).unwrap());
        assert_eq!(img.path, None);
        assert!(img.enabled, "destroy keeps enabled");
        let mut out = String::new();
        img.write_json(&mut out, "\t");
        assert_eq!(out, "\t\"value\": \"(null)\",\n\t\"drawing\": \"on\",\n\t\"scale\": 2.000000");
        assert!(img.load("media.artwork", &mut cx).unwrap());
        assert!(cx.fx.begin_media_events);
        assert_eq!(
            img.set_prop("foo.x", "1", &mut cx).unwrap_err().to_string(),
            "[?] Image: Invalid subdomain: foo \n"
        );
    }
}
