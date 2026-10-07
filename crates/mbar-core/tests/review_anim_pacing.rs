//! Review regression RT-3: while an animation runs, `next_deadline` is "now" (display-link
//! platforms pace frames themselves, D12), so a headless loop sleeping until the deadline
//! busy-spun. The paced deadline asks for the next frame one frame interval after the last.

mod wpc_common;
use mbar_core::animation::{Animator, FRAME_INTERVAL};
use std::time::{Duration, Instant};
use wpc_common::*;

#[test]
fn animator_paced_deadline_is_one_frame_after_last_step() {
    assert_eq!(
        Animator::new().next_deadline_paced(Instant::now(), FRAME_INTERVAL),
        None
    );
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left"]);
    h.msg(&["--animate", "linear", "600", "--set", "a", "y_offset=100"]);
    // The runtime stepped the first frame at `now` (in `msg`).
    let now = h.res.now;
    assert_eq!(
        h.rt.next_deadline(),
        Some(now),
        "display-link contract unchanged"
    );
    assert_eq!(
        h.rt.next_deadline_paced(FRAME_INTERVAL),
        Some(now + FRAME_INTERVAL)
    );
}

#[test]
fn paced_deadline_follows_frames_and_ends_with_the_animation() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left"]);
    h.msg(&["--animate", "linear", "600", "--set", "a", "y_offset=100"]);
    let t0 = h.res.now;
    // 5 ms later (no frame): the paced deadline is still in the future.
    h.res.now = t0 + Duration::from_millis(5);
    let d = h.rt.next_deadline_paced(FRAME_INTERVAL).unwrap();
    assert!(d > h.res.now, "paced deadline must not be in the past");
    // A frame at the deadline moves the next one by another interval.
    h.res.now = d;
    h.frame();
    assert_eq!(
        h.rt.next_deadline_paced(FRAME_INTERVAL),
        Some(d + FRAME_INTERVAL)
    );
    // A late frame (long sleep) is due immediately, not `interval` after an old frame.
    h.res.now = d + Duration::from_millis(500);
    assert!(h
        .rt
        .next_deadline_paced(FRAME_INTERVAL)
        .is_some_and(|x| x <= h.res.now));
    // Once the animation finished, pacing no longer contributes (routine clock only).
    h.res.now = t0 + Duration::from_secs(11);
    h.frame();
    assert!(!h.rt.animating());
    assert_eq!(
        h.rt.next_deadline_paced(FRAME_INTERVAL),
        h.rt.next_deadline()
    );
}
