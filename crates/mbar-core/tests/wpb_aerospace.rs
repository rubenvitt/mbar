//! AeroSpace integration in the core runtime: built-in `aerospace_*` events, lazy
//! `StartAerospace`, `provider=aerospace`, `--query aerospace` and `--reload`
//! (`docs/superpowers/specs/2026-10-09-aerospace-design.md` §Events, §Provider,
//! §`--query aerospace`, §Core).

mod wpc_common;
use mbar_core::aerospace::{AerospaceEvent, AerospaceStatus, AerospaceTransport};
use mbar_core::command::{parse, Command, QueryTarget};
use mbar_core::platform::{Effect, Input, LuaRequest, PlatformRequest};
use wpc_common::*;

fn ws(workspace: &str, prev: &str) -> Input {
    Input::Aerospace(AerospaceEvent::WorkspaceChanged {
        workspace: workspace.into(),
        prev_workspace: prev.into(),
    })
}

fn mode(m: &str) -> Input {
    Input::Aerospace(AerospaceEvent::ModeChanged {
        mode: Some(m.into()),
    })
}

fn monitor(workspace: &str, id: i64) -> Input {
    Input::Aerospace(AerospaceEvent::MonitorChanged {
        workspace: workspace.into(),
        monitor_id: id,
    })
}

/// Number of `StartAerospace` requests among `fx`.
fn starts(fx: &[Effect]) -> usize {
    platform(fx)
        .iter()
        .filter(|p| matches!(p, PlatformRequest::StartAerospace))
        .count()
}

/// Feeds an input and runs a frame.
fn feed(h: &mut H, input: Input) -> Vec<Effect> {
    let fx = h.input(input);
    h.frame();
    fx
}

fn label(h: &mut H, item: &str) -> String {
    h.query(&[item])["label"]["value"]
        .as_str()
        .unwrap()
        .to_string()
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[test]
fn subscription_without_add_event_receives_the_event() {
    let mut h = H::new();
    let rsp = h.msg(&[
        "--add",
        "item",
        "space.1",
        "left",
        "--set",
        "space.1",
        "script=ws.sh 1",
        "--subscribe",
        "space.1",
        "aerospace_workspace_change",
    ]);
    assert_eq!(rsp, "", "no '[?] Event: ... not found'");
    // Registered on first use like a custom event (first free bit).
    assert_eq!(
        h.query(&["events"])["aerospace_workspace_change"]["bit"],
        1u64 << 18
    );

    let fx = feed(&mut h, ws("2", "1"));
    let r = runs_of(&fx, "space.1");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].script, "ws.sh 1");
    assert_eq!(r[0].sender(), Some("aerospace_workspace_change"));
    assert_eq!(r[0].get("NAME"), Some("space.1"));
    assert_eq!(r[0].get("FOCUSED_WORKSPACE"), Some("2"));
    assert_eq!(r[0].get("PREV_WORKSPACE"), Some("1"));
    assert_eq!(
        r[0].get("INFO"),
        Some(r#"{"focused_workspace":"2","prev_workspace":"1"}"#)
    );

    // Other AeroSpace events do not reach this item.
    let fx = feed(&mut h, mode("service"));
    assert!(runs(&fx).is_empty());
}

/// (input, event name, expected variables).
type Case = (Input, &'static str, Vec<(&'static str, &'static str)>);

#[test]
fn every_event_type_is_delivered_to_its_subscribers() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "script=a.sh",
        "--subscribe",
        "a",
        "aerospace_focus_change",
        "aerospace_monitor_change",
        "aerospace_mode_change",
        "aerospace_window_detected",
        "aerospace_binding_triggered",
    ]);
    let cases: Vec<Case> = vec![
        (
            Input::Aerospace(AerospaceEvent::FocusChanged {
                window_id: Some(42),
                workspace: "3".into(),
            }),
            "aerospace_focus_change",
            vec![("FOCUSED_WORKSPACE", "3"), ("WINDOW_ID", "42")],
        ),
        (
            monitor("4", 2),
            "aerospace_monitor_change",
            vec![("FOCUSED_WORKSPACE", "4"), ("MONITOR_ID", "2")],
        ),
        (
            mode("resize"),
            "aerospace_mode_change",
            vec![("MODE", "resize")],
        ),
        (
            Input::Aerospace(AerospaceEvent::WindowDetected {
                window_id: 7,
                workspace: Some("1".into()),
                app_bundle_id: Some("com.x".into()),
                app_name: Some("X".into()),
            }),
            "aerospace_window_detected",
            vec![
                ("WINDOW_ID", "7"),
                ("WORKSPACE", "1"),
                ("APP_BUNDLE_ID", "com.x"),
                ("APP_NAME", "X"),
            ],
        ),
        (
            Input::Aerospace(AerospaceEvent::BindingTriggered {
                mode: "main".into(),
                binding: "alt-1".into(),
            }),
            "aerospace_binding_triggered",
            vec![("MODE", "main"), ("BINDING", "alt-1")],
        ),
    ];
    for (input, name, vars) in cases {
        let expected_info = match &input {
            Input::Aerospace(ev) => ev.info_json(),
            _ => unreachable!(),
        };
        let fx = feed(&mut h, input);
        let r = runs_of(&fx, "a");
        assert_eq!(r.len(), 1, "{name}");
        assert_eq!(r[0].sender(), Some(name));
        assert_eq!(r[0].get("INFO"), Some(expected_info.as_str()), "{name}");
        for (k, v) in vars {
            assert_eq!(r[0].get(k), Some(v), "{name} {k}");
        }
    }
}

#[test]
fn lua_handlers_get_the_event_in_process() {
    let mut h = H::new();
    h.msg(&["--add", "item", "l", "left"]);
    let fx = h.input(Input::Lua(LuaRequest::Subscribe {
        item: "l".into(),
        events: vec!["aerospace_workspace_change".into()],
        handler: 11,
    }));
    assert_eq!(starts(&fx), 1, "a Lua subscription starts the connection");
    let fx = feed(&mut h, ws("5", "4"));
    let r = runs(&fx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].script, "lua:11");
    assert_eq!(r[0].sender(), Some("aerospace_workspace_change"));
    assert_eq!(r[0].get("FOCUSED_WORKSPACE"), Some("5"));
    assert_eq!(
        r[0].get("INFO"),
        Some(r#"{"focused_workspace":"5","prev_workspace":"4"}"#)
    );
}

#[test]
fn events_without_subscribers_only_update_state() {
    let mut h = H::new();
    let fx = feed(&mut h, ws("2", "1"));
    assert!(runs(&fx).is_empty());
    assert_eq!(h.rt.model.aerospace.focused_workspace, "2");
    // Not registered by the event itself.
    assert!(h
        .query(&["events"])
        .get("aerospace_workspace_change")
        .is_none());
}

#[test]
fn add_event_is_accepted_and_manual_trigger_keeps_working() {
    let mut h = H::new();
    // SketchyBar recipe: `--add event` first, with or without a notification name.
    let (rsp, fx) = h.msg_fx(&["--add", "event", "aerospace_workspace_change"]);
    assert_eq!(rsp.as_deref(), Some(""));
    assert!(platform(&fx).is_empty(), "no notification, no start");
    let (rsp, fx) = h.msg_fx(&[
        "--add",
        "event",
        "aerospace_mode_change",
        "com.example.mode",
    ]);
    assert_eq!(rsp.as_deref(), Some(""));
    assert!(
        platform(&fx).is_empty(),
        "the notification of a built-in AeroSpace event is not observed"
    );
    let ev = h.query(&["events"]);
    assert_eq!(ev["aerospace_workspace_change"]["bit"], 1u64 << 18);
    assert_eq!(ev["aerospace_mode_change"]["notification"], "(null)");
    // Adding it again changes nothing.
    assert_eq!(h.msg(&["--add", "event", "aerospace_workspace_change"]), "");
    assert_eq!(
        h.query(&["events"])["aerospace_workspace_change"]["bit"],
        1u64 << 18
    );

    for sid in ["1", "2"] {
        let name = format!("space.{sid}");
        let script = format!("aerospace.sh {sid}");
        h.msg(&[
            "--add",
            "item",
            &name,
            "left",
            "--subscribe",
            &name,
            "aerospace_workspace_change",
            "--set",
            &name,
            &format!("script={script}"),
        ]);
    }
    assert_eq!(
        h.query(&["space.1"])["scripting"]["update_mask"],
        1u64 << 18
    );

    // Manual trigger: exactly the custom-event path (only the passed variables).
    let (_, fx) = h.msg_fx(&[
        "--trigger",
        "aerospace_workspace_change",
        "FOCUSED_WORKSPACE=2",
    ]);
    let r = runs(&fx);
    assert_eq!(r.len(), 2);
    for run in &r {
        assert_eq!(run.sender(), Some("aerospace_workspace_change"));
        assert_eq!(run.get("FOCUSED_WORKSPACE"), Some("2"));
        assert_eq!(run.get("PREV_WORKSPACE"), None);
        assert_eq!(run.get("INFO"), None);
    }
    // ... and the event from AeroSpace reaches the same subscribers.
    let fx = feed(&mut h, ws("1", "2"));
    assert_eq!(runs(&fx).len(), 2);
}

// ---------------------------------------------------------------------------
// Lazy start
// ---------------------------------------------------------------------------

#[test]
fn start_is_requested_once_per_runtime() {
    let mut h = H::new();
    assert_eq!(starts(&h.effects), 0, "nothing at startup");
    h.msg(&["--add", "item", "a", "left", "--add", "item", "b", "left"]);
    h.msg(&["--add", "event", "aerospace_workspace_change"]);
    h.msg(&[
        "--trigger",
        "aerospace_workspace_change",
        "FOCUSED_WORKSPACE=1",
    ]);
    h.msg(&["--subscribe", "a", "front_app_switched"]);
    assert_eq!(
        starts(&h.effects),
        0,
        "--add event / --trigger do not connect"
    );

    let (_, fx) = h.msg_fx(&["--subscribe", "a", "aerospace_workspace_change"]);
    assert_eq!(starts(&fx), 1);
    let (_, fx) = h.msg_fx(&["--subscribe", "b", "aerospace_mode_change"]);
    assert_eq!(starts(&fx), 0);
    let (_, fx) = h.msg_fx(&["--set", "b", "provider=aerospace"]);
    assert_eq!(starts(&fx), 0);
    let (_, fx) = h.msg_fx(&["--query", "aerospace"]);
    assert_eq!(starts(&fx), 0);
    assert!(h.rt.request_aerospace().is_none());

    let (_, fx) = h.msg_fx(&["--reload"]);
    assert!(fx.iter().any(|e| matches!(e, Effect::RunConfig { .. })));
    h.msg(&[
        "--add",
        "item",
        "a",
        "left",
        "--subscribe",
        "a",
        "aerospace_workspace_change",
        "--set",
        "a",
        "provider=aerospace",
        "--query",
        "aerospace",
    ]);
    assert_eq!(
        starts(&h.effects),
        1,
        "the connection survives --reload: never requested again"
    );
    assert!(h.rt.aerospace_started());
}

#[test]
fn each_trigger_starts_the_connection() {
    // provider=aerospace
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "w",
        "left",
        "--set",
        "w",
        "provider=aerospace",
    ]);
    assert_eq!(starts(&fx), 1);
    assert!(
        !platform(&fx)
            .iter()
            .any(|p| matches!(p, PlatformRequest::StartProvider { .. })),
        "core provider: no platform provider"
    );

    // --query aerospace only reports (mbar.app polls it): no connection, `active` off.
    let mut h = H::new();
    let (text, fx) = h.msg_fx(&["--query", "aerospace"]);
    assert_eq!(starts(&fx), 0);
    assert!(text.unwrap().contains("\"active\": \"off\""));
    assert!(!h.rt.aerospace_started());

    // An item named `aerospace` wins and does not connect.
    let mut h = H::new();
    h.msg(&["--add", "item", "aerospace", "left"]);
    let (text, fx) = h.msg_fx(&["--query", "aerospace"]);
    assert_eq!(starts(&fx), 0);
    assert!(text.unwrap().starts_with("{\n\t\"name\": \"aerospace\""));

    // Lua command path (`mbar.subscribe` compiles to argv).
    let mut h = H::new();
    h.msg(&["--add", "item", "x", "left"]);
    let fx = h.input(Input::Lua(LuaRequest::Command {
        args: vec![
            "--subscribe".into(),
            "x".into(),
            "aerospace_focus_change".into(),
        ],
        callback: None,
    }));
    assert_eq!(starts(&fx), 1);

    // The binary / Lua `mbar.aerospace` path.
    let mut h = H::new();
    assert_eq!(
        h.rt.request_aerospace(),
        Some(Effect::Platform(PlatformRequest::StartAerospace))
    );
    assert_eq!(h.rt.request_aerospace(), None);
    assert_eq!(h.query(&["aerospace"])["active"], "on");

    // An item-less Lua handler (`mbar.aerospace.on`).
    let mut h = H::new();
    let fx = h.input(Input::Lua(LuaRequest::On {
        events: vec!["aerospace_mode_change".into()],
        handler: 3,
    }));
    assert_eq!(starts(&fx), 1);
    // A global handler for another event does not connect.
    let mut h = H::new();
    let fx = h.input(Input::Lua(LuaRequest::On {
        events: vec!["front_app_switched".into()],
        handler: 3,
    }));
    assert_eq!(starts(&fx), 0);
}

// ---------------------------------------------------------------------------
// provider=aerospace
// ---------------------------------------------------------------------------

#[test]
fn provider_labels_follow_the_state() {
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "ws",
        "left",
        "--set",
        "ws",
        "label=none",
        "provider=aerospace",
        "script=ws.sh",
        "--add",
        "item",
        "mode",
        "left",
        "--set",
        "mode",
        "provider=aerospace",
        "provider.args=mode",
        "--add",
        "item",
        "mon",
        "left",
        "--set",
        "mon",
        "provider=aerospace",
        "provider.args=monitor",
        "provider.format=M{value} on {workspace}",
    ]);
    // Nothing known yet: labels untouched, no provider run.
    assert!(runs(&fx).is_empty());
    assert_eq!(label(&mut h, "ws"), "none");

    let fx = feed(&mut h, ws("2", "1"));
    assert_eq!(label(&mut h, "ws"), "2");
    assert_eq!(label(&mut h, "mode"), "");
    assert_eq!(label(&mut h, "mon"), "M on 2");
    let r = runs_of(&fx, "ws");
    assert_eq!(
        r.len(),
        1,
        "provider run (item not subscribed to the event)"
    );
    assert_eq!(r[0].sender(), Some("provider"));
    let info: serde_json::Value = serde_json::from_str(r[0].get("INFO").unwrap()).unwrap();
    assert_eq!(info["value"], "2");
    assert_eq!(info["workspace"], "2");
    assert_eq!(info["prev_workspace"], "1");

    feed(&mut h, mode("service"));
    assert_eq!(label(&mut h, "mode"), "service");
    feed(&mut h, monitor("3", 2));
    assert_eq!(label(&mut h, "mon"), "M2 on 3");
    assert_eq!(label(&mut h, "ws"), "3");

    // Events without state (window detected, binding) do not re-run providers.
    let fx = feed(
        &mut h,
        Input::Aerospace(AerospaceEvent::BindingTriggered {
            mode: "main".into(),
            binding: "alt-1".into(),
        }),
    );
    assert!(runs(&fx).is_empty());

    // Configuring the provider when the state is known applies it right away, once per
    // command, before a later query of the same message.
    let (text, fx) = h.msg_fx(&[
        "--add",
        "item",
        "late",
        "left",
        "--set",
        "late",
        "script=late.sh",
        "provider=aerospace",
        "provider.args=mode",
        "--query",
        "late",
    ]);
    let q: serde_json::Value = serde_json::from_str(&text.unwrap()).unwrap();
    assert_eq!(q["label"]["value"], "service");
    assert_eq!(runs_of(&fx, "late").len(), 1);

    // Switching back to none: no further updates.
    h.msg(&["--set", "ws", "provider=none", "label=fixed"]);
    feed(&mut h, ws("4", "3"));
    assert_eq!(label(&mut h, "ws"), "fixed");
    assert_eq!(label(&mut h, "mon"), "M2 on 4");
}

#[test]
fn provider_is_core_driven() {
    let mut h = H::new();
    h.msg(&["--add", "item", "p", "left", "--set", "p", "provider=cpu"]);
    let p = h.rt.model.find("p").unwrap();
    let (_, fx) = h.msg_fx(&["--set", "p", "provider=aerospace"]);
    let reqs = platform(&fx);
    assert!(
        reqs.contains(&PlatformRequest::StopProvider { item: p }),
        "the previous platform provider is stopped"
    );
    assert!(!reqs
        .iter()
        .any(|r| matches!(r, PlatformRequest::StartProvider { .. })));
    feed(&mut h, ws("7", "6"));
    // A late platform sample is ignored.
    feed(
        &mut h,
        Input::ProviderSample {
            item: p,
            values: vec![("percent".into(), "42".into())],
        },
    );
    assert_eq!(label(&mut h, "p"), "7");
    assert_eq!(h.query(&["p"])["provider"]["name"], "aerospace");

    // Removing or reloading sends no StopProvider for a core provider.
    let (_, fx) = h.msg_fx(&["--remove", "p"]);
    assert!(!platform(&fx)
        .iter()
        .any(|r| matches!(r, PlatformRequest::StopProvider { .. })));
    h.msg(&[
        "--add",
        "item",
        "q",
        "left",
        "--set",
        "q",
        "provider=aerospace",
    ]);
    let (_, fx) = h.msg_fx(&["--reload"]);
    assert!(!platform(&fx)
        .iter()
        .any(|r| matches!(r, PlatformRequest::StopProvider { .. })));
}

// ---------------------------------------------------------------------------
// --query aerospace, status, reload
// ---------------------------------------------------------------------------

#[test]
fn query_parses() {
    assert_eq!(
        parse(&["--query".to_string(), "aerospace".to_string()]),
        vec![Command::Query(QueryTarget::Aerospace)]
    );
}

#[test]
fn query_json_exact() {
    let mut h = H::new();
    assert_eq!(
        h.msg(&["--query", "aerospace"]),
        "{\n\t\"connected\": \"off\",\n\t\"active\": \"off\",\n\t\"transport\": \"none\",\n\t\"server_version\": \"\",\n\t\"error\": \"\",\n\t\"focused_workspace\": \"\",\n\t\"prev_workspace\": \"\",\n\t\"mode\": \"\",\n\t\"monitor\": 0\n}\n"
    );
    feed(
        &mut h,
        Input::AerospaceStatus(AerospaceStatus {
            connected: true,
            transport: AerospaceTransport::Socket,
            server_version: Some("0.20.0-Beta 33fa0643".into()),
            error: None,
        }),
    );
    feed(&mut h, ws("2", "1"));
    feed(&mut h, mode("main"));
    feed(&mut h, monitor("2", 1));
    assert!(h.rt.request_aerospace().is_some());
    assert_eq!(
        h.msg(&["--query", "aerospace"]),
        "{\n\t\"connected\": \"on\",\n\t\"active\": \"on\",\n\t\"transport\": \"socket\",\n\t\"server_version\": \"0.20.0-Beta 33fa0643\",\n\t\"error\": \"\",\n\t\"focused_workspace\": \"2\",\n\t\"prev_workspace\": \"1\",\n\t\"mode\": \"main\",\n\t\"monitor\": 1\n}\n"
    );
}

#[test]
fn status_is_stored() {
    let mut h = H::new();
    let status = AerospaceStatus {
        connected: false,
        transport: AerospaceTransport::Cli,
        server_version: None,
        error: Some("AeroSpace is not running \"x\"".into()),
    };
    let fx = feed(&mut h, Input::AerospaceStatus(status.clone()));
    assert!(runs(&fx).is_empty());
    assert_eq!(h.rt.model.aerospace.status, status);
    let q = h.query(&["aerospace"]);
    assert_eq!(q["connected"], "off");
    assert_eq!(q["transport"], "cli");
    assert_eq!(q["error"], "AeroSpace is not running \"x\"");
}

#[test]
fn reload_keeps_state_and_status() {
    let mut h = H::new();
    assert!(h.rt.request_aerospace().is_some());
    feed(
        &mut h,
        Input::AerospaceStatus(AerospaceStatus {
            connected: true,
            transport: AerospaceTransport::Socket,
            server_version: Some("1.0".into()),
            error: None,
        }),
    );
    feed(&mut h, ws("3", "2"));
    feed(&mut h, mode("main"));
    let before = h.msg(&["--query", "aerospace"]);

    h.msg(&["--reload"]);
    assert_eq!(h.msg(&["--query", "aerospace"]), before);
    // The re-run config gets correct labels without a new event from AeroSpace.
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "ws",
        "left",
        "--set",
        "ws",
        "provider=aerospace",
    ]);
    assert_eq!(starts(&fx), 0);
    assert_eq!(label(&mut h, "ws"), "3");
    // The event registry is rebuilt; subscribing again re-registers the name.
    assert_eq!(
        h.msg(&["--subscribe", "ws", "aerospace_workspace_change"]),
        ""
    );
    assert_eq!(
        h.query(&["events"])["aerospace_workspace_change"]["bit"],
        1u64 << 18
    );
}

// ---------------------------------------------------------------------------
// Item-less handlers (`LuaRequest::On`, Lua `mbar.aerospace.on`)
// ---------------------------------------------------------------------------

fn on(h: &mut H, event: &str, handler: u64) -> Vec<Effect> {
    h.input(Input::Lua(LuaRequest::On {
        events: vec![event.into()],
        handler,
    }))
}

/// Global handlers fire for every trigger of their event with the event's variables and
/// `SENDER` (no `NAME`), whatever the default item says (`updates=when_shown` / `off`,
/// `update_freq`, `click_script`), and they never appear as an item.
#[test]
fn global_handlers_are_item_less() {
    for updates in ["when_shown", "off"] {
        let mut h = H::new();
        h.msg(&[
            "--default",
            &format!("updates={updates}"),
            "update_freq=5",
            "click_script=echo hi",
            "drawing=off",
        ]);
        on(&mut h, "aerospace_workspace_change", 7);
        on(&mut h, "aerospace_workspace_change", 8);
        on(&mut h, "aerospace_mode_change", 9);
        // Registering twice changes nothing.
        on(&mut h, "aerospace_mode_change", 9);
        let fx = feed(&mut h, ws("2", "1"));
        let r = runs(&fx);
        assert_eq!(
            r.iter().map(|r| r.script.as_str()).collect::<Vec<_>>(),
            ["lua:7", "lua:8"],
            "{updates}"
        );
        assert_eq!(r[0].sender(), Some("aerospace_workspace_change"));
        assert_eq!(r[0].get("NAME"), None);
        assert_eq!(r[0].get("FOCUSED_WORKSPACE"), Some("2"));
        assert_eq!(r[0].get("AEROSPACE_PREV_WORKSPACE"), Some("1"));
        assert_eq!(
            r[0].get("INFO"),
            Some(r#"{"focused_workspace":"2","prev_workspace":"1"}"#)
        );
        let fx = feed(&mut h, mode("service"));
        let r = runs(&fx);
        assert_eq!(r.len(), 1, "{updates}");
        assert_eq!(r[0].script, "lua:9");
        assert_eq!(r[0].get("MODE"), Some("service"));

        // No carrier item: nothing in --query bar, regex selectors do not touch them.
        assert_eq!(h.query(&["bar"])["items"], serde_json::json!([]));
        h.msg(&["--remove", "/.*/"]);
        let fx = feed(&mut h, ws("3", "2"));
        assert_eq!(runs(&fx).len(), 2, "{updates}");
    }
}

/// Global handlers also see manual `--trigger`s of their event (the same trigger path as
/// item scripts) and are dropped by `--reload` (the config registers them again).
#[test]
fn global_handlers_follow_triggers_and_reload() {
    let mut h = H::new();
    on(&mut h, "aerospace_workspace_change", 4);
    let (_, fx) = h.msg_fx(&[
        "--trigger",
        "aerospace_workspace_change",
        "FOCUSED_WORKSPACE=6",
    ]);
    let r = runs(&fx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].script, "lua:4");
    assert_eq!(r[0].get("FOCUSED_WORKSPACE"), Some("6"));
    assert_eq!(r[0].sender(), Some("aerospace_workspace_change"));

    h.msg(&["--reload"]);
    let fx = feed(&mut h, ws("2", "1"));
    assert!(runs(&fx).is_empty(), "cleared by --reload");
    on(&mut h, "aerospace_workspace_change", 5);
    let fx = feed(&mut h, ws("3", "2"));
    assert_eq!(runs(&fx)[0].script, "lua:5");
}

// ---------------------------------------------------------------------------
// Initial state for late subscribers
// ---------------------------------------------------------------------------

/// The SketchyBar shell loop (`for sid …; --subscribe space.$sid
/// aerospace_workspace_change`) runs after AeroSpace sent its initial state: each new
/// subscriber gets the stored state once, nobody else does.
#[test]
fn late_item_subscribers_get_the_stored_state() {
    let mut h = H::new();
    h.msg(&["--default", "updates=when_shown"]);
    // Nothing known yet: a subscription delivers nothing.
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "space.0",
        "left",
        "--set",
        "space.0",
        "script=ws.sh 0",
        "--subscribe",
        "space.0",
        "aerospace_workspace_change",
    ]);
    assert!(runs(&fx).is_empty());
    feed(&mut h, ws("2", "1"));
    feed(&mut h, mode("main"));

    for sid in ["1", "2"] {
        let name = format!("space.{sid}");
        let (_, fx) = h.msg_fx(&[
            "--add",
            "item",
            &name,
            "left",
            "--subscribe",
            &name,
            "aerospace_workspace_change",
            "aerospace_mode_change",
            "aerospace_focus_change",
            "--set",
            &name,
            &format!("script=ws.sh {sid}"),
        ]);
        let r = runs(&fx);
        assert_eq!(r.len(), 2, "{sid}: {r:?}");
        assert!(r.iter().all(|r| r.item.as_deref() == Some(name.as_str())));
        assert_eq!(r[0].sender(), Some("aerospace_workspace_change"));
        assert_eq!(r[0].get("FOCUSED_WORKSPACE"), Some("2"));
        assert_eq!(r[0].get("PREV_WORKSPACE"), Some("1"));
        assert_eq!(r[0].get("NAME"), Some(name.as_str()));
        assert_eq!(r[1].sender(), Some("aerospace_mode_change"));
        assert_eq!(r[1].get("MODE"), Some("main"));
    }
    // Subscribing again to the same event delivers nothing new.
    let (_, fx) = h.msg_fx(&["--subscribe", "space.1", "aerospace_workspace_change"]);
    assert!(runs(&fx).is_empty());
    // An `updates=off` item is gated like for a real event.
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "off",
        "left",
        "--set",
        "off",
        "updates=off",
        "script=off.sh",
        "--subscribe",
        "off",
        "aerospace_workspace_change",
    ]);
    assert!(runs(&fx).is_empty());
    // Focus and monitor state arrive later and are delivered to later subscribers.
    feed(
        &mut h,
        Input::Aerospace(AerospaceEvent::FocusChanged {
            window_id: Some(9),
            workspace: "2".into(),
        }),
    );
    feed(&mut h, monitor("2", 1));
    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "w",
        "left",
        "--set",
        "w",
        "script=w.sh",
        "--subscribe",
        "w",
        "aerospace_focus_change",
        "aerospace_monitor_change",
        "aerospace_window_detected",
    ]);
    let r = runs(&fx);
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(r[0].get("WINDOW_ID"), Some("9"));
    assert_eq!(r[1].get("MONITOR_ID"), Some("1"));
}

/// After `--reload` the re-run config's subscribers and item-less handlers get the state
/// (AeroSpace does not resend it: the connection survives the reload).
#[test]
fn reload_delivers_the_stored_state_again() {
    let mut h = H::new();
    on(&mut h, "aerospace_workspace_change", 1);
    feed(&mut h, ws("4", "3"));
    h.msg(&["--reload"]);

    let fx = on(&mut h, "front_app_switched", 2);
    assert!(runs(&fx).is_empty());
    let fx = on(&mut h, "aerospace_workspace_change", 3);
    let r = runs(&fx);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].script, "lua:3");
    assert_eq!(r[0].get("FOCUSED_WORKSPACE"), Some("4"));
    assert_eq!(r[0].get("PREV_WORKSPACE"), Some("3"));
    assert_eq!(r[0].get("NAME"), None);

    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "space.4",
        "left",
        "--set",
        "space.4",
        "script=ws.sh",
        "--subscribe",
        "space.4",
        "aerospace_workspace_change",
    ]);
    let r = runs_of(&fx, "space.4");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].get("FOCUSED_WORKSPACE"), Some("4"));
}

// ---------------------------------------------------------------------------
// provider=aerospace only applies changes; manual triggers; env aliases
// ---------------------------------------------------------------------------

#[test]
fn provider_ignores_events_that_change_nothing() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "ws",
        "left",
        "--set",
        "ws",
        "provider=aerospace",
        "script=ws.sh",
    ]);
    let fx = feed(&mut h, ws("2", "1"));
    assert_eq!(runs_of(&fx, "ws").len(), 1);
    // Focus changes on the same workspace: the sample is the same.
    for wid in [1, 2, 3] {
        let fx = feed(
            &mut h,
            Input::Aerospace(AerospaceEvent::FocusChanged {
                window_id: Some(wid),
                workspace: "2".into(),
            }),
        );
        assert!(runs(&fx).is_empty(), "window {wid}");
    }
    // A real change applies again.
    let fx = feed(&mut h, ws("3", "2"));
    assert_eq!(runs_of(&fx, "ws").len(), 1);
    assert_eq!(label(&mut h, "ws"), "3");
    // Reconfiguring the provider applies even with an unchanged sample.
    let (_, fx) = h.msg_fx(&["--set", "ws", "provider.format=[{value}]"]);
    assert_eq!(runs_of(&fx, "ws").len(), 1);
    assert_eq!(label(&mut h, "ws"), "[3]");
}

/// Old AeroSpace without `subscribe`: `exec-on-workspace-change` triggers update the
/// state while not connected (provider and `--query aerospace` keep working), never while
/// connected (a late forked trigger must not overwrite a newer native event).
#[test]
fn manual_trigger_updates_the_state_only_while_disconnected() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "ws",
        "left",
        "--set",
        "ws",
        "provider=aerospace",
    ]);
    h.msg(&[
        "--trigger",
        "aerospace_workspace_change",
        "FOCUSED_WORKSPACE=2",
    ]);
    assert_eq!(label(&mut h, "ws"), "2");
    h.msg(&[
        "--trigger",
        "aerospace_workspace_change",
        "AEROSPACE_FOCUSED_WORKSPACE=5",
        "AEROSPACE_PREV_WORKSPACE=4",
    ]);
    assert_eq!(label(&mut h, "ws"), "5");
    let q = h.query(&["aerospace"]);
    assert_eq!(q["focused_workspace"], "5");
    assert_eq!(q["prev_workspace"], "4");

    feed(
        &mut h,
        Input::AerospaceStatus(AerospaceStatus {
            connected: true,
            transport: AerospaceTransport::Socket,
            server_version: None,
            error: None,
        }),
    );
    feed(&mut h, ws("6", "5"));
    h.msg(&[
        "--trigger",
        "aerospace_workspace_change",
        "FOCUSED_WORKSPACE=5",
    ]);
    assert_eq!(label(&mut h, "ws"), "6");
    assert_eq!(h.query(&["aerospace"])["focused_workspace"], "6");
}

#[test]
fn scripts_get_the_aerospace_aliases() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "s",
        "left",
        "--set",
        "s",
        "script=s.sh",
        "--subscribe",
        "s",
        "aerospace_workspace_change",
        "aerospace_monitor_change",
    ]);
    let fx = feed(&mut h, ws("2", "1"));
    let r = runs_of(&fx, "s");
    assert_eq!(r[0].get("AEROSPACE_FOCUSED_WORKSPACE"), Some("2"));
    assert_eq!(r[0].get("AEROSPACE_PREV_WORKSPACE"), Some("1"));
    let fx = feed(&mut h, monitor("3", 1));
    let r = runs_of(&fx, "s");
    assert_eq!(r[0].get("AEROSPACE_FOCUSED_WORKSPACE"), Some("3"));
    assert_eq!(
        r[0].get("INFO"),
        Some(r#"{"focused_workspace":"3","monitor_id":"1"}"#)
    );
}
