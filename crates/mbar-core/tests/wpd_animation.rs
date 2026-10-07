//! WP-D: animation curves, frame stepping over simulated wall-clock time (D12), chaining,
//! cancellation (snap / locked / D18) and the marquee (`events.md` §10, `components.md` §4.10).

use mbar_core::animation::{interpolate, marquee, AnimStep, Animator, Curve};
use mbar_core::components::Text;
use mbar_core::item::ItemId;
use mbar_core::props::{AnimTarget, AnimValue, PendingAnim};
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn pend(
    target: AnimTarget,
    path: &str,
    from: AnimValue,
    to: AnimValue,
    frames: u32,
) -> PendingAnim {
    PendingAnim {
        target,
        path: path.into(),
        from,
        to,
        duration: frames,
        curve: Curve::Linear,
    }
}

fn int(target: AnimTarget, path: &str, from: i32, to: i32, frames: u32) -> PendingAnim {
    pend(
        target,
        path,
        AnimValue::Int(from),
        AnimValue::Int(to),
        frames,
    )
}

fn values(steps: &[AnimStep]) -> Vec<AnimValue> {
    steps.iter().map(|s| s.value).collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn curve_key_points() {
    assert!(close(Curve::Linear.eval(0.5), 0.5));
    assert!(close(Curve::Quadratic.eval(0.5), 0.25));
    assert!(close(Curve::Sin.eval(0.5), std::f64::consts::FRAC_1_SQRT_2));
    assert!(close(Curve::Exp.eval(0.5), 0.5 * (-0.5f64).exp()));
    assert!(close(Curve::Tanh.eval(0.5), 0.5));
    // f32 intermediates: 1 - 0.25 is exact, sqrt(0.75).
    assert!(close(Curve::Circ.eval(0.5), 0.75f64.sqrt()));
    assert!(close(Curve::Bounce.eval(0.5), 0.765625));
    assert!(close(
        Curve::Overshoot.eval(0.5),
        1.0 - 2.70158 * 0.125 + 1.70158 * 0.25
    ));
    // tanh is symmetric and hits its ends (within f64 precision).
    assert!(close(Curve::Tanh.eval(0.25) + Curve::Tanh.eval(0.75), 1.0));
    assert!(Curve::Tanh.eval(0.0).abs() < 1e-12);
    // Unknown first characters are linear; selection by first byte only.
    assert_eq!(Curve::from_token("xyz"), Curve::Linear);
    assert_eq!(Curve::from_token("quad"), Curve::Quadratic);
    assert_eq!(Curve::from_token("circ"), Curve::Circ);
    assert_eq!(Curve::from_token("tanh"), Curve::Tanh);
    assert_eq!(Curve::from_token("overshoot"), Curve::Overshoot);
}

#[test]
fn color_bytes_truncate_per_byte() {
    // byte-wise (u8)((1-s)a + s b): 0x03 * 0.5 = 1.5 -> 1, 0xff * 0.5 = 127.5 -> 127.
    assert_eq!(
        interpolate(
            AnimValue::Color(0x00000000),
            AnimValue::Color(0x03ff0103),
            0.5,
            false
        ),
        AnimValue::Color(0x017f0001)
    );
    // Decreasing bytes truncate too: 0xff -> 0x00 at 0.5 = 127.5 -> 127.
    assert_eq!(
        interpolate(
            AnimValue::Color(0xffffffff),
            AnimValue::Color(0x00000000),
            0.5,
            false
        ),
        AnimValue::Color(0x7f7f7f7f)
    );
}

#[test]
fn single_int_animation_wall_clock() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    a.add(int(AnimTarget::Bar, "margin", 0, 100, 60)); // 60 frames = 1 s
    assert!(a.needs_frame());
    assert!(a.is_active());
    assert_eq!(a.next_deadline(t0), Some(t0));

    // First stepped frame: t = 0, value = initial.
    let s = a.step(t0);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].target, AnimTarget::Bar);
    assert_eq!(s[0].path, "margin");
    assert_eq!(s[0].value, AnimValue::Int(0));
    assert_eq!(a.animations()[0].start, Some(t0));

    assert_eq!(values(&a.step(t0 + ms(250))), vec![AnimValue::Int(25)]);
    assert_eq!(values(&a.step(t0 + ms(500))), vec![AnimValue::Int(50)]);
    // Frames at arbitrary refresh rates: 8.333 ms (120 Hz) after 500 ms.
    let v = a.step(t0 + ms(508))[0].value;
    assert_eq!(v, AnimValue::Int(51)); // 50.8 + 0.5 -> 51
                                       // Final frame lands exactly on the target, then the animation is removed.
    assert_eq!(values(&a.step(t0 + ms(1000))), vec![AnimValue::Int(100)]);
    assert!(a.is_empty());
    assert!(!a.needs_frame());
    assert_eq!(a.next_deadline(t0 + ms(1000)), None);
    assert!(a.step(t0 + ms(1100)).is_empty());
}

#[test]
fn late_frame_jumps_to_final_and_start_is_first_step() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    a.add(int(AnimTarget::Bar, "height", 10, 20, 30));
    // The animation was created long before its first frame: progress starts at the first
    // stepped frame, not at creation.
    let first = t0 + ms(5000);
    assert_eq!(values(&a.step(first)), vec![AnimValue::Int(10)]);
    // A frame far past the end (sleep/wake) completes it.
    assert_eq!(
        values(&a.step(first + ms(60_000))),
        vec![AnimValue::Int(20)]
    );
    assert!(a.is_empty());
}

#[test]
fn zero_duration_completes_on_first_frame() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    a.add(int(AnimTarget::Bar, "width", 5, -1, 0));
    assert_eq!(values(&a.step(t0)), vec![AnimValue::Int(-1)]);
    assert!(a.is_empty());
}

#[test]
fn curve_applied_and_final_bypasses_curve() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    let mut p = int(AnimTarget::Bar, "y_offset", 0, 100, 60);
    p.curve = Curve::Quadratic;
    a.add(p);
    a.step(t0);
    // t = 0.5 -> s = 0.25 -> 25.
    assert_eq!(values(&a.step(t0 + ms(500))), vec![AnimValue::Int(25)]);
    // Overshoot would exceed the target mid-way but lands exactly on it.
    let mut b = Animator::new();
    let mut p = int(AnimTarget::Bar, "y_offset", 0, 100, 60);
    p.curve = Curve::Overshoot;
    b.add(p);
    b.step(t0);
    let mid = b.step(t0 + ms(800))[0].value.as_i32();
    assert!(mid > 100, "{mid}");
    assert_eq!(values(&b.step(t0 + ms(1000))), vec![AnimValue::Int(100)]);
}

#[test]
fn negative_int_truncates_toward_zero() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    a.add(int(AnimTarget::Bar, "y_offset", 0, -10, 60));
    a.step(t0);
    // s = 0.27: -2.7 + 0.5 = -2.2 -> -2 (C cast), not -3.
    let v = a.step(t0 + Duration::from_micros(270_000))[0].value;
    assert_eq!(v, AnimValue::Int(-2));
}

#[test]
fn float_and_color_animation_steps() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    let item = AnimTarget::Item(ItemId(7));
    a.add(pend(
        item,
        "label.font.size",
        AnimValue::Float(10.0),
        AnimValue::Float(20.0),
        60,
    ));
    a.add(pend(
        item,
        "label.color",
        AnimValue::Color(0x00000000),
        AnimValue::Color(0xff0000ff),
        60,
    ));
    let s = a.step(t0);
    assert_eq!(
        values(&s),
        vec![AnimValue::Float(10.0), AnimValue::Color(0)]
    );
    let s = a.step(t0 + ms(500));
    assert_eq!(
        values(&s),
        vec![AnimValue::Float(15.0), AnimValue::Color(0x7f00007f)]
    );
    // Different keys run concurrently, in insertion order.
    assert_eq!(s[0].path, "label.font.size");
    assert_eq!(s[1].path, "label.color");
    let s = a.step(t0 + ms(1000));
    assert_eq!(
        values(&s),
        vec![AnimValue::Float(20.0), AnimValue::Color(0xff0000ff)]
    );
    assert!(a.is_empty());
}

#[test]
fn chained_animations_successor_starts_in_same_frame() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    // `--animate linear 30 --set x y_offset=10 y_offset=0` in one batch.
    a.add(int(AnimTarget::Bar, "y_offset", 0, 10, 30));
    a.add(int(AnimTarget::Bar, "y_offset", 99, 0, 30));
    assert_eq!(a.animations()[1].from, AnimValue::Int(10));
    a.lock_all();

    // Only the head is stepped.
    assert_eq!(values(&a.step(t0)), vec![AnimValue::Int(0)]);
    assert!(a.animations()[1].waiting);
    assert_eq!(values(&a.step(t0 + ms(250))), vec![AnimValue::Int(5)]);
    // Head finishes: successor released and stepped in the same frame at t = 0.
    let s = a.step(t0 + ms(500));
    assert_eq!(values(&s), vec![AnimValue::Int(10), AnimValue::Int(10)]);
    assert_eq!(a.len(), 1);
    assert_eq!(a.animations()[0].start, Some(t0 + ms(500)));
    assert_eq!(values(&a.step(t0 + ms(750))), vec![AnimValue::Int(5)]);
    assert_eq!(values(&a.step(t0 + ms(1000))), vec![AnimValue::Int(0)]);
    assert!(a.is_empty());
}

#[test]
fn chain_with_zero_duration_tail_completes_together() {
    // Text width animation pre -> post followed by the chained 0-frame `-1` unpin.
    let t0 = Instant::now();
    let mut a = Animator::new();
    let item = AnimTarget::Item(ItemId(1));
    a.add(int(item, "label.width", 40, 80, 6));
    a.add(int(item, "label.width", 0, -1, 0));
    assert_eq!(values(&a.step(t0)), vec![AnimValue::Int(40)]);
    assert_eq!(
        values(&a.step(t0 + ms(100))),
        vec![AnimValue::Int(80), AnimValue::Int(-1)]
    );
    assert!(a.is_empty());
}

#[test]
fn three_link_chain_all_zero_duration_in_one_frame() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    a.add(int(AnimTarget::Bar, "margin", 0, 1, 0));
    a.add(int(AnimTarget::Bar, "margin", 0, 2, 0));
    a.add(int(AnimTarget::Bar, "margin", 0, 3, 0));
    assert_eq!(
        values(&a.step(t0)),
        vec![AnimValue::Int(1), AnimValue::Int(2), AnimValue::Int(3)]
    );
    assert!(a.is_empty());
}

#[test]
fn cancel_snaps_and_cancel_locked_restarts_from_intermediate() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    a.add(int(AnimTarget::Bar, "y_offset", 0, 10, 60));
    a.add(int(AnimTarget::Bar, "y_offset", 0, 20, 60));
    a.lock_all();
    a.step(t0);
    let cur = a.step(t0 + ms(500))[0].value; // 5
    assert_eq!(cur, AnimValue::Int(5));

    // Next batch, animated assignment: locked chain removed without snapping; the new
    // animation starts from the current intermediate value (read by the property layer).
    a.cancel_locked(AnimTarget::Bar, "y_offset");
    assert!(a.is_empty());
    a.add(pend(
        AnimTarget::Bar,
        "y_offset",
        cur,
        AnimValue::Int(-5),
        60,
    ));
    assert_eq!(values(&a.step(t0 + ms(600))), vec![AnimValue::Int(5)]);
    assert_eq!(values(&a.step(t0 + ms(1100))), vec![AnimValue::Int(0)]);

    // Immediate assignment: everything snaps (finals in list order).
    a.add(int(AnimTarget::Bar, "y_offset", 0, 7, 60));
    assert_eq!(
        a.cancel(AnimTarget::Bar, "y_offset"),
        vec![AnimValue::Int(-5), AnimValue::Int(7)]
    );
    assert!(a.is_empty());
}

#[test]
fn cancel_locked_spares_unlocked_and_other_targets() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    a.add(int(AnimTarget::Bar, "y_offset", 0, 10, 30));
    a.add(int(AnimTarget::Bar, "y_offset", 0, 0, 30));
    // Unlocked chain + a locked other key: cancel_locked of the chain's key leaves it.
    a.lock_all();
    a.add(int(AnimTarget::Bar, "margin", 0, 4, 30));
    a.cancel_locked(AnimTarget::Bar, "margin");
    assert_eq!(a.len(), 3);
    a.cancel_target(AnimTarget::Default);
    assert_eq!(a.len(), 3);
    let s = a.step(t0);
    assert_eq!(s.len(), 2); // y_offset head + margin
}

#[test]
fn removed_item_animations_cancelled_d18() {
    let t0 = Instant::now();
    let mut a = Animator::new();
    let gone = AnimTarget::Item(ItemId(3));
    let kept = AnimTarget::Item(ItemId(4));
    a.add(int(gone, "y_offset", 0, 10, 30));
    a.add(int(kept, "y_offset", 0, 10, 30));
    a.add(int(gone, "y_offset", 0, 0, 30));
    a.step(t0);
    a.cancel_target(gone);
    assert_eq!(a.len(), 1);
    let s = a.step(t0 + ms(500));
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].target, kept);
    assert_eq!(s[0].value, AnimValue::Int(10));
    assert!(a.is_empty());
}

#[test]
fn clear_drops_everything() {
    let mut a = Animator::new();
    a.add(int(AnimTarget::Bar, "y_offset", 0, 10, 30));
    a.add(int(AnimTarget::Default, "y_offset", 0, 10, 30));
    a.clear();
    assert!(a.is_empty());
    assert_eq!(a.next_deadline(Instant::now()), None);
}

fn truncated_text(width: u32, full: f32) -> Text {
    let mut t = Text {
        string: "a long label text".into(),
        max_chars: 5,
        width,
        ..Default::default()
    };
    t.bounds.width = full;
    t
}

#[test]
fn marquee_animations() {
    let target = AnimTarget::Item(ItemId(9));
    let t = truncated_text(50, 120.0);
    let m = marquee(target, "label.", &t).expect("marquee");
    assert_eq!(m.len(), 3);
    // D1 = (u32)(100 * 120/50) = 240 frames.
    assert_eq!(
        m[0],
        PendingAnim {
            target,
            path: "label.scroll".into(),
            from: AnimValue::Float(0.0),
            to: AnimValue::Float(120.0),
            duration: 240,
            curve: Curve::Linear,
        }
    );
    assert_eq!(m[1].to, AnimValue::Float(-50.0));
    assert_eq!(m[1].duration, 0);
    assert_eq!(m[2].from, AnimValue::Float(-50.0));
    assert_eq!(m[2].to, AnimValue::Float(0.0));
    assert_eq!(m[2].duration, 100);
    assert!(m
        .iter()
        .all(|p| p.path == "label.scroll" && p.curve == Curve::Linear));

    // Prefix is used verbatim (slider knob).
    let k = marquee(target, "slider.knob.", &t).unwrap();
    assert_eq!(k[0].path, "slider.knob.scroll");

    // Duration truncation: 100 * 101/30 = 336.67 -> 336.
    let t2 = truncated_text(30, 101.0);
    assert_eq!(marquee(target, "icon.", &t2).unwrap()[0].duration, 336);

    // Run the chain through the animator (cancel_locked + add as the runtime does).
    let t0 = Instant::now();
    let mut a = Animator::new();
    for p in m {
        a.cancel_locked(p.target, &p.path.clone());
        a.add(p);
    }
    assert_eq!(values(&a.step(t0)), vec![AnimValue::Float(0.0)]);
    // 240 frames = 4 s; half-way = 60 px.
    assert_eq!(values(&a.step(t0 + ms(2000))), vec![AnimValue::Float(60.0)]);
    // End of phase 1: 120, jump to -50 and phase 3 starts at -50 in the same frame.
    assert_eq!(
        values(&a.step(t0 + ms(4000))),
        vec![
            AnimValue::Float(120.0),
            AnimValue::Float(-50.0),
            AnimValue::Float(-50.0)
        ]
    );
    // Phase 3: 100 frames; half-way at 50/60 s.
    let s = a.step(t0 + ms(4000) + Duration::from_nanos(833_333_333));
    let v = s[0].value.as_f32();
    assert!((v + 25.0).abs() < 1e-3, "{v}");
    assert_eq!(values(&a.step(t0 + ms(6000))), vec![AnimValue::Float(0.0)]);
    assert!(a.is_empty());
}

#[test]
fn marquee_preconditions() {
    let target = AnimTarget::Item(ItemId(1));
    // max_chars == 0.
    let mut t = truncated_text(50, 120.0);
    t.max_chars = 0;
    assert!(marquee(target, "label.", &t).is_none());
    // Already scrolling.
    let mut t = truncated_text(50, 120.0);
    t.scroll = -3.0;
    assert!(marquee(target, "label.", &t).is_none());
    // Const width smaller than the truncated width.
    let mut t = truncated_text(50, 120.0);
    t.has_const_width = true;
    t.custom_width = 49;
    assert!(marquee(target, "label.", &t).is_none());
    // Const width >= width is fine.
    t.custom_width = 50;
    assert!(marquee(target, "label.", &t).is_some());
    // Width 0 and not truncated.
    assert!(marquee(target, "label.", &truncated_text(0, 120.0)).is_none());
    assert!(marquee(target, "label.", &truncated_text(120, 120.0)).is_none());
    // scroll_duration 0: animations still created (nothing visible happens).
    let mut t = truncated_text(50, 120.0);
    t.scroll_duration = 0;
    let m = marquee(target, "label.", &t).unwrap();
    assert_eq!(m[0].duration, 0);
    assert_eq!(m[2].duration, 0);
}
