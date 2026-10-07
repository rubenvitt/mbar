//! Font description (`font.c`, `docs/spec/components.md` §3).
//!
//! The core only stores the description; the platform resolves it (CoreText descriptor
//! matching + CTLine cascade on macOS, monospace metrics in `HeadlessResources`). Changes
//! set `Text::font_changed`, which triggers a relayout at the next length query (§4.4).

use crate::props::{set_f32, AnimValue, PropCx, PropError, PropResult};
use crate::value;

/// `struct font` without the CTFont handle.
#[derive(Debug, Clone, PartialEq)]
pub struct FontSpec {
    /// Default `"Hack Nerd Font"`.
    pub family: String,
    /// Default `"Bold"`.
    pub style: String,
    /// Default `14.0`.
    pub size: f32,
    /// Comma separated feature list (`+tnum,-liga,1:0`); `None` = no features. Not
    /// serialized, **not inherited** (`text_copy` quirk).
    pub features: Option<String>,
    /// Width from the typographic advance instead of the glyph bounds (§4.3).
    pub typographical_width: bool,
}

impl Default for FontSpec {
    fn default() -> Self {
        FontSpec {
            family: "Hack Nerd Font".to_string(),
            style: "Bold".to_string(),
            size: 14.0,
            features: None,
            typographical_width: false,
        }
    }
}

/// One parsed entry of `font.features` (`font_create_ctfont`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontFeature {
    /// `N:M` — AAT feature type / selector.
    Aat { kind: i32, selector: i32 },
    /// 4-byte OpenType tag with value (`+tag` → 1, `-tag` → 0, `tag` → 1).
    OpenType { tag: String, value: i32 },
}

/// `%254[^:]` of `sscanf`: 1..=254 bytes up to the next `:`. Returns `None` for an empty
/// field (the scan stops). Byte-truncation is rounded down to a char boundary.
fn scan_field(s: &str) -> Option<(&str, &str)> {
    let end = s.find(':').unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let mut take = end.min(254);
    while !s.is_char_boundary(take) {
        take -= 1;
    }
    Some((&s[..take], &s[take..]))
}

impl FontSpec {
    /// `sscanf(value, "%254[^:]:%254[^:]:%f", family, style, &size)` with `family = style =
    /// ""` and `size = 10.0` pre-initialised (§3.3). Returns `(family, style, size)`.
    pub fn parse_triplet(value: &str) -> (String, String, f32) {
        let mut family = String::new();
        let mut style = String::new();
        let mut size = 10.0f32;
        if let Some((f, rest)) = scan_field(value) {
            family = f.to_string();
            if let Some(rest) = rest.strip_prefix(':') {
                if let Some((st, rest)) = scan_field(rest) {
                    style = st.to_string();
                    if let Some(rest) = rest.strip_prefix(':') {
                        if let Some(v) = value::parse_float_prefix(rest) {
                            size = v;
                        }
                    }
                }
            }
        }
        (family, style, size)
    }

    /// `font=<Family>:<Style>:<Size>` (`font_set`): each part set with change detection.
    pub fn set_from_string(&mut self, value: &str) -> bool {
        let (family, style, size) = Self::parse_triplet(value);
        let mut changed = self.set_family(&family);
        changed |= self.set_style(&style);
        changed |= self.set_size(size);
        changed
    }

    pub fn set_family(&mut self, family: &str) -> bool {
        if self.family == family {
            return false;
        }
        self.family = family.to_string();
        true
    }

    pub fn set_style(&mut self, style: &str) -> bool {
        if self.style == style {
            return false;
        }
        self.style = style.to_string();
        true
    }

    pub fn set_size(&mut self, size: f32) -> bool {
        set_f32(&mut self.size, size)
    }

    pub fn set_features(&mut self, features: &str) -> bool {
        if self.features.as_deref() == Some(features) {
            return false;
        }
        self.features = Some(features.to_string());
        true
    }

    pub fn set_typographical_width(&mut self, on: bool) -> bool {
        if self.typographical_width == on {
            return false;
        }
        self.typographical_width = on;
        true
    }

    /// Parsed `features` (`strtok` on `,`: empty entries skipped; invalid entries skipped
    /// silently; OpenType tags must be exactly 4 bytes after the optional sign).
    pub fn parsed_features(&self) -> Vec<FontFeature> {
        let Some(f) = &self.features else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in f.split(',').filter(|e| !e.is_empty()) {
            if let Some((a, b)) = entry.split_once(':') {
                let a_ok = !a.is_empty() && a.trim_start_matches(['-', '+']).chars().all(|c| c.is_ascii_digit());
                let b_digits = b.trim_start_matches(['-', '+']);
                let b_num: String = b_digits.chars().take_while(|c| c.is_ascii_digit()).collect();
                if a_ok && !b_num.is_empty() {
                    out.push(FontFeature::Aat {
                        kind: value::parse_int(a),
                        selector: value::parse_int(b),
                    });
                    continue;
                }
            }
            let (val, tag) = match entry.as_bytes()[0] {
                b'+' => (1, &entry[1..]),
                b'-' => (0, &entry[1..]),
                _ => (1, entry),
            };
            if tag.len() == 4 {
                out.push(FontFeature::OpenType {
                    tag: tag.to_string(),
                    value: val,
                });
            }
        }
        out
    }

    /// `"<family>:<style>:<size %.2f>"` as printed in text JSON (§3.6).
    pub fn query_string(&self) -> String {
        format!(
            "{}:{}:{}",
            value::json_escape(&self.family),
            value::json_escape(&self.style),
            value::fmt_f2(self.size as f64)
        )
    }

    /// `font.<p>` sub-properties (`font_parse_sub_domain`, §3.4). There is no further
    /// sub-domain split: any other key (even dotted) is `[!] Text: Invalid property '<key>'`.
    /// Returns whether the font changed (the caller sets `font_changed`).
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "size" => {
                let from = AnimValue::Float(self.size);
                cx.animate("size", from, AnimValue::Float(value::parse_float(v)), |a, _| {
                    self.set_size(a.as_f32())
                })
            }
            "family" => self.set_family(v),
            "style" => self.set_style(v),
            "features" => self.set_features(v),
            "typographical_width" => {
                let on = value::parse_bool(v, self.typographical_width);
                self.set_typographical_width(on)
            }
            _ => return Err(PropError::TextInvalidProperty(value::display_key(key).to_string())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triplet_parsing() {
        assert_eq!(
            FontSpec::parse_triplet("Hack Nerd Font:Bold:17.0"),
            ("Hack Nerd Font".into(), "Bold".into(), 17.0)
        );
        assert_eq!(FontSpec::parse_triplet("SF Pro:Semibold"), ("SF Pro".into(), "Semibold".into(), 10.0));
        assert_eq!(FontSpec::parse_triplet("Menlo"), ("Menlo".into(), "".into(), 10.0));
        assert_eq!(FontSpec::parse_triplet(":Bold:12"), ("".into(), "".into(), 10.0));
        assert_eq!(FontSpec::parse_triplet("Fam::12"), ("Fam".into(), "".into(), 10.0));
        assert_eq!(FontSpec::parse_triplet("Fam:S:x"), ("Fam".into(), "S".into(), 10.0));
        let long = "a".repeat(300);
        assert_eq!(FontSpec::parse_triplet(&long).0.len(), 254);
    }

    #[test]
    fn features() {
        let f = FontSpec {
            features: Some("+tnum,-liga,,1:0,ss01,toolong".into()),
            ..FontSpec::default()
        };
        assert_eq!(
            f.parsed_features(),
            vec![
                FontFeature::OpenType { tag: "tnum".into(), value: 1 },
                FontFeature::OpenType { tag: "liga".into(), value: 0 },
                FontFeature::Aat { kind: 1, selector: 0 },
                FontFeature::OpenType { tag: "ss01".into(), value: 1 },
            ]
        );
        assert_eq!(FontSpec::default().query_string(), "Hack Nerd Font:Bold:14.00");
    }
}
