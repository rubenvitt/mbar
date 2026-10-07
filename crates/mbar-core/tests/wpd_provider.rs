//! WP-D: provider templates, defaults, INFO JSON and the pure-Rust clock formatting
//! (`docs/EXTENSIONS.md`).

use mbar_core::provider::{
    apply_sample, clock_sample, format_rate, format_template, sample_info_json, strftime,
    LocalTime, ProviderConfig, ProviderKind, ProviderOutput, DEFAULT_CLOCK_FORMAT,
};

fn vals(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn templates() {
    let v = vals(&[("percent", "42"), ("user", "30"), ("sys", "12")]);
    assert_eq!(format_template("{percent}%", &v), "42%");
    assert_eq!(format_template("u{user} s{sys}", &v), "u30 s12");
    assert_eq!(format_template("{missing}!", &v), "!");
    assert_eq!(format_template("{{percent}}", &v), "{percent}");
    assert_eq!(format_template("{{{percent}}}", &v), "{42}");
    assert_eq!(format_template("open {percent", &v), "open {percent");
    assert_eq!(format_template("a } b", &v), "a } b");
    assert_eq!(format_template("{a{percent}", &v), "{a42");
    assert_eq!(format_template("", &v), "");
    assert_eq!(format_template("plain", &[]), "plain");
    assert_eq!(format_template("{}", &vals(&[("", "e")])), "e");
    // Unicode around placeholders.
    assert_eq!(format_template("↓{percent}↑", &v), "↓42↑");
    // First matching key wins.
    assert_eq!(
        format_template("{k}", &vals(&[("k", "1"), ("k", "2")])),
        "1"
    );
}

#[test]
fn info_json() {
    assert_eq!(
        sample_info_json(&vals(&[("percent", "42"), ("charging", "true")])),
        "{\n\t\"percent\": \"42\",\n\t\"charging\": \"true\"\n}"
    );
    assert_eq!(sample_info_json(&[]), "{\n}");
    assert_eq!(
        sample_info_json(&vals(&[("title", "a \"b\"\\\n")])),
        "{\n\t\"title\": \"a \\\"b\\\"\\\\\\n\"\n}"
    );
    // Parses as JSON.
    let parsed: serde_json::Value =
        serde_json::from_str(&sample_info_json(&vals(&[("x", "\u{1}y"), ("z", "")]))).unwrap();
    assert_eq!(parsed["x"], "\u{1}y");
    assert_eq!(parsed["z"], "");
}

#[test]
fn defaults() {
    assert_eq!(ProviderKind::Clock.default_format(), "{time}");
    assert_eq!(ProviderKind::Cpu.default_format(), "{percent}%");
    assert_eq!(ProviderKind::Memory.default_format(), "{percent}%");
    assert_eq!(ProviderKind::Battery.default_format(), "{percent}%");
    assert_eq!(ProviderKind::Volume.default_format(), "{percent}%");
    assert_eq!(ProviderKind::Wifi.default_format(), "{ssid}");
    assert_eq!(ProviderKind::Network.default_format(), "↓{down} ↑{up}");
    assert_eq!(ProviderKind::Disk.default_format(), "{percent}%");
    assert_eq!(ProviderKind::FrontApp.default_format(), "{name}");
    assert_eq!(ProviderKind::Media.default_format(), "{title}");
    for k in ProviderKind::ALL {
        if k.is_event_driven() {
            assert_eq!(k.default_freq(), 0.0, "{k:?}");
        } else {
            assert!(k.default_freq() > 0.0, "{k:?}");
        }
    }
    assert_eq!(ProviderKind::Clock.default_freq(), 1.0);
    assert_eq!(ProviderKind::Cpu.default_freq(), 2.0);
    assert_eq!(
        ProviderKind::Clock.default_args(),
        Some(DEFAULT_CLOCK_FORMAT)
    );
    assert_eq!(ProviderKind::Disk.default_args(), None);
}

#[test]
fn apply() {
    let v = vals(&[
        ("percent", "7"),
        ("charging", "false"),
        ("remaining", "1:05"),
    ]);
    let mut cfg = ProviderConfig {
        kind: Some(ProviderKind::Battery),
        ..Default::default()
    };
    let out = apply_sample(&cfg, &v);
    assert_eq!(
        out,
        ProviderOutput {
            label: Some("7%".into()),
            icon: None,
            info: sample_info_json(&v),
        }
    );
    cfg.format = Some("{percent}% ({remaining})".into());
    cfg.icon_format = Some("{charging}".into());
    let out = apply_sample(&cfg, &v);
    assert_eq!(out.label.as_deref(), Some("7% (1:05)"));
    assert_eq!(out.icon.as_deref(), Some("false"));
    // Empty templates leave the texts untouched.
    cfg.format = Some(String::new());
    cfg.icon_format = Some(String::new());
    let out = apply_sample(&cfg, &v);
    assert_eq!(out.label, None);
    assert_eq!(out.icon, None);
    // No provider: info only.
    let out = apply_sample(&ProviderConfig::default(), &v);
    assert_eq!(out.label, None);
    assert_eq!(out.icon, None);
    assert_eq!(out.info, sample_info_json(&v));
}

fn sample_time() -> LocalTime {
    // Wednesday 2026-10-07 09:05:03, day 280 of the year, CEST.
    LocalTime {
        year: 2026,
        month: 10,
        day: 7,
        hour: 9,
        minute: 5,
        second: 3,
        weekday: 3,
        yday: 279,
        utc_offset: 7200,
        zone: Some("CEST".into()),
    }
}

#[test]
fn strftime_specifiers() {
    let t = sample_time();
    assert_eq!(strftime("%H:%M", &t), "09:05");
    assert_eq!(strftime("%a %A", &t), "Wed Wednesday");
    assert_eq!(strftime("%b %B %h", &t), "Oct October Oct");
    assert_eq!(strftime("%d|%e|%m", &t), "07| 7|10");
    assert_eq!(strftime("%I %p", &t), "09 AM");
    assert_eq!(strftime("%S", &t), "03");
    assert_eq!(strftime("%y %Y %C", &t), "26 2026 20");
    assert_eq!(strftime("%j", &t), "280");
    assert_eq!(strftime("%Z %z", &t), "CEST +0200");
    assert_eq!(strftime("100%%", &t), "100%");
    assert_eq!(strftime("%D %F", &t), "10/07/26 2026-10-07");
    assert_eq!(strftime("%T %R", &t), "09:05:03 09:05");
    assert_eq!(strftime("%c", &t), "Wed Oct  7 09:05:03 2026");
    assert_eq!(strftime("%x %X", &t), "10/07/26 09:05:03");
    assert_eq!(strftime("%u %w", &t), "3 3");
    assert_eq!(strftime("%k|%l", &t), " 9| 9");
    assert_eq!(strftime("a%nb%tc", &t), "a\nb\tc");
    // Padding flags.
    assert_eq!(strftime("%-d/%-m %-H:%M", &t), "7/10 9:05");
    assert_eq!(strftime("%_H %0e", &t), " 9 07");
    // Unknown conversions and a trailing `%` are literal.
    assert_eq!(strftime("%Q %-Q %", &t), "%Q %-Q %");
    // Unicode text passes through.
    assert_eq!(strftime("⏰ %H", &t), "⏰ 09");
}

#[test]
fn strftime_edges() {
    let mut t = sample_time();
    t.hour = 0;
    assert_eq!(strftime("%I %l %p", &t), "12 12 AM");
    t.hour = 12;
    assert_eq!(strftime("%I %p", &t), "12 PM");
    t.hour = 23;
    assert_eq!(strftime("%I %p %H", &t), "11 PM 23");
    t.weekday = 0;
    assert_eq!(strftime("%a %u %w", &t), "Sun 7 0");
    t.yday = 0;
    assert_eq!(strftime("%j", &t), "001");
    t.utc_offset = -(5 * 3600 + 30 * 60);
    t.zone = None;
    assert_eq!(strftime("[%Z] %z", &t), "[] -0530");
    t.year = 2005;
    assert_eq!(strftime("%y", &t), "05");
}

#[test]
fn clock() {
    let t = sample_time();
    assert_eq!(clock_sample(None, &t), vals(&[("time", "09:05")]));
    assert_eq!(clock_sample(Some(""), &t), vals(&[("time", "09:05")]));
    assert_eq!(
        clock_sample(Some("%d/%m %H:%M"), &t),
        vals(&[("time", "07/10 09:05")])
    );
    let cfg = ProviderConfig {
        kind: Some(ProviderKind::Clock),
        ..Default::default()
    };
    assert_eq!(
        apply_sample(&cfg, &clock_sample(None, &t)).label.as_deref(),
        Some("09:05")
    );
}

#[test]
fn rates() {
    assert_eq!(format_rate(0.0), "0 B/s");
    assert_eq!(format_rate(-5.0), "0 B/s");
    assert_eq!(format_rate(f64::NAN), "0 B/s");
    assert_eq!(format_rate(512.0), "512 B/s");
    assert_eq!(format_rate(1023.9), "1023 B/s");
    assert_eq!(format_rate(1024.0), "1.0 KB/s");
    assert_eq!(format_rate(12.3 * 1024.0), "12.3 KB/s");
    assert_eq!(format_rate(1.5 * 1024.0 * 1024.0), "1.5 MB/s");
    assert_eq!(format_rate(3.0 * 1024.0 * 1024.0 * 1024.0), "3.0 GB/s");
    assert_eq!(
        format_rate(5000.0 * 1024.0 * 1024.0 * 1024.0),
        "5000.0 GB/s"
    );
}
