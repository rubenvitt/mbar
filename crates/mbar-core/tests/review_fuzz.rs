//! Deterministic fuzz-style robustness test: generated argv sequences through
//! `Runtime::handle` + `frame()` with `HeadlessResources`; asserts no panic.

use mbar_core::geometry::Point;
use mbar_core::platform::{
    HeadlessResources, Input, MouseButton, MouseInput, MouseKind, OsEvent, ReplyToken, WindowKey,
};
use mbar_core::{Runtime, RuntimeConfig};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &'a [&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

const NAMES: &[&str] = &[
    "a", "b", "c", "g", "s", "br", "x.y", "🙂", "é", "a,b", "", "/a/", "/", "//", "-x",
];
const TYPES: &[&str] = &[
    "item", "space", "alias", "bracket", "graph", "slider", "event", "bogus", "",
];
const POS: &[&str] = &[
    "left",
    "right",
    "center",
    "q",
    "e",
    "popup.a",
    "popup.b",
    "popup.🙂",
    "popup.",
    "x",
    "",
];
const VALUES: &[&str] = &[
    "0",
    "1",
    "-1",
    "on",
    "off",
    "toggle",
    "yes",
    "no",
    "true",
    "",
    "🙂🙂🙂",
    "éé",
    "a",
    "0xffffffff",
    "0xff00ff00",
    "0x",
    "0x1",
    "-0xffffffff",
    "4294967295",
    "4294967296",
    "2147483647",
    "-2147483648",
    "99999999999999999999999",
    "-99999999999999999999999",
    "1e40",
    "-1e40",
    "nan",
    "inf",
    "-inf",
    "3.5",
    "0.0000001",
    "1e-45",
    "Hack Nerd Font:Bold:14.0",
    ":::",
    "x:y:z:w",
    "a:b:nan",
    "a:b:-5",
    "a:b:1e30",
    "left",
    "right",
    "center",
    "top",
    "bottom",
    "above",
    "below",
    "app.Safari",
    "space.1",
    "media.artwork",
    "/nonexist.png",
    "~/x",
    "$HOME",
    "=",
    "==",
    "a=b",
    "100000000",
    "-100000000",
    "65536",
    "1000000000",
    "18446744073709551615",
    "-9223372036854775808",
    "active",
    "main",
    "all",
    "1,2,3",
    "0,0",
    "-1,-1",
    ",",
    "🙂:🙂:🙂",
    "\u{301}",
    "a\u{301}b",
    "ﷺ",
    "\n",
    "\t",
    "\\",
];
const KEYS: &[&str] = &[
    "drawing",
    "updates",
    "position",
    "space",
    "display",
    "ignore_association",
    "y_offset",
    "width",
    "scroll_texts",
    "blur_radius",
    "background.drawing",
    "background.color",
    "background.height",
    "background.corner_radius",
    "background.border_width",
    "background.padding_left",
    "background.padding_right",
    "background.y_offset",
    "background.x_offset",
    "background.image",
    "background.image.scale",
    "background.image.corner_radius",
    "background.shadow.drawing",
    "background.shadow.angle",
    "background.shadow.distance",
    "background.clip",
    "icon",
    "icon.drawing",
    "icon.font",
    "icon.font.size",
    "icon.font.family",
    "icon.font.style",
    "icon.color",
    "icon.padding_left",
    "icon.padding_right",
    "icon.width",
    "icon.max_chars",
    "icon.scroll_duration",
    "icon.align",
    "icon.y_offset",
    "icon.shadow.drawing",
    "icon.shadow.distance",
    "icon.shadow.angle",
    "icon.highlight",
    "icon.background.drawing",
    "icon.background.height",
    "label",
    "label.drawing",
    "label.font",
    "label.max_chars",
    "label.scroll_duration",
    "label.width",
    "label.align",
    "label.padding_left",
    "label.padding_right",
    "label.color",
    "label.highlight",
    "label.shadow.drawing",
    "label.background.drawing",
    "label.background.image",
    "label.y_offset",
    "label.string",
    "label.font.size",
    "update_freq",
    "script",
    "click_script",
    "mach_helper",
    "padding_left",
    "padding_right",
    "align",
    "associated_space",
    "associated_display",
    "popup.drawing",
    "popup.horizontal",
    "popup.align",
    "popup.height",
    "popup.topmost",
    "popup.y_offset",
    "popup.background.drawing",
    "popup.blur_radius",
    "graph.color",
    "graph.fill_color",
    "graph.line_width",
    "slider.percentage",
    "slider.width",
    "slider.knob",
    "slider.knob.drawing",
    "slider.highlight_color",
    "slider.background.drawing",
    "slider.background.height",
    "alias.color",
    "alias.scale",
    "image",
    "image.scale",
    "image.drawing",
    "image.corner_radius",
    "image.border_width",
    "image.padding_left",
    "image.padding_right",
    "image.y_offset",
    "provider",
    "provider.freq",
    "app_menu",
    "max_titles",
    "scroll",
    "highlight",
    "x_offset",
    "label.highlight_color",
    "icon.string",
    "bogus",
    "background",
    "label.font.family",
    "label.font.style",
    "label.max_chars",
];
const BAR_KEYS: &[&str] = &[
    "position",
    "height",
    "margin",
    "y_offset",
    "corner_radius",
    "border_width",
    "border_color",
    "color",
    "blur_radius",
    "padding_left",
    "padding_right",
    "display",
    "topmost",
    "sticky",
    "hidden",
    "shadow",
    "font_smoothing",
    "notch_width",
    "notch_offset",
    "notch_display_height",
    "drawing",
    "image",
    "image.scale",
    "show_in_fullscreen",
    "hide_menubar",
    "bogus",
];
const CURVES: &[&str] = &[
    "linear",
    "quadratic",
    "tanh",
    "sin",
    "exp",
    "circ",
    "bogus",
    "",
];
const EVENTS: &[&str] = &[
    "mouse.entered",
    "mouse.exited",
    "mouse.clicked",
    "mouse.scrolled",
    "front_app_switched",
    "space_change",
    "routine",
    "forced",
    "my_event",
    "volume_change",
    "system_woke",
    "media_change",
    "display_change",
    "space_windows_change",
    "wifi_change",
    "mouse.entered.global",
    "mouse.exited.global",
    "menus_change",
    "🙂",
];
const REGEXES: &[&str] = &[
    "/a/",
    "/.*/",
    "/^a$/",
    "/[a-z]/",
    "/\\(a\\)\\{2\\}/",
    "/[[:alpha:]]/",
    "/[/",
    "/\\/",
    "/a\\{255\\}/",
    "/\\(\\(a*\\)*\\)*/",
    "/🙂/",
    "/é./",
    "/[é-ü]/",
    "/\\{/",
    "/*/",
    "/\\(a\\{255\\}\\)\\{255\\}/",
    "/[[:x/",
    "/[[.a/",
    "/^*$/",
    "/\\|/",
    "/a\\|/",
];

const EXTREME: &[&str] = &[
    "2147483647",
    "-2147483648",
    "4294967295",
    "-4294967295",
    "2147483648",
    "-2147483649",
    "1e38",
    "-1e38",
    "3e38",
    "-3e38",
    "inf",
    "-inf",
    "nan",
    "65535",
    "-65535",
    "1000000",
];

fn value(r: &mut Rng) -> String {
    if r.chance(30) {
        return r.pick(EXTREME).to_string();
    }
    if r.chance(10) {
        // long / weird string
        let n = r.below(300);
        let unit = r.pick(&["a", "🙂", "é", "\u{301}", "👨‍👩‍👧", " ", "\u{0}"]);
        return unit.repeat(n);
    }
    r.pick(VALUES).to_string()
}

/// Avoids the two already-reported i32::MIN negation panics so others can surface
/// (set FUZZ_KNOWN=1 to keep them).
fn kv(k: &str, v: String) -> String {
    let known = std::env::var("FUZZ_KNOWN").is_ok();
    if !known && k.ends_with("offset") && mbar_core::value::parse_int(&v) == i32::MIN {
        return format!("{k}=0");
    }
    format!("{k}={v}")
}

fn name(r: &mut Rng) -> String {
    r.pick(NAMES).to_string()
}

fn selector(r: &mut Rng) -> String {
    if r.chance(25) {
        r.pick(REGEXES).to_string()
    } else {
        name(r)
    }
}

fn gen_message(r: &mut Rng) -> Vec<String> {
    let mut out = Vec::new();
    let n = 1 + r.below(4);
    for _ in 0..n {
        match r.below(22) {
            0..=2 => {
                out.push("--add".into());
                let t = r.pick(TYPES).to_string();
                out.push(t.clone());
                out.push(name(r));
                if t == "bracket" {
                    for _ in 0..r.below(4) {
                        out.push(selector(r));
                    }
                } else {
                    out.push(r.pick(POS).to_string());
                    if t == "graph" {
                        // Huge widths abort the process (finding: unbounded allocation).
                        out.push(r.pick(&["0", "1", "50", "abc", "300"]).into());
                    } else if r.chance(50) {
                        out.push(value(r));
                    }
                }
            }
            3..=6 => {
                out.push("--set".into());
                out.push(selector(r));
                for _ in 0..r.below(5) {
                    let k = r.pick(KEYS);
                    let v = value(r);
                    out.push(kv(k, v));
                }
            }
            7 => {
                out.push("--bar".into());
                for _ in 0..r.below(4) {
                    let k = r.pick(BAR_KEYS);
                    let v = value(r);
                    out.push(kv(k, v));
                }
            }
            8 => {
                out.push("--default".into());
                for _ in 0..r.below(4) {
                    let k = r.pick(KEYS);
                    let v = value(r);
                    out.push(kv(k, v));
                }
            }
            9 => {
                out.push("--animate".into());
                out.push(r.pick(CURVES).into());
                out.push(
                    r.pick(&["0", "1", "10", "30", "0xffffffff", "-1", "1000000", ""])
                        .into(),
                );
            }
            10 => {
                out.push("--push".into());
                out.push(name(r));
                for _ in 0..r.below(6) {
                    out.push(value(r));
                }
            }
            11 => {
                out.push("--clone".into());
                out.push(name(r));
                out.push(name(r));
                if r.chance(50) {
                    out.push(r.pick(&["before", "after", "x"]).into());
                }
            }
            12 => {
                out.push("--subscribe".into());
                out.push(name(r));
                for _ in 0..r.below(3) {
                    out.push(r.pick(EVENTS).into());
                }
            }
            13 => {
                out.push("--trigger".into());
                out.push(r.pick(EVENTS).into());
                for _ in 0..r.below(3) {
                    out.push(format!("K={}", value(r)));
                }
            }
            14 => {
                out.push("--query".into());
                let w = r.pick(&[
                    "bar",
                    "defaults",
                    "events",
                    "displays",
                    "default_menu_items",
                    "item",
                    "stats",
                    "menus",
                    "a",
                    "🙂",
                    "",
                ]);
                out.push(w.into());
                if w == "item" {
                    out.push(name(r));
                }
            }
            15 => {
                out.push("--reorder".into());
                for _ in 0..r.below(4) {
                    out.push(name(r));
                }
            }
            16 => {
                out.push("--move".into());
                out.push(name(r));
                out.push(r.pick(&["before", "after"]).into());
                out.push(name(r));
            }
            17 => {
                out.push("--remove".into());
                out.push(selector(r));
            }
            18 => {
                out.push("--rename".into());
                out.push(name(r));
                out.push(name(r));
            }
            19 => out.push("--update".into()),
            20 => {
                out.push(
                    r.pick(&[
                        "--menu",
                        "--menubar",
                        "--monitor",
                        "--hotload",
                        "--bogus",
                        "-",
                        "",
                        "--add",
                    ])
                    .into(),
                );
                if r.chance(50) {
                    out.push(value(r));
                }
            }
            _ => {
                out.push("--add".into());
                out.push("event".into());
                out.push(r.pick(EVENTS).into());
            }
        }
    }
    out
}

fn run_seed(seed: u64, steps: usize) {
    let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let mut res = HeadlessResources::default();
    res.files.insert(
        "/nonexist.png".into(),
        Ok(mbar_core::geometry::Size::new(10.0, 10.0)),
    );
    res.apps.push("Safari".into());
    let mut rt = Runtime::new(RuntimeConfig {
        bar_name: "mbar".into(),
        home: "/home/u".into(),
        config_path: None,
    });
    rt.begin(&mut res);
    let _ = rt.frame(res.now, &mut res);
    let setup: &[&[&str]] = &[
        &[
            "--add", "item", "a", "left", "--add", "item", "b", "right", "--add", "item", "c",
            "center",
        ],
        &[
            "--add", "graph", "g", "q", "50", "--add", "slider", "s", "e", "100",
        ],
        &[
            "--add", "item", "p1", "popup.a", "--add", "item", "p2", "popup.p1", "--add", "space",
            "x.y", "left",
        ],
        &["--add", "bracket", "br", "a", "b", "/g/"],
        &[
            "--set",
            "a",
            "popup.drawing=on",
            "label=🙂🙂🙂🙂",
            "label.max_chars=2",
            "scroll_texts=on",
            "background.drawing=on",
        ],
    ];
    if seed % 2 == 0 {
        for (i, m) in setup.iter().enumerate() {
            rt.handle(
                Input::Message {
                    args: m.iter().map(|s| s.to_string()).collect(),
                    reply: ReplyToken(1_000_000 + i as u64),
                },
                &mut res,
            );
        }
    }
    if seed % 3 == 0 {
        let pos = ["left", "right", "bottom"][(seed / 3 % 3) as usize];
        rt.handle(
            Input::Message {
                args: vec!["--bar".into(), format!("position={pos}")],
                reply: ReplyToken(2_000_000),
            },
            &mut res,
        );
    }
    for step in 0..steps {
        let input = match r.below(10) {
            0 | 9 => Input::Mouse(MouseInput {
                kind: match r.below(5) {
                    0 => MouseKind::Up {
                        button: MouseButton::Left,
                        button_code: 0,
                    },
                    1 => MouseKind::Moved,
                    2 => MouseKind::Scrolled {
                        delta: [i32::MIN, i32::MAX, -1, 1, 0][r.below(5)],
                    },
                    3 => MouseKind::Entered,
                    _ => MouseKind::Exited,
                },
                point: Point::new((r.below(2000) as f32) - 40.0, r.below(60) as f32),
                window: if r.chance(80) {
                    Some(WindowKey::Bar(1))
                } else {
                    None
                },
                modifiers: 0,
            }),
            1 => Input::Timer,
            2 => Input::Event(match r.below(5) {
                0 => OsEvent::SpaceChanged,
                1 => OsEvent::VolumeChanged(f32::NAN),
                2 => OsEvent::FrontAppSwitched {
                    name: Some("🙂".into()),
                    bundle_id: None,
                },
                3 => OsEvent::SystemWoke,
                _ => OsEvent::ConfigChanged,
            }),
            _ => Input::Message {
                args: gen_message(&mut r),
                reply: ReplyToken(step as u64),
            },
        };
        let desc = format!("{input:?}");
        let t0 = std::time::Instant::now();
        let out = catch_unwind(AssertUnwindSafe(|| {
            rt.handle(input, &mut res);
            res.now += Duration::from_millis(r.below(400) as u64);
            rt.frame(res.now, &mut res);
        }));
        if t0.elapsed() > Duration::from_millis(500) {
            eprintln!(
                "SLOW seed {seed} step {step} {:?}: {}",
                t0.elapsed(),
                &desc[..desc.len().min(400)]
            );
        }
        if out.is_err() {
            panic!("seed {seed} step {step}: panic on {desc}");
        }
    }
}

#[test]
fn fuzz_runtime_no_panic() {
    let seeds: u64 = std::env::var("FUZZ_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    let mut failures = Vec::new();
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        if !loc.contains("zz_fuzz_robust") {
            eprintln!("PANIC_AT {loc}: {}", &msg[..msg.len().min(200)]);
        }
    }));
    for seed in 0..seeds {
        if let Err(e) = catch_unwind(|| run_seed(seed, 200)) {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "?".into());
            failures.push(msg);
        }
    }
    for f in &failures {
        eprintln!("FAIL: {}", &f[..f.len().min(600)]);
    }
    assert!(failures.is_empty(), "{} failing seeds", failures.len());
}
