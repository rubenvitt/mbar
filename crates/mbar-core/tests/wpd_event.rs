//! WP-D: event INFO payloads and env builders (byte-exact, `events.md` §5–§7) and the
//! scroll throttle (§6.3).

use mbar_core::event::{
    button_description, click_env, display_change_info, is_forced_trigger, level_info,
    modifier_description, scroll_env, scroll_global_env, space_change_info, trigger_env, BarSpace,
    ScrollThrottle,
};
use mbar_core::platform::MouseButton;
use mbar_core::script::EnvVars;
use std::time::{Duration, Instant};

fn pairs(env: &EnvVars) -> Vec<(String, String)> {
    env.to_vec()
}

fn kv(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

#[test]
fn space_change_payload() {
    assert_eq!(
        space_change_info(&[BarSpace { adid: 1, sid: 2 }, BarSpace { adid: 2, sid: 5 }]),
        "{\n\t\"display-1\": 2,\n\t\"display-2\": 5\n}"
    );
    assert_eq!(
        space_change_info(&[BarSpace { adid: 1, sid: 0 }]),
        "{\n\t\"display-1\": 0\n}"
    );
    assert_eq!(space_change_info(&[]), "{\n}");
    // B2: no truncation for long lines.
    assert_eq!(
        space_change_info(&[BarSpace {
            adid: 12,
            sid: 1234
        }]),
        "{\n\t\"display-12\": 1234\n}"
    );
}

#[test]
fn display_and_level_payloads() {
    assert_eq!(display_change_info(1), "1");
    assert_eq!(display_change_info(0), "0");
    // B3: no 2-char truncation.
    assert_eq!(display_change_info(123), "123");

    assert_eq!(level_info(0.0), "0");
    assert_eq!(level_info(1.0), "100");
    assert_eq!(level_info(0.5), "50");
    assert_eq!(level_info(0.344), "34");
    // f32 inputs promoted to double: 0.345f32 = 0.34499999… -> 34, 0.335f32 = 0.33500001… -> 34.
    assert_eq!(level_info(0.345), "34");
    assert_eq!(level_info(0.335), "34");
    assert_eq!(level_info(0.004), "0");
    assert_eq!(level_info(0.005), "0");
    assert_eq!(level_info(0.006), "1");
}

#[test]
fn modifiers() {
    assert_eq!(modifier_description(0), "none");
    assert_eq!(modifier_description(256), "none");
    assert_eq!(modifier_description(0x20000), "shift");
    assert_eq!(modifier_description(0x40000), "ctrl");
    assert_eq!(modifier_description(0x80000), "alt");
    assert_eq!(modifier_description(0x100000), "cmd");
    assert_eq!(modifier_description(0x800000), "fn");
    assert_eq!(
        modifier_description(0x20000 | 0x40000 | 0x80000 | 0x100000 | 0x800000 | 0x100),
        "shift,ctrl,alt,cmd,fn"
    );
    assert_eq!(modifier_description(0x100000 | 0x20000), "shift,cmd");
    assert_eq!(button_description(MouseButton::Left), "left");
    assert_eq!(button_description(MouseButton::Right), "right");
    assert_eq!(button_description(MouseButton::Other), "other");
}

#[test]
fn click_payload() {
    let env = click_env(MouseButton::Left, 0, 256);
    assert_eq!(
        pairs(&env),
        vec![
            kv(
                "INFO",
                "{\n\t\"button\": \"left\",\n\t\"button_code\": 0,\n\t\"modifier\": \"none\",\n\t\"modfier_code\": 256\n}\n"
            ),
            kv("BUTTON", "left"),
            kv("MODIFIER", "none"),
        ]
    );
    let env = click_env(MouseButton::Other, 2, 0x100100);
    assert_eq!(
        env.get("INFO"),
        Some(
            "{\n\t\"button\": \"other\",\n\t\"button_code\": 2,\n\t\"modifier\": \"cmd\",\n\t\"modfier_code\": 1048832\n}\n"
        )
    );
    assert_eq!(env.get("BUTTON"), Some("other"));
    assert_eq!(env.get("MODIFIER"), Some("cmd"));
    let env = click_env(MouseButton::Right, 1, 0x60000);
    assert_eq!(env.get("BUTTON"), Some("right"));
    assert_eq!(env.get("MODIFIER"), Some("shift,ctrl"));
}

#[test]
fn scroll_payloads() {
    let env = scroll_env(-3, 0x80000);
    assert_eq!(
        pairs(&env),
        vec![
            kv(
                "INFO",
                "{\n\t\"delta\": -3,\n\t\"modifier\": \"alt\",\n\t\"modfier_code\": 524288\n}\n"
            ),
            kv("SCROLL_DELTA", "-3"),
            kv("MODIFIER", "alt"),
        ]
    );
    let env = scroll_global_env(5, 2, 0);
    assert_eq!(
        pairs(&env),
        vec![
            kv("SCROLL_DELTA", "5"),
            kv(
                "INFO",
                "{\n\t\"delta\": 5,\n\t\"modifier\": \"none\",\n\t\"modfier_code\": 0\n}\n"
            ),
            kv("DID", "2"),
            kv("MODIFIER", "none"),
        ]
    );
}

#[test]
fn trigger_vars() {
    let toks: Vec<String> = ["A=1", "NOEQ", "B=", "C=x=y", "A=2", "=v", "D=with space"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let env = trigger_env(&toks);
    // `B=` skipped, split at the first `=`, later duplicates win (and move to the end).
    assert_eq!(
        pairs(&env),
        vec![
            kv("C", "x=y"),
            kv("A", "2"),
            kv("", "v"),
            kv("D", "with space"),
        ]
    );
    assert!(trigger_env(&[]).is_empty());
    assert!(is_forced_trigger("volume_change"));
    assert!(!is_forced_trigger("brightness_change"));
    assert!(!is_forced_trigger("front_app_switched"));
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn scroll_throttle_leading_edge() {
    let t0 = Instant::now();
    let mut th = ScrollThrottle::default();
    // First event always delivered.
    assert_eq!(th.feed(1, t0), Some(1));
    th.reset();
    // Within 150 ms: swallowed and accumulated.
    assert_eq!(th.feed(2, t0 + ms(10)), None);
    assert_eq!(th.feed(3, t0 + ms(149)), None);
    assert_eq!(th.acc, 5);
    // Exactly 150 ms: delivered with the carry-over (≤ 300 ms since last delivery).
    assert_eq!(th.feed(1, t0 + ms(150)), Some(6));
    th.reset();
    assert_eq!(th.acc, 0);
    assert_eq!(th.last, Some(t0 + ms(150)));
}

#[test]
fn scroll_throttle_carry_over_window() {
    let t0 = Instant::now();
    let mut th = ScrollThrottle::default();
    assert_eq!(th.feed(-1, t0), Some(-1));
    assert_eq!(th.feed(-4, t0 + ms(100)), None);
    // Exactly 300 ms after the last delivery: accumulation kept.
    assert_eq!(th.feed(-1, t0 + ms(300)), Some(-5));

    // Older than 300 ms: accumulation dropped.
    let t1 = t0 + ms(300);
    assert_eq!(th.feed(7, t1 + ms(50)), None);
    assert_eq!(th.feed(2, t1 + ms(301)), Some(2));
    // The accumulation is cleared on delivery even without reset().
    assert_eq!(th.acc, 0);
    // Every swallowed event is measured from the last *delivered* one.
    let t2 = t1 + ms(301);
    assert_eq!(th.feed(1, t2 + ms(100)), None);
    assert_eq!(th.feed(1, t2 + ms(140)), None);
    assert_eq!(th.feed(1, t2 + ms(160)), Some(3));
}

#[test]
fn scroll_throttle_no_overflow_panic() {
    let t0 = Instant::now();
    let mut th = ScrollThrottle::default();
    th.feed(1, t0);
    assert_eq!(th.feed(i32::MAX, t0 + ms(1)), None);
    assert_eq!(th.feed(i32::MAX, t0 + ms(2)), None);
    assert!(th.feed(1, t0 + ms(200)).is_some());
}
