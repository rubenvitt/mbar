//! WP-D: script env construction (`events.md` §4.2/§4.4/§6.2, D1, D16, Q3) and the mach
//! helper payload (§4.6).

use mbar_core::event::{click_env, trigger_env};
use mbar_core::platform::MouseButton;
use mbar_core::script::{
    build_click_script_env, build_update_env, serialize_for_mach, EnvVars, Sender,
    MACH_HELPER_DESTROY,
};

fn kv(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

fn space_item_env() -> EnvVars {
    let mut e = EnvVars::new();
    e.set("NAME", "space.1");
    e.set("SELECTED", "false");
    e.set("SID", "1");
    e.set("DID", "1");
    e
}

#[test]
fn event_env_order() {
    let mut persistent = space_item_env();
    let mut ev = EnvVars::new();
    ev.set("INFO", "{\n\t\"display-1\": 1\n}");
    let env = build_update_env(
        &mut persistent,
        Some(&ev),
        Some("space.1"),
        &Sender::Event("space_change".into()),
    );
    // event keys, persistent keys (in order), NAME (moved to the end), SENDER.
    assert_eq!(
        env.to_vec(),
        vec![
            kv("INFO", "{\n\t\"display-1\": 1\n}"),
            kv("SELECTED", "false"),
            kv("SID", "1"),
            kv("DID", "1"),
            kv("NAME", "space.1"),
            kv("SENDER", "space_change"),
        ]
    );
    // The persistent env is untouched by event deliveries.
    assert_eq!(persistent, space_item_env());
}

#[test]
fn item_vars_override_event_vars() {
    let mut persistent = EnvVars::new();
    persistent.set("NAME", "a");
    persistent.set("PERCENTAGE", "40");
    let ev = trigger_env(&["PERCENTAGE=99".into(), "NAME=x".into(), "X=1".into()]);
    let env = build_update_env(
        &mut persistent,
        Some(&ev),
        Some("a"),
        &Sender::Event("my_event".into()),
    );
    assert_eq!(
        env.to_vec(),
        vec![
            kv("X", "1"),
            kv("PERCENTAGE", "40"),
            kv("NAME", "a"),
            kv("SENDER", "my_event"),
        ]
    );
}

#[test]
fn fresh_env_per_recipient_d1_d16() {
    // One event delivered to a space item, then to a plain item: the plain item must not
    // see SELECTED/SID/DID of the first one.
    let ev = trigger_env(&["INFO=hi".into()]);
    let mut first = space_item_env();
    let mut second = EnvVars::new();
    second.set("NAME", "plain");
    let sender = Sender::Event("custom".into());
    let e1 = build_update_env(&mut first, Some(&ev), Some("space.1"), &sender);
    let e2 = build_update_env(&mut second, Some(&ev), Some("plain"), &sender);
    assert_eq!(e1.get("SELECTED"), Some("false"));
    assert_eq!(
        e2.to_vec(),
        vec![
            kv("INFO", "hi"),
            kv("NAME", "plain"),
            kv("SENDER", "custom")
        ]
    );
    // The shared event env itself is not mutated.
    assert_eq!(ev.to_vec(), vec![kv("INFO", "hi")]);
}

#[test]
fn env_less_delivery_writes_sender_into_persistent_q3() {
    let mut persistent = EnvVars::new();
    persistent.set("NAME", "clock");
    let env = build_update_env(&mut persistent, None, Some("clock"), &Sender::Routine);
    assert_eq!(
        env.to_vec(),
        vec![kv("NAME", "clock"), kv("SENDER", "routine")]
    );
    assert_eq!(persistent.get("SENDER"), Some("routine"));

    let env = build_update_env(&mut persistent, None, Some("clock"), &Sender::Forced);
    assert_eq!(env.get("SENDER"), Some("forced"));
    let env = build_update_env(
        &mut persistent,
        None,
        Some("clock"),
        &Sender::Event("mouse.entered".into()),
    );
    assert_eq!(env.get("SENDER"), Some("mouse.entered"));
    assert_eq!(persistent.get("SENDER"), Some("mouse.entered"));

    // The leaked SENDER shows up in a later click_script env (stale), and is overridden in
    // a later event env by the event's own SENDER.
    let click = click_env(MouseButton::Left, 0, 256);
    let cenv = build_click_script_env(&click, &persistent);
    assert_eq!(cenv.get("SENDER"), Some("mouse.entered"));
    let env = build_update_env(
        &mut persistent,
        Some(&click),
        Some("clock"),
        &Sender::Event("mouse.clicked".into()),
    );
    assert_eq!(env.get("SENDER"), Some("mouse.clicked"));
    // The returned env is a copy: mutating it does not touch the persistent env.
    let mut e = env.clone();
    e.set("NEW", "1");
    assert_eq!(persistent.get("NEW"), None);
}

#[test]
fn name_none_is_omitted() {
    let mut persistent = EnvVars::new();
    let ev = trigger_env(&["K=v".into()]);
    let env = build_update_env(&mut persistent, Some(&ev), None, &Sender::Provider);
    assert_eq!(env.to_vec(), vec![kv("K", "v"), kv("SENDER", "provider")]);
}

#[test]
fn click_script_env() {
    let click = click_env(MouseButton::Right, 1, 0x40000);
    let mut persistent = EnvVars::new();
    persistent.set("NAME", "vol");
    persistent.set("PERCENTAGE", "30");
    persistent.set("MODIFIER", "shadowed");
    let env = build_click_script_env(&click, &persistent);
    assert_eq!(
        env.to_vec(),
        vec![
            kv(
                "INFO",
                "{\n\t\"button\": \"right\",\n\t\"button_code\": 1,\n\t\"modifier\": \"ctrl\",\n\t\"modfier_code\": 262144\n}\n"
            ),
            kv("BUTTON", "right"),
            kv("NAME", "vol"),
            kv("PERCENTAGE", "30"),
            kv("MODIFIER", "shadowed"),
        ]
    );
    // No explicit SENDER.
    assert_eq!(env.get("SENDER"), None);
}

#[test]
fn mach_payload() {
    let mut env = EnvVars::new();
    env.set("NAME", "x");
    env.set("SENDER", "routine");
    assert_eq!(
        serialize_for_mach(&env),
        b"NAME\0x\0SENDER\0routine\0\0".to_vec()
    );
    assert_eq!(serialize_for_mach(&EnvVars::new()), b"\0".to_vec());
    let mut env = EnvVars::new();
    env.set("E", "");
    assert_eq!(serialize_for_mach(&env), b"E\0\0\0".to_vec());
    assert_eq!(MACH_HELPER_DESTROY, b"k\0");
}
