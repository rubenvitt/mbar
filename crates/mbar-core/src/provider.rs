//! Native data providers (mbar extension, `docs/EXTENSIONS.md` "Item properties").
//!
//! Item properties:
//!
//! | property | |
//! |---|---|
//! | `provider=<name>` | `clock`, `cpu`, `memory`, `battery`, `volume`, `wifi`, `network`, `disk`, `front_app`, `media`; `none` disables |
//! | `provider.format=<template>` | label template with `{key}` placeholders, e.g. `"{percent}%"`; default per provider |
//! | `provider.icon_format=<template>` | optional icon template |
//! | `provider.freq=<seconds>` | sampling interval (float); event-driven providers (`volume`, `wifi`, `front_app`, `media`, `battery`) ignore it |
//! | `provider.args=<string>` | provider specific: strftime format for `clock` (default `%H:%M`), interface for `network`, mount point for `disk` |
//!
//! Sampling is the platform's job (`PlatformRequest::StartProvider` →
//! `Input::ProviderSample`). The core formats the templates into `label`/`icon` and also
//! runs the item's `script` with `SENDER=provider` and the sample as JSON in `INFO`.
//!
//! Keys per provider:
//!
//! | provider | keys |
//! |---|---|
//! | `clock` | `time` |
//! | `cpu` | `percent`, `user`, `sys` |
//! | `memory` | `percent`, `used_gb`, `total_gb` |
//! | `battery` | `percent`, `charging` (`true`/`false`), `remaining` (`h:mm` or empty) |
//! | `volume` | `percent`, `muted` |
//! | `wifi` | `ssid`, `rssi` |
//! | `network` | `down`, `up` (human readable per second), `down_bytes`, `up_bytes` |
//! | `disk` | `percent`, `free_gb`, `total_gb` |
//! | `front_app` | `name`, `bundle_id` |
//! | `media` | `title`, `artist`, `album`, `app`, `state` |
//!
//! [`ProviderConfig::set_prop`] is complete (the item property layer needs it);
//! formatting and scheduling helpers are WP-D.

use crate::props::{PropCx, PropError, PropRequest, PropResult};
use crate::value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Clock,
    Cpu,
    Memory,
    Battery,
    Volume,
    Wifi,
    Network,
    Disk,
    FrontApp,
    Media,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 10] = [
        ProviderKind::Clock,
        ProviderKind::Cpu,
        ProviderKind::Memory,
        ProviderKind::Battery,
        ProviderKind::Volume,
        ProviderKind::Wifi,
        ProviderKind::Network,
        ProviderKind::Disk,
        ProviderKind::FrontApp,
        ProviderKind::Media,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::Clock => "clock",
            ProviderKind::Cpu => "cpu",
            ProviderKind::Memory => "memory",
            ProviderKind::Battery => "battery",
            ProviderKind::Volume => "volume",
            ProviderKind::Wifi => "wifi",
            ProviderKind::Network => "network",
            ProviderKind::Disk => "disk",
            ProviderKind::FrontApp => "front_app",
            ProviderKind::Media => "media",
        }
    }

    pub fn from_name(name: &str) -> Option<ProviderKind> {
        ProviderKind::ALL.into_iter().find(|p| p.name() == name)
    }

    /// Event-driven providers ignore `provider.freq`.
    pub fn is_event_driven(self) -> bool {
        matches!(
            self,
            ProviderKind::Volume
                | ProviderKind::Wifi
                | ProviderKind::FrontApp
                | ProviderKind::Media
                | ProviderKind::Battery
        )
    }

    /// Default label template (e.g. clock `{time}`, cpu `{percent}%`). WP-D.
    pub fn default_format(self) -> &'static str {
        todo!("WP-D: choose and document defaults per provider")
    }

    /// Default sampling interval in seconds for polled providers. WP-D.
    pub fn default_freq(self) -> f32 {
        todo!("WP-D")
    }
}

/// An item's provider configuration (`BarItem::provider`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderConfig {
    /// `None` = `provider=none` / never set.
    pub kind: Option<ProviderKind>,
    pub format: Option<String>,
    pub icon_format: Option<String>,
    /// `None` = provider default.
    pub freq: Option<f32>,
    pub args: Option<String>,
}

impl ProviderConfig {
    /// `provider=<name>` (`key == ""`) and `provider.<p>` (`key == p`). Pushes
    /// `PropRequest::ProviderChanged` on every successful change. `item` is the item name
    /// for error messages.
    pub fn set_prop(
        &mut self,
        key: &str,
        v: &str,
        item: Option<&str>,
        cx: &mut PropCx,
    ) -> PropResult {
        let changed = match key {
            "" => {
                let kind = if v == "none" {
                    None
                } else {
                    match ProviderKind::from_name(v) {
                        Some(k) => Some(k),
                        None => {
                            return Err(PropError::ItemInvalidProvider {
                                item: item.map(str::to_string),
                                name: v.to_string(),
                            })
                        }
                    }
                };
                let c = self.kind != kind;
                self.kind = kind;
                c
            }
            "format" => replace(&mut self.format, v),
            "icon_format" => replace(&mut self.icon_format, v),
            "freq" => {
                let f = Some(value::parse_float(v));
                let c = self.freq != f;
                self.freq = f;
                c
            }
            "args" => replace(&mut self.args, v),
            _ => {
                return Err(PropError::ProviderInvalidProperty(
                    value::display_key(key).to_string(),
                ))
            }
        };
        if changed {
            cx.request(PropRequest::ProviderChanged);
        }
        Ok(changed)
    }

    /// Writes the extension query block (`"provider": {…}` fields) at indent `i`.
    pub fn write_json(&self, out: &mut String, i: &str) {
        use std::fmt::Write;
        let s = |o: &Option<String>| value::json_opt(o.as_deref());
        let _ = write!(
            out,
            "{i}\"name\": \"{}\",\n{i}\"format\": \"{}\",\n{i}\"icon_format\": \"{}\",\n{i}\"freq\": {},\n{i}\"args\": \"{}\"",
            self.kind.map(|k| k.name()).unwrap_or("none"),
            s(&self.format),
            s(&self.icon_format),
            value::fmt_f(self.freq.unwrap_or(0.0) as f64),
            s(&self.args)
        );
    }
}

fn replace(field: &mut Option<String>, v: &str) -> bool {
    if field.as_deref() == Some(v) {
        return false;
    }
    *field = Some(v.to_string());
    true
}

/// Expands `{key}` placeholders with the sample values; unknown keys expand to the empty
/// string, `{{`/`}}` are literal braces. WP-D.
pub fn format_template(template: &str, values: &[(String, String)]) -> String {
    let _ = (template, values);
    todo!("WP-D")
}

/// The sample as a flat JSON object (`INFO` for `SENDER=provider`), keys in sample order,
/// strings JSON-escaped. WP-D.
pub fn sample_info_json(values: &[(String, String)]) -> String {
    let _ = values;
    todo!("WP-D")
}

/// What the runtime should do with a sample: new label/icon strings (if templates apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOutput {
    pub label: Option<String>,
    pub icon: Option<String>,
    pub info: String,
}

/// Formats a sample for an item's provider config (default template when `format` is
/// unset; `icon` only with `icon_format`). WP-D.
pub fn apply_sample(cfg: &ProviderConfig, values: &[(String, String)]) -> ProviderOutput {
    let _ = (cfg, values);
    todo!("WP-D")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::Animator;
    use crate::platform::HeadlessResources;

    #[test]
    fn config_props() {
        let mut res = HeadlessResources::default();
        let mut an = Animator::new();
        let mut cx = PropCx::new(&mut res, &mut an, "/h");
        let mut p = ProviderConfig::default();
        assert!(p.set_prop("", "cpu", Some("x"), &mut cx).unwrap());
        assert_eq!(p.kind, Some(ProviderKind::Cpu));
        assert!(p
            .set_prop("format", "{percent}%", Some("x"), &mut cx)
            .unwrap());
        assert!(!p
            .set_prop("format", "{percent}%", Some("x"), &mut cx)
            .unwrap());
        assert_eq!(cx.requests.len(), 2);
        assert_eq!(
            p.set_prop("", "gpu", Some("x"), &mut cx)
                .unwrap_err()
                .to_string(),
            "[!] Item (x): Invalid provider 'gpu'\n"
        );
        assert!(p.set_prop("", "none", Some("x"), &mut cx).unwrap());
        assert_eq!(p.kind, None);
    }
}
