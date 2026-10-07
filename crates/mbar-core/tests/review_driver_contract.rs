//! Review regressions of the runtime side of the driver contract (F9, PERF-11, F11).

mod wpc_common;
use mbar_core::command::MonitorMode;
use mbar_core::platform::{Effect, Input, ReplyToken};
use std::time::Duration;
use wpc_common::*;

fn message(h: &mut H, reply: u64, args: &[&str]) -> Vec<Effect> {
    h.input(Input::Message {
        args: args.iter().map(|s| s.to_string()).collect(),
        reply: ReplyToken(reply),
    })
}

fn monitor_start(fx: &[Effect]) -> Option<(MonitorMode, String)> {
    fx.iter().find_map(|e| match e {
        Effect::MonitorStart { mode, text, .. } => Some((*mode, text.clone())),
        _ => None,
    })
}

/// F9: the runtime reports an executed `--monitor` (with the message's output) instead of
/// the platform scanning argv; an unexecuted one is a plain reply.
#[test]
fn monitor_start_only_when_executed() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left"]);

    let fx = message(&mut h, 100, &["--set", "a", "label=x", "", "--monitor"]);
    assert!(monitor_start(&fx).is_none());
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Reply {
            reply: ReplyToken(100),
            ..
        }
    )));

    let fx = message(&mut h, 101, &["--query", "a", "--monitor", "events"]);
    let (mode, text) = monitor_start(&fx).expect("monitor accepted");
    assert_eq!(mode, MonitorMode::Events);
    assert!(text.contains("\"name\": \"a\""), "{text}");
    assert!(!fx.iter().any(|e| matches!(e, Effect::Reply { .. })));

    let fx = message(&mut h, 102, &["--monitor", "events", "--monitor", "stats"]);
    assert_eq!(monitor_start(&fx).unwrap().0, MonitorMode::All);
}

/// PERF-11: render and animation deadlines are not timer work.
#[test]
fn timer_due_ignores_render_and_animation() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left"]);
    let now = h.res.now;
    message(&mut h, 50, &["--set", "a", "label=changed"]);
    assert_eq!(h.rt.next_deadline(), Some(now), "render pending");
    assert!(!h.rt.timer_due(now));
    h.msg(&["--animate", "linear", "30", "--set", "a", "y_offset=10"]);
    assert!(h.rt.animating());
    assert!(!h.rt.timer_due(h.res.now));
    assert!(h.rt.timer_due(now + Duration::from_secs(1)), "routine tick");
}

/// F11: `lua.avg_us` / `max_us` come from the recorded callbacks.
#[test]
fn lua_stats_recorded() {
    let mut h = H::new();
    h.rt.record_lua_callbacks(2, 300, 200);
    h.rt.record_lua_callbacks(1, 30, 30);
    let v = h.query(&["stats"]);
    assert_eq!(v["lua"]["callbacks"], 3);
    assert_eq!(v["lua"]["avg_us"], 110);
    assert_eq!(v["lua"]["max_us"], 200);
}
