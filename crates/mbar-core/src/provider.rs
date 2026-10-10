//! Native data providers (mbar extension, `docs/EXTENSIONS.md` "Item properties").
//!
//! Item properties:
//!
//! | property | |
//! |---|---|
//! | `provider=<name>` | `clock`, `cpu`, `memory`, `battery`, `volume`, `wifi`, `network`, `disk`, `front_app`, `media`, `aerospace`; `none` disables |
//! | `provider.format=<template>` | label template with `{key}` placeholders, e.g. `"{percent}%"`; default per provider |
//! | `provider.icon_format=<template>` | optional icon template |
//! | `provider.freq=<seconds>` | sampling interval (float); event-driven providers (`volume`, `wifi`, `front_app`, `media`, `battery`) ignore it |
//! | `provider.args=<string>` | provider specific: strftime format for `clock` (default `%H:%M`), interface for `network`, mount point for `disk`, `workspace` (default) / `mode` / `monitor` for `aerospace` |
//!
//! Sampling is the platform's job (`PlatformRequest::StartProvider` →
//! `Input::ProviderSample`), except for core providers ([`ProviderKind::is_core`]):
//! `aerospace` is fed by the runtime from its AeroSpace state ([`aerospace_sample`], when
//! the provider is configured and after a state change that changes the item's sample)
//! and never reaches the platform. The core formats the templates into `label`/`icon` and also runs the item's
//! `script` with `SENDER=provider` and the sample as JSON in `INFO`.
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
//! | `aerospace` | `value` (selected by `provider.args`), `workspace`, `prev_workspace`, `mode`, `monitor` |
//!
//! [`ProviderConfig::set_prop`] (property layer), templates ([`format_template`],
//! [`apply_sample`], [`sample_info_json`]), defaults ([`ProviderKind::default_format`],
//! [`ProviderKind::default_freq`]) and the pure-Rust clock formatting ([`strftime`],
//! [`clock_sample`]) used by the platform's `clock` sampler.

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
    /// AeroSpace state (core provider, `docs/superpowers/specs/2026-10-09-aerospace-design.md`).
    Aerospace,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 11] = [
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
        ProviderKind::Aerospace,
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
            ProviderKind::Aerospace => "aerospace",
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
                | ProviderKind::Aerospace
        )
    }

    /// Core providers are fed by the runtime itself: no `PlatformRequest::StartProvider` /
    /// `StopProvider` is sent for them.
    pub fn is_core(self) -> bool {
        matches!(self, ProviderKind::Aerospace)
    }

    /// Default label template (`docs/EXTENSIONS.md` "Default templates").
    pub fn default_format(self) -> &'static str {
        match self {
            ProviderKind::Clock => "{time}",
            ProviderKind::Cpu => "{percent}%",
            ProviderKind::Memory => "{percent}%",
            ProviderKind::Battery => "{percent}%",
            ProviderKind::Volume => "{percent}%",
            ProviderKind::Wifi => "{ssid}",
            ProviderKind::Network => "↓{down} ↑{up}",
            ProviderKind::Disk => "{percent}%",
            ProviderKind::FrontApp => "{name}",
            ProviderKind::Media => "{title}",
            ProviderKind::Aerospace => "{value}",
        }
    }

    /// Default sampling interval in seconds for polled providers (`clock` 1, `cpu` 2,
    /// `memory` 5, `network` 2, `disk` 60). Event-driven providers return `0.0`
    /// (not polled; `provider.freq` is ignored for them).
    pub fn default_freq(self) -> f32 {
        match self {
            ProviderKind::Clock => 1.0,
            ProviderKind::Cpu => 2.0,
            ProviderKind::Memory => 5.0,
            ProviderKind::Network => 2.0,
            ProviderKind::Disk => 60.0,
            ProviderKind::Battery
            | ProviderKind::Volume
            | ProviderKind::Wifi
            | ProviderKind::FrontApp
            | ProviderKind::Media
            | ProviderKind::Aerospace => 0.0,
        }
    }

    /// Default `provider.args` (`clock`: [`DEFAULT_CLOCK_FORMAT`]).
    pub fn default_args(self) -> Option<&'static str> {
        match self {
            ProviderKind::Clock => Some(DEFAULT_CLOCK_FORMAT),
            _ => None,
        }
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

/// Expands `{key}` placeholders with the sample values (first matching key wins);
/// unknown keys expand to the empty string, `{{`/`}}` are literal braces. A `{` without a
/// closing `}` (or with another `{` before it) and a lone `}` are copied literally.
pub fn format_template(template: &str, values: &[(String, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let brace = rest.as_bytes()[i];
        let after = &rest[i + 1..];
        if after.as_bytes().first() == Some(&brace) {
            // `{{` / `}}`
            out.push(brace as char);
            rest = &after[1..];
        } else if brace == b'}' {
            out.push('}');
            rest = after;
        } else {
            let inner = after;
            match inner.find(['{', '}']) {
                Some(j) if inner.as_bytes()[j] == b'}' => {
                    let key = &inner[..j];
                    if let Some((_, v)) = values.iter().find(|(k, _)| k == key) {
                        out.push_str(v);
                    }
                    rest = &inner[j + 1..];
                }
                _ => {
                    out.push('{');
                    rest = inner;
                }
            }
        }
    }
    out.push_str(rest);
    out
}

/// The sample as a flat JSON object (`INFO` for `SENDER=provider`), keys in sample order,
/// every value a JSON string, laid out like SketchyBar's INFO payloads:
/// `{\n\t"k": "v",\n\t"k2": "v2"\n}` (no trailing newline; empty sample `{\n}`).
pub fn sample_info_json(values: &[(String, String)]) -> String {
    let mut out = String::from("{\n");
    for (i, (k, v)) in values.iter().enumerate() {
        out.push_str("\t\"");
        out.push_str(&value::json_escape(k));
        out.push_str("\": \"");
        out.push_str(&value::json_escape(v));
        out.push('"');
        if i + 1 < values.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push('}');
    out
}

/// What the runtime should do with a sample: new label/icon strings (if templates apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOutput {
    pub label: Option<String>,
    pub icon: Option<String>,
    pub info: String,
}

/// Formats a sample for an item's provider config: `label` from `provider.format` (the
/// provider's [`ProviderKind::default_format`] when unset), `icon` only with
/// `provider.icon_format`. An explicitly empty template leaves that text untouched
/// (`None`), so `provider.format=""` turns a provider into a pure script feed. Without a
/// provider (`kind == None`) neither text is produced.
pub fn apply_sample(cfg: &ProviderConfig, values: &[(String, String)]) -> ProviderOutput {
    let info = sample_info_json(values);
    let Some(kind) = cfg.kind else {
        return ProviderOutput {
            label: None,
            icon: None,
            info,
        };
    };
    let fmt = |t: &str| (!t.is_empty()).then(|| format_template(t, values));
    ProviderOutput {
        label: fmt(cfg.format.as_deref().unwrap_or(kind.default_format())),
        icon: cfg.icon_format.as_deref().and_then(fmt),
        info,
    }
}

/// The `aerospace` sample for `args` (`provider.args`): `value` is the focused workspace
/// (args unset, empty, `workspace` or unknown), the binding mode (`mode`) or the focused
/// monitor id (`monitor`, empty while unknown); the other keys are always present.
pub fn aerospace_sample(
    state: &crate::aerospace::AerospaceState,
    args: Option<&str>,
) -> Vec<(String, String)> {
    let monitor = if state.monitor == 0 {
        String::new()
    } else {
        state.monitor.to_string()
    };
    let value = match args.unwrap_or("") {
        "mode" => state.mode.clone(),
        "monitor" => monitor.clone(),
        _ => state.focused_workspace.clone(),
    };
    [
        ("value", value),
        ("workspace", state.focused_workspace.clone()),
        ("prev_workspace", state.prev_workspace.clone()),
        ("mode", state.mode.clone()),
        ("monitor", monitor),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Default `clock` format (`provider.args` unset or empty).
pub const DEFAULT_CLOCK_FORMAT: &str = "%H:%M";

/// Broken-down local time supplied by the platform (`struct tm` equivalent; no libc
/// locale or time-zone database is consulted by the core).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalTime {
    /// Full year, e.g. 2026.
    pub year: i32,
    /// 1..=12.
    pub month: u32,
    /// 1..=31.
    pub day: u32,
    /// 0..=23.
    pub hour: u32,
    /// 0..=59.
    pub minute: u32,
    /// 0..=60.
    pub second: u32,
    /// 0 = Sunday … 6 = Saturday (`tm_wday`).
    pub weekday: u32,
    /// Day of the year, 0-based (`tm_yday`).
    pub yday: u32,
    /// Offset from UTC in seconds (east positive, `tm_gmtoff`).
    pub utc_offset: i32,
    /// Zone abbreviation (`%Z`); `None` prints nothing.
    pub zone: Option<String>,
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `strftime` in the C/POSIX locale, pure Rust.
///
/// Supported conversions: `%a %A %b %h %B %c %C %d %D %e %F %H %I %j %k %l %m %M %n %p %R
/// %S %t %T %u %w %x %X %y %Y %z %Z %%`. Numeric conversions accept the BSD/GNU padding
/// flags `-` (no padding), `_` (space) and `0` (zero) between `%` and the letter. Unknown
/// conversions are copied literally (`%Q` → `%Q`), a trailing lone `%` prints `%`.
pub fn strftime(format: &str, t: &LocalTime) -> String {
    let mut out = String::with_capacity(format.len() + 16);
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let flag = match chars.peek() {
            Some(&f @ ('-' | '_' | '0')) => {
                chars.next();
                Some(f)
            }
            _ => None,
        };
        let Some(spec) = chars.next() else {
            out.push('%');
            if let Some(f) = flag {
                out.push(f);
            }
            break;
        };
        let num = |out: &mut String, v: i64, width: usize, default_pad: char| {
            let pad = match flag {
                Some('-') => None,
                Some('_') => Some(' '),
                Some('0') => Some('0'),
                _ => Some(default_pad),
            };
            let digits = v.unsigned_abs().to_string();
            if v < 0 {
                out.push('-');
            }
            if let Some(p) = pad {
                for _ in digits.len()..width {
                    out.push(p);
                }
            }
            out.push_str(&digits);
        };
        let h12 = match t.hour % 12 {
            0 => 12,
            h => h,
        } as i64;
        let wd = WEEKDAYS[(t.weekday % 7) as usize];
        let mon = MONTHS[(t.month.clamp(1, 12) - 1) as usize];
        match spec {
            'a' => out.push_str(&wd[..3]),
            'A' => out.push_str(wd),
            'b' | 'h' => out.push_str(&mon[..3]),
            'B' => out.push_str(mon),
            'c' => out.push_str(&strftime("%a %b %e %H:%M:%S %Y", t)),
            'C' => num(&mut out, t.year.div_euclid(100) as i64, 2, '0'),
            'd' => num(&mut out, t.day as i64, 2, '0'),
            'D' | 'x' => out.push_str(&strftime("%m/%d/%y", t)),
            'e' => num(&mut out, t.day as i64, 2, ' '),
            'F' => out.push_str(&strftime("%Y-%m-%d", t)),
            'H' => num(&mut out, t.hour as i64, 2, '0'),
            'I' => num(&mut out, h12, 2, '0'),
            'j' => num(&mut out, t.yday as i64 + 1, 3, '0'),
            'k' => num(&mut out, t.hour as i64, 2, ' '),
            'l' => num(&mut out, h12, 2, ' '),
            'm' => num(&mut out, t.month as i64, 2, '0'),
            'M' => num(&mut out, t.minute as i64, 2, '0'),
            'n' => out.push('\n'),
            'p' => out.push_str(if t.hour < 12 { "AM" } else { "PM" }),
            'R' => out.push_str(&strftime("%H:%M", t)),
            'S' => num(&mut out, t.second as i64, 2, '0'),
            't' => out.push('\t'),
            'T' | 'X' => out.push_str(&strftime("%H:%M:%S", t)),
            'u' => num(
                &mut out,
                if t.weekday % 7 == 0 {
                    7
                } else {
                    t.weekday as i64 % 7
                },
                1,
                '0',
            ),
            'w' => num(&mut out, (t.weekday % 7) as i64, 1, '0'),
            'y' => num(&mut out, t.year.rem_euclid(100) as i64, 2, '0'),
            'Y' => num(&mut out, t.year as i64, 1, '0'),
            'z' => {
                let o = t.utc_offset;
                out.push(if o < 0 { '-' } else { '+' });
                let a = o.unsigned_abs();
                let _ = std::fmt::Write::write_fmt(
                    &mut out,
                    format_args!("{:02}{:02}", a / 3600, (a % 3600) / 60),
                );
            }
            'Z' => out.push_str(t.zone.as_deref().unwrap_or("")),
            '%' => out.push('%'),
            other => {
                out.push('%');
                if let Some(f) = flag {
                    out.push(f);
                }
                out.push(other);
            }
        }
    }
    out
}

/// The `clock` sample for local time `t`: `[("time", strftime(args or "%H:%M", t))]`.
/// The platform calls this (with `provider.args`) and posts the result as
/// `Input::ProviderSample`.
pub fn clock_sample(args: Option<&str>, t: &LocalTime) -> Vec<(String, String)> {
    let fmt = match args {
        Some(a) if !a.is_empty() => a,
        _ => DEFAULT_CLOCK_FORMAT,
    };
    vec![("time".to_string(), strftime(fmt, t))]
}

/// Human-readable transfer rate for the `network` provider's `down`/`up` keys, base 1024:
/// `"512 B/s"`, `"12.3 KB/s"`, `"1.2 MB/s"`, `"3.0 GB/s"` (one decimal from KB/s on).
pub fn format_rate(bytes_per_sec: f64) -> String {
    let b = if bytes_per_sec.is_finite() && bytes_per_sec > 0.0 {
        bytes_per_sec
    } else {
        0.0
    };
    if b < 1024.0 {
        return format!("{} B/s", b as u64);
    }
    const UNITS: [&str; 3] = ["KB/s", "MB/s", "GB/s"];
    let mut v = b / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
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
