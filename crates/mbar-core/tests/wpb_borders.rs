//! Window borders (JankyBorders take-over): `--borders` parsing, `Runtime` effects,
//! responses, `--query borders` and `--reload` (`docs/spec/borders.md` §2.3, §3.5;
//! `docs/superpowers/specs/2026-10-09-borders-design.md` §1).

mod wpc_common;
use mbar_core::borders::{
    BorderColor, BorderOrder, BorderStyle, BordersUpdate, GradientDirection, UpdateMask,
};
use mbar_core::command::{parse, Command, QueryTarget};
use mbar_core::platform::{Effect, Input, OsEvent, PlatformRequest};
use wpc_common::*;

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The `SetBorders` requests among `fx`, in order.
fn borders_updates(fx: &[Effect]) -> Vec<BordersUpdate> {
    platform(fx)
        .into_iter()
        .filter_map(|r| match r {
            PlatformRequest::SetBorders(u) => Some(*u),
            _ => None,
        })
        .collect()
}

/// Sends a message and returns (reply, SetBorders requests).
fn send(h: &mut H, args: &[&str]) -> (String, Vec<BordersUpdate>) {
    let (text, fx) = h.msg_fx(args);
    (text.unwrap_or_default(), borders_updates(&fx))
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[test]
fn borders_domain_is_a_pair_list() {
    assert_eq!(
        parse(&argv(&[
            "--borders",
            "active_color=gradient(top_left=0xff111111,bottom_right=0xff222222)",
            "width=5.0",
            "apply-to=12",
        ])),
        vec![Command::Borders {
            pairs: pairs(&[
                (
                    "active_color",
                    "gradient(top_left=0xff111111,bottom_right=0xff222222)"
                ),
                ("width", "5.0"),
                ("apply-to", "12"),
            ]),
            malformed: None,
        }]
    );
    // Stops at the next `-` token, which is the next command.
    assert_eq!(
        parse(&argv(&["--borders", "width=2", "--query", "borders"])),
        vec![
            Command::Borders {
                pairs: pairs(&[("width", "2")]),
                malformed: None,
            },
            Command::Query(QueryTarget::Borders),
        ]
    );
    // A token without `=` ends the list and is reported; the next token is a command.
    assert_eq!(
        parse(&argv(&[
            "--borders",
            "width=2",
            "off",
            "--bar",
            "height=30"
        ])),
        vec![
            Command::Borders {
                pairs: pairs(&[("width", "2")]),
                malformed: Some("off".into()),
            },
            Command::Bar {
                pairs: pairs(&[("height", "30")]),
                malformed: None,
            },
        ]
    );
    // An empty argv element ends the message.
    assert_eq!(
        parse(&argv(&["--borders", "width=2", "", "style=square"])),
        vec![Command::Borders {
            pairs: pairs(&[("width", "2")]),
            malformed: None,
        }]
    );
    assert_eq!(
        parse(&argv(&["--borders"])),
        vec![Command::Borders {
            pairs: vec![],
            malformed: None,
        }]
    );
    assert_eq!(
        parse(&argv(&["--query", "borders"])),
        vec![Command::Query(QueryTarget::Borders)]
    );
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

#[test]
fn borders_are_off_until_configured() {
    let mut h = H::new();
    assert!(borders_updates(&h.effects).is_empty());
    assert!(!h.rt.model.borders.drawing);
    assert!(!h.rt.model.borders.configured);
    let q = h.query(&["borders"]);
    assert_eq!(q["drawing"], "off");
}

#[test]
fn one_request_per_changed_message() {
    let mut h = H::new();
    let (rsp, ups) = send(
        &mut h,
        &[
            "--borders",
            "active_color=0xffe1e3e4",
            "inactive_color=0xff494d64",
            "width=5.0",
        ],
    );
    assert_eq!(rsp, "");
    assert_eq!(ups.len(), 1, "exactly one request per message");
    let u = &ups[0];
    assert!(u.drawing, "the first message turns borders on");
    assert_eq!(u.settings.inactive, BorderColor::Solid(0xff494d64));
    assert_eq!(u.settings.width, 5.0);
    assert!(u.mask.contains(UpdateMask::ALL));
    assert!(u.overrides.is_empty());

    // Unchanged: no request.
    let (rsp, ups) = send(
        &mut h,
        &["--borders", "width=5", "inactive_color=0xff494d64"],
    );
    assert_eq!(rsp, "");
    assert!(ups.is_empty());

    // Two --borders commands in one message: one request each.
    let (_, ups) = send(
        &mut h,
        &["--borders", "style=square", "--borders", "order=above"],
    );
    assert_eq!(ups.len(), 2);
    assert_eq!(ups[0].settings.style, BorderStyle::Square);
    assert_eq!(ups[1].settings.order, BorderOrder::Above);
    assert_eq!(ups[1].mask, UpdateMask(UpdateMask::ALL));

    // Masks follow spec §2.3.
    let (_, ups) = send(&mut h, &["--borders", "active_color=glow(0xff00ff00)"]);
    assert_eq!(ups[0].mask, UpdateMask(UpdateMask::ACTIVE));
    let (_, ups) = send(&mut h, &["--borders", "hidpi=on"]);
    assert_eq!(ups[0].mask, UpdateMask(UpdateMask::RECREATE_ALL));
    let (_, ups) = send(&mut h, &["--borders", "ax_focus=off"]);
    assert_eq!(ups[0].mask, UpdateMask(UpdateMask::SETTING));
    assert_eq!(ups[0].settings.ax_focus, Some(false));
}

#[test]
fn drawing_toggles() {
    let mut h = H::new();
    // A first message with drawing=off configures without drawing: nothing to send.
    let (_, ups) = send(&mut h, &["--borders", "drawing=off"]);
    assert!(ups.is_empty());
    assert!(h.rt.model.borders.configured);
    // Settings changes are still sent (the platform keeps them for later).
    let (_, ups) = send(&mut h, &["--borders", "width=8"]);
    assert_eq!(ups.len(), 1);
    assert!(!ups[0].drawing);
    // Turning on: RECREATE_ALL so the platform creates every border.
    let (_, ups) = send(&mut h, &["--borders", "drawing=on"]);
    assert_eq!(ups.len(), 1);
    assert!(ups[0].drawing);
    assert_eq!(ups[0].settings.width, 8.0);
    assert_eq!(ups[0].mask, UpdateMask(UpdateMask::RECREATE_ALL));
    // Already on: nothing.
    assert!(send(&mut h, &["--borders", "drawing=yes"]).1.is_empty());
    // mbar booleans, including toggle.
    let (_, ups) = send(&mut h, &["--borders", "drawing=toggle"]);
    assert!(!ups[0].drawing);
    let (_, ups) = send(&mut h, &["--borders", "drawing=!off"]);
    assert!(ups[0].drawing);
    assert_eq!(h.query(&["borders"])["drawing"], "on");
}

#[test]
fn errors_go_to_the_response() {
    let mut h = H::new();
    let (rsp, ups) = send(
        &mut h,
        &[
            "--borders",
            "active_color=red",
            "blur_radius=5",
            "width=3",
            "active_colorful=1",
        ],
    );
    assert_eq!(
        rsp,
        "[!] Borders: Invalid color argument color=red\n\
         [!] Borders: Invalid argument 'blur_radius=5'\n\
         [!] Borders: Invalid color argument colorful=1\n"
    );
    // The valid key still applied.
    assert_eq!(ups.len(), 1);
    assert_eq!(ups[0].settings.width, 3.0);

    // Malformed token: reported, the pairs before it apply.
    let (rsp, ups) = send(&mut h, &["--borders", "width=4", "square"]);
    assert_eq!(
        rsp,
        "[!] Borders: Expected <key>=<value> pair, but got: 'square'\n"
    );
    assert_eq!(ups.len(), 1);
    assert_eq!(ups[0].settings.width, 4.0);

    // Errors are logged like other responses.
    let (_, fx) = h.msg_fx(&["--borders", "nope=1"]);
    assert!(fx.contains(&Effect::Log(
        "[!] Borders: Invalid argument 'nope=1'\n".into()
    )));
    assert!(borders_updates(&fx).is_empty());
}

#[test]
fn malformed_only_message_does_not_turn_borders_on() {
    let mut h = H::new();
    let (rsp, ups) = send(&mut h, &["--borders", "on"]);
    assert_eq!(
        rsp,
        "[!] Borders: Expected <key>=<value> pair, but got: 'on'\n"
    );
    assert!(ups.is_empty());
    assert!(!h.rt.model.borders.configured);
    // A bare `--borders` is a (first) message: defaults, drawing on.
    let (rsp, ups) = send(&mut h, &["--borders"]);
    assert_eq!(rsp, "");
    assert_eq!(ups.len(), 1);
    assert!(ups[0].drawing);
}

#[test]
fn apply_to_and_overrides() {
    let mut h = H::new();
    send(&mut h, &["--borders", "active_color=0xff111111", "width=5"]);
    let (rsp, ups) = send(
        &mut h,
        &[
            "--borders",
            "apply-to=4711",
            "active_color=gradient(top_right=0xff00ff00,bottom_left=0xff0000ff)",
            "blacklist=Safari",
        ],
    );
    assert_eq!(rsp, "");
    assert_eq!(ups.len(), 1);
    let u = &ups[0];
    assert_eq!(u.settings.active, BorderColor::Solid(0xff111111));
    assert!(u.settings.blacklist.is_empty(), "globals untouched");
    assert_eq!(u.overrides.len(), 1);
    assert_eq!(u.overrides[0].0, 4711);
    assert_eq!(
        u.overrides[0].1.active,
        BorderColor::Gradient {
            direction: GradientDirection::TopRightToBottomLeft,
            color1: 0xff00ff00,
            color2: 0xff0000ff,
        }
    );
    assert_eq!(u.overrides[0].1.width, 5.0, "created from the globals");
    assert!(
        !u.mask.intersects(UpdateMask::RECREATE_ALL),
        "lists in an override recreate nothing (BR-IPC-09)"
    );

    // A global message reaches the override too (BR-IPC-08).
    let (_, ups) = send(&mut h, &["--borders", "width=7"]);
    assert_eq!(ups[0].settings.width, 7.0);
    assert_eq!(ups[0].overrides[0].1.width, 7.0);
    assert!(matches!(
        ups[0].overrides[0].1.active,
        BorderColor::Gradient { .. }
    ));

    // apply-to=0 is the global path.
    let (_, ups) = send(&mut h, &["--borders", "apply-to=0", "style=uniform"]);
    assert_eq!(ups[0].settings.style, BorderStyle::Uniform);
    assert_eq!(ups[0].overrides[0].1.style, BorderStyle::Uniform);

    // Query lists the override.
    let q = h.query(&["borders"]);
    assert_eq!(q["overrides"][0]["window"], 4711);
    assert_eq!(
        q["overrides"][0]["active_color"],
        "gradient(top_right=0xff00ff00,bottom_left=0xff0000ff)"
    );
    assert_eq!(q["overrides"][0]["blacklist"][0], "Safari");

    // RECREATE_ALL (global whitelist) drops every override.
    let (_, ups) = send(&mut h, &["--borders", "whitelist=kitty,Code"]);
    assert!(ups[0].overrides.is_empty());
    assert!(ups[0].mask.contains(UpdateMask::RECREATE_ALL));
    assert_eq!(h.query(&["borders"])["overrides"], serde_json::json!([]));
}

// ---------------------------------------------------------------------------
// --query borders
// ---------------------------------------------------------------------------

#[test]
fn query_text_is_exact() {
    let mut h = H::new();
    send(
        &mut h,
        &[
            "--borders",
            "active_color=0xffe1e3e4",
            "inactive_color=glow(0x80494d64)",
            "background_color=gradient(top_left=0xff000001,bottom_right=0xff000002)",
            "width=5.5",
            "style=square",
            "order=above",
            "hidpi=on",
            "ax_focus=on",
            "blacklist=Safari, kitty",
            "whitelist=",
        ],
    );
    assert_eq!(
        h.msg(&["--query", "borders"]),
        "{\n\
         \t\"drawing\": \"on\",\n\
         \t\"active_color\": \"0xffe1e3e4\",\n\
         \t\"inactive_color\": \"glow(0x80494d64)\",\n\
         \t\"background_color\": \"gradient(top_left=0xff000001,bottom_right=0xff000002)\",\n\
         \t\"width\": 5.500000,\n\
         \t\"style\": \"square\",\n\
         \t\"order\": \"above\",\n\
         \t\"hidpi\": \"on\",\n\
         \t\"ax_focus\": \"on\",\n\
         \t\"blacklist\": [\"Safari\", \" kitty\"],\n\
         \t\"whitelist\": [],\n\
         \t\"overrides\": []\n\
         }\n"
    );
    send(&mut h, &["--borders", "apply-to=9", "width=1"]);
    let text = h.msg(&["--query", "borders"]);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["overrides"][0]["window"], 9);
    assert_eq!(v["overrides"][0]["width"], 1.0);
    assert_eq!(v["overrides"][0]["style"], "square");
    assert!(v["overrides"][0].get("drawing").is_none());
}

#[test]
fn item_named_borders_wins() {
    let mut h = H::new();
    send(&mut h, &["--borders", "width=6"]);
    assert_eq!(h.query(&["borders"])["width"], 6.0);
    h.msg(&["--add", "item", "borders", "left"]);
    let q = h.query(&["borders"]);
    assert_eq!(q["name"], "borders");
    assert!(q.get("active_color").is_none());
    h.msg(&["--remove", "borders"]);
    assert_eq!(h.query(&["borders"])["width"], 6.0);
}

// ---------------------------------------------------------------------------
// --reload
// ---------------------------------------------------------------------------

#[test]
fn reload_resets_and_turns_borders_off() {
    let mut h = H::new();
    send(&mut h, &["--borders", "width=9", "apply-to=3"]);
    send(&mut h, &["--borders", "style=square"]);
    let (_, fx) = h.msg_fx(&["--reload"]);
    let ups = borders_updates(&fx);
    assert_eq!(ups.len(), 1);
    assert!(!ups[0].drawing);
    assert_eq!(ups[0].mask, UpdateMask(UpdateMask::RECREATE_ALL));
    assert!(ups[0].overrides.is_empty());
    // Sent before the config runs again.
    let off = fx
        .iter()
        .position(|e| matches!(e, Effect::Platform(PlatformRequest::SetBorders(_))))
        .unwrap();
    let run = fx
        .iter()
        .position(|e| matches!(e, Effect::RunConfig { .. }))
        .unwrap();
    assert!(off < run);
    // The configuration is back to the defaults, not configured.
    let b = &h.rt.model.borders;
    assert!(!b.drawing && !b.configured);
    assert!(b.overrides.is_empty());
    assert_eq!(b.settings.style, BorderStyle::Round);
    // The config's first --borders message turns them on again.
    let (_, ups) = send(&mut h, &["--borders", "width=2"]);
    assert!(ups[0].drawing);

    // Not drawing before the reload: no request.
    send(&mut h, &["--borders", "drawing=off"]);
    let (_, fx) = h.msg_fx(&["--reload"]);
    assert!(borders_updates(&fx).is_empty());
    assert!(fx.contains(&Effect::RunConfig { path: None }));
}

#[test]
fn hotload_reload_turns_borders_off_too() {
    let mut h = H::new();
    h.msg(&["--hotload", "on"]);
    send(&mut h, &["--borders", "width=9"]);
    let fx = h.input(Input::Event(OsEvent::ConfigChanged));
    let ups = borders_updates(&fx);
    assert_eq!(ups.len(), 1);
    assert!(!ups[0].drawing);
    assert!(!h.rt.model.borders.configured);
}
