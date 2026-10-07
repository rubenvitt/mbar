//! Review probes (temporary).
mod wpc_common;
use mbar_core::platform::{Input, MouseKind, OsEvent, WindowKey};
use std::time::Duration;
use wpc_common::*;

fn senders(fx: &[mbar_core::platform::Effect], item: &str) -> Vec<String> {
    runs_of(fx, item)
        .iter()
        .map(|r| r.sender().unwrap_or("").to_string())
        .collect()
}

#[test]
fn p1_when_shown_after_wake() {
    let mut h = H::new();
    h.msg(&[
        "--add", "item", "w", "left", "--set", "w", "label=W", "script=w.sh",
        "updates=when_shown", "--subscribe", "w", "system_woke", "display_change",
        "space_change",
    ]);
    h.frame();
    assert!(h.rt.model.items[0].is_shown());
    h.input(Input::Event(OsEvent::SystemWillSleep));
    let fx = h.input(Input::Event(OsEvent::SystemWoke));
    eprintln!("wake senders: {:?}", senders(&fx, "w"));
    let fx = h.input(Input::DisplaysChanged);
    eprintln!("displays changed senders: {:?}", senders(&fx, "w"));
    assert!(!senders(&fx, "w").is_empty(), "when_shown item missed display rebuild events");
}
