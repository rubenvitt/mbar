//! Animation curves and the animator (`docs/spec/events.md` §10, `docs/spec/cli.md` §8).
//!
//! * [`Curve`] and [`interpolate`] are complete (formulas from `animation.c` /
//!   `misc/helpers.h`; D15 adds real `bounce`/`overshoot` curves).
//! * [`Animator`] queueing semantics (`animator_add` chaining, `animator_cancel_locked`,
//!   `animator_cancel` with snap, `animator_lock`, D18 cancel-on-remove) are complete because
//!   the property layer depends on them.
//! * Frame stepping ([`Animator::step`], [`Animator::next_deadline`]) and the marquee
//!   ([`marquee`]) implement `events.md` §10.3–10.6/§10.9.
//!
//! Time base (D12): duration `n` = `n/60` s of wall-clock time measured with
//! [`Instant`]s supplied by the runtime; frames are produced at the display refresh rate.

use crate::color::Color;
use crate::props::{AnimTarget, AnimValue, PendingAnim};
use std::time::{Duration, Instant};

/// One frame of the 60 Hz time base (D12): the pacing interval for platforms without a
/// display link (headless), see [`Animator::next_deadline_paced`].
pub const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

/// Interpolation curve, selected by the **first character** of the `--animate` curve token
/// (`events.md` §10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    /// `l` and every unknown first character (including empty): `x`.
    #[default]
    Linear,
    /// `q`: `x²`.
    Quadratic,
    /// `t`: `0.52·tanh(2·atanh(1/1.04)·(x−0.5)) + 0.5`.
    Tanh,
    /// `s`: `sin(π/2·x)`.
    Sin,
    /// `e`: `x·e^(x−1)`.
    Exp,
    /// `c`: `sqrt(1−(x−1)²)` with C's f32 intermediates.
    Circ,
    /// `b`: ease-out-bounce (D15; linear in SketchyBar).
    Bounce,
    /// `o`: ease-out-back with overshoot 1.70158 (D15; linear in SketchyBar).
    Overshoot,
}

impl Curve {
    /// Curve for the first byte of the curve token (`'\0'` for an empty token).
    pub fn from_byte(b: u8) -> Curve {
        match b {
            b'l' => Curve::Linear,
            b'q' => Curve::Quadratic,
            b't' => Curve::Tanh,
            b's' => Curve::Sin,
            b'e' => Curve::Exp,
            b'c' => Curve::Circ,
            b'b' => Curve::Bounce,
            b'o' => Curve::Overshoot,
            _ => Curve::Linear,
        }
    }

    /// Curve for a full `--animate` token (`linear`, `sin`, `smooth` = sin, …).
    pub fn from_token(token: &str) -> Curve {
        Curve::from_byte(crate::value::first_byte(token))
    }

    /// `f(x)` for `x ∈ [0, 1]`, computed in f64.
    pub fn eval(self, x: f64) -> f64 {
        match self {
            Curve::Linear => x,
            Curve::Quadratic => x * x,
            Curve::Tanh => {
                let a = 0.52f64;
                a * (2.0 * (1.0 / (2.0 * a)).atanh() * (x - 0.5)).tanh() + 0.5
            }
            Curve::Sin => (std::f64::consts::FRAC_PI_2 * x).sin(),
            Curve::Exp => x * (x - 1.0).exp(),
            Curve::Circ => {
                // C: sqrt(1.f - powf(x - 1.f, 2.f)) — f32 intermediates, f64 sqrt.
                let d = (x - 1.0) as f32;
                let s = 1.0f32 - d * d;
                (s as f64).sqrt()
            }
            Curve::Bounce => {
                let n1 = 7.5625;
                let d1 = 2.75;
                if x < 1.0 / d1 {
                    n1 * x * x
                } else if x < 2.0 / d1 {
                    let x = x - 1.5 / d1;
                    n1 * x * x + 0.75
                } else if x < 2.5 / d1 {
                    let x = x - 2.25 / d1;
                    n1 * x * x + 0.9375
                } else {
                    let x = x - 2.625 / d1;
                    n1 * x * x + 0.984375
                }
            }
            Curve::Overshoot => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                let y = x - 1.0;
                1.0 + c3 * y * y * y + c1 * y * y
            }
        }
    }
}

/// One interpolation step (`animation_update`): `s` is the curve output (`1.0` on the
/// final frame, where the curve is bypassed).
///
/// * `Int`: `(int)((1-s)*a + s*b + 0.5)` (C truncation toward zero), exactly `b` when
///   `final_frame`.
/// * `Float`: `(float)((1-s)*a + s*b)` computed in f64.
/// * `Color`: per byte `(u8)((1-s)*a_i + s*b_i)`.
///
/// Mixed kinds use the `from` kind; the property layer never mixes them.
pub fn interpolate(from: AnimValue, to: AnimValue, s: f64, final_frame: bool) -> AnimValue {
    match from {
        AnimValue::Int(a) => {
            let b = to.as_i32();
            if final_frame {
                AnimValue::Int(b)
            } else {
                AnimValue::Int(((1.0 - s) * a as f64 + s * b as f64 + 0.5) as i32)
            }
        }
        AnimValue::Float(a) => {
            let b = to.as_f32();
            AnimValue::Float(((1.0 - s) * a as f64 + s * b as f64) as f32)
        }
        AnimValue::Color(a) => AnimValue::Color(Color::lerp_bytes(a, to.as_u32(), s)),
    }
}

/// A queued or running animation (`struct animation`).
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub id: u64,
    pub target: AnimTarget,
    pub path: String,
    pub from: AnimValue,
    pub to: AnimValue,
    /// 60 Hz frames; seconds = `duration / 60`.
    pub duration: u32,
    pub curve: Curve,
    /// Set for every animation at the end of each message (`animator_lock`).
    pub locked: bool,
    /// Chained successor waiting for its predecessor to finish.
    pub waiting: bool,
    /// Time of the first frame this animation was stepped (`initial_time`).
    pub start: Option<Instant>,
    /// Successor in a chain (`next`).
    pub next: Option<u64>,
}

/// A value the runtime must apply this frame: resolve `target`, call the owner's
/// `anim_set(path, value, fx)`; a `true` result marks the owner dirty
/// (`events.md` §10.7).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimStep {
    pub target: AnimTarget,
    pub path: String,
    pub value: AnimValue,
}

/// All animations, in insertion order (= stepping order).
#[derive(Debug, Default)]
pub struct Animator {
    animations: Vec<Animation>,
    next_id: u64,
    /// Time of the last [`Animator::step`] (pacing base of [`Animator::next_deadline_paced`]).
    last_step: Option<Instant>,
}

impl Animator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.animations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.animations.is_empty()
    }

    pub fn animations(&self) -> &[Animation] {
        &self.animations
    }

    /// `animator_add` + `animator_calculate_offset_for_animation`: if any animation with the
    /// same (target, path) exists (searching from the end), the new one starts from its
    /// final value and waits for it.
    pub fn add(&mut self, p: PendingAnim) {
        let id = self.next_id;
        self.next_id += 1;
        let mut anim = Animation {
            id,
            target: p.target,
            path: p.path,
            from: p.from,
            to: p.to,
            duration: p.duration,
            curve: p.curve,
            locked: false,
            waiting: false,
            start: None,
            next: None,
        };
        if let Some(prev) = self
            .animations
            .iter_mut()
            .rev()
            .find(|a| a.target == anim.target && a.path == anim.path)
        {
            anim.from = prev.to;
            prev.next = Some(id);
            anim.waiting = true;
        }
        self.animations.push(anim);
    }

    /// `animator_cancel_locked`: removes locked animations of (target, path) without
    /// applying their final values.
    pub fn cancel_locked(&mut self, target: AnimTarget, path: &str) {
        self.remove_where(|a| a.locked && a.target == target && a.path == path);
    }

    /// `animator_cancel`: removes **all** animations of (target, path) and returns their
    /// final values in list order; the caller applies them through the setter (snap).
    pub fn cancel(&mut self, target: AnimTarget, path: &str) -> Vec<AnimValue> {
        let finals = self
            .animations
            .iter()
            .filter(|a| a.target == target && a.path == path)
            .map(|a| a.to)
            .collect();
        self.remove_where(|a| a.target == target && a.path == path);
        finals
    }

    /// `animator_lock`: called at the end of every message.
    pub fn lock_all(&mut self) {
        for a in &mut self.animations {
            a.locked = true;
        }
    }

    /// D18: drop every animation owned by a removed item (no final values applied).
    pub fn cancel_target(&mut self, target: AnimTarget) {
        self.remove_where(|a| a.target == target);
    }

    /// `animator_destroy` (hotload / reload / exit): drop everything.
    pub fn clear(&mut self) {
        self.animations.clear();
    }

    fn remove_where(&mut self, pred: impl Fn(&Animation) -> bool) {
        let removed: Vec<u64> = self
            .animations
            .iter()
            .filter(|a| pred(a))
            .map(|a| a.id)
            .collect();
        if removed.is_empty() {
            return;
        }
        self.animations.retain(|a| !removed.contains(&a.id));
        // `animator_remove` unlinks the removed animation from its neighbours; a successor
        // of a removed animation is released so it does not wait forever.
        for a in &mut self.animations {
            if let Some(n) = a.next {
                if removed.contains(&n) {
                    a.next = None;
                }
            }
        }
        let still_linked: Vec<u64> = self.animations.iter().filter_map(|a| a.next).collect();
        for a in &mut self.animations {
            if a.waiting && !still_linked.contains(&a.id) {
                a.waiting = false;
            }
        }
    }

    /// One display frame (`animator_update`, `events.md` §10.3/§10.6), at time `now`:
    ///
    /// ```text
    /// for a in animations (insertion order):
    ///     skip if a.waiting
    ///     if a.start is None: a.start = now
    ///     t = duration > 0 ? (now - start) / (duration/60 s) : 1.0
    ///     final = t >= 1.0;  t = clamp(t, 0, 1)
    ///     s = final ? 1.0 : curve.eval(t)
    ///     push AnimStep { value: interpolate(from, to, s, final) }
    ///     if final: release a.next (waiting = false; it is stepped in this same frame
    ///               because successors are always later in the list) and remove a
    /// ```
    ///
    /// Returns the steps in order; the runtime applies them (`Runtime::apply_anim_steps`).
    pub fn step(&mut self, now: Instant) -> Vec<AnimStep> {
        self.last_step = Some(now);
        let mut steps = Vec::new();
        let mut finished: Vec<u64> = Vec::new();
        // Index loop: releasing a successor mutates a later element of the same vector.
        for i in 0..self.animations.len() {
            let a = &mut self.animations[i];
            if a.waiting {
                continue;
            }
            let start = *a.start.get_or_insert(now);
            let t = if a.duration > 0 {
                let elapsed = now.saturating_duration_since(start).as_secs_f64();
                elapsed / (a.duration as f64 / 60.0)
            } else {
                1.0
            };
            let final_frame = t >= 1.0;
            let t = t.clamp(0.0, 1.0);
            let s = if final_frame { 1.0 } else { a.curve.eval(t) };
            steps.push(AnimStep {
                target: a.target,
                path: a.path.clone(),
                value: interpolate(a.from, a.to, s, final_frame),
            });
            if final_frame {
                finished.push(a.id);
                if let Some(next) = a.next.take() {
                    if let Some(n) = self.animations.iter_mut().find(|n| n.id == next) {
                        n.waiting = false;
                    }
                }
            }
        }
        if !finished.is_empty() {
            self.animations.retain(|a| !finished.contains(&a.id));
        }
        steps
    }

    /// True while any animation exists (the platform should deliver display-synced frames).
    pub fn needs_frame(&self) -> bool {
        !self.animations.is_empty()
    }

    /// Next instant a frame is needed: `Some(now)` while any animation exists (every frame
    /// may change a value; display-link pacing at the refresh rate is the platform's job,
    /// D12), `None` when idle (`animator_update` destroys the display link once the list is
    /// empty). Platforms without a display link use [`Animator::next_deadline_paced`].
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        if self.animations.is_empty() {
            None
        } else {
            Some(now)
        }
    }

    /// [`Animator::next_deadline`] for platforms without a display link (headless): one
    /// `interval` after the last stepped frame instead of "now", so a main loop that sleeps
    /// until the deadline produces frames at `1 / interval` instead of busy-spinning for the
    /// whole animation (or forever with `scroll_texts`). Durations are wall-clock (D12), so
    /// pacing does not change the animated values, only how often they are sampled. "Now"
    /// when nothing was stepped yet or the last step is more than `interval` ago.
    pub fn next_deadline_paced(&self, now: Instant, interval: Duration) -> Option<Instant> {
        self.next_deadline(now).map(|d| match self.last_step {
            Some(last) => d.max(last + interval),
            None => d,
        })
    }

    /// True while any animation is in flight (alias of [`Animator::needs_frame`]).
    pub fn is_active(&self) -> bool {
        self.needs_frame()
    }
}

/// `text_animate_scroll` (`components.md` §4.10, `events.md` §10.9): the marquee of one
/// text, or `None` when its preconditions fail (`max_chars == 0`, `scroll != 0`,
/// `has_const_width && custom_width < width`, `width == 0`, `width == bounds.w`). Returns the
/// three chained float animations of `<prefix>scroll` (prefix e.g. `icon.`,
/// `slider.knob.`): linear `0 → bounds.w` over `(u32)(scroll_duration * bounds.w / width)`
/// frames, a 0-frame jump to `-width`, linear `-width → 0` over `scroll_duration` frames.
/// The runtime adds them with the `ANIMATE_FLOAT` semantics (cancel locked, then add);
/// Quirk Q8 (resetting `--animate` for the rest of the message) is the runtime's call.
pub fn marquee(
    target: AnimTarget,
    prefix: &str,
    text: &crate::components::Text,
) -> Option<Vec<PendingAnim>> {
    if text.max_chars == 0 || text.scroll != 0.0 {
        return None;
    }
    if text.has_const_width && text.custom_width < text.width {
        return None;
    }
    let width = text.width as f64;
    let full = text.bounds.width as f64;
    if text.width == 0 || width == full {
        return None;
    }
    let path = format!("{prefix}scroll");
    let anim = |from: f32, to: f32, duration: u32| PendingAnim {
        target,
        path: path.clone(),
        from: AnimValue::Float(from),
        to: AnimValue::Float(to),
        duration,
        curve: Curve::Linear,
    };
    // C: `(uint32_t)(scroll_duration * (bounds.size.width / width))` in double.
    let out_frames = (text.scroll_duration as f64 * (full / width)) as u32;
    let neg = -(text.width as f32);
    Some(vec![
        anim(text.scroll, text.bounds.width, out_frames),
        anim(text.bounds.width, neg, 0),
        anim(neg, 0.0, text.scroll_duration),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pend(path: &str, from: i32, to: i32) -> PendingAnim {
        PendingAnim {
            target: AnimTarget::Bar,
            path: path.into(),
            from: AnimValue::Int(from),
            to: AnimValue::Int(to),
            duration: 30,
            curve: Curve::Linear,
        }
    }

    #[test]
    fn curves() {
        assert_eq!(Curve::from_token("smooth"), Curve::Sin);
        assert_eq!(Curve::from_token("elastic"), Curve::Exp);
        assert_eq!(Curve::from_token(""), Curve::Linear);
        assert_eq!(Curve::from_token("bounce"), Curve::Bounce);
        for c in [
            Curve::Linear,
            Curve::Quadratic,
            Curve::Tanh,
            Curve::Sin,
            Curve::Exp,
            Curve::Circ,
            Curve::Bounce,
            Curve::Overshoot,
        ] {
            assert!(c.eval(0.0).abs() < 1e-6, "{c:?}");
            assert!((c.eval(1.0) - 1.0).abs() < 1e-6, "{c:?}");
        }
        assert!(Curve::Overshoot.eval(0.8) > 1.0);
    }

    #[test]
    fn interpolation_kinds() {
        assert_eq!(
            interpolate(AnimValue::Int(0), AnimValue::Int(10), 0.25, false),
            AnimValue::Int(3)
        );
        // Truncation toward zero for negatives: -2.7 + 0.5 = -2.2 -> -2.
        assert_eq!(
            interpolate(AnimValue::Int(0), AnimValue::Int(-10), 0.32, false),
            AnimValue::Int(-2)
        );
        assert_eq!(
            interpolate(AnimValue::Int(0), AnimValue::Int(7), 0.99, true),
            AnimValue::Int(7)
        );
        assert_eq!(
            interpolate(
                AnimValue::Color(0),
                AnimValue::Color(0xff0000ff),
                0.5,
                false
            ),
            AnimValue::Color(0x7f00007f)
        );
        assert_eq!(
            interpolate(AnimValue::Float(0.0), AnimValue::Float(1.0), 0.5, false),
            AnimValue::Float(0.5)
        );
    }

    #[test]
    fn chaining_locking_cancel() {
        let mut a = Animator::new();
        a.add(pend("y_offset", 0, 10));
        a.add(pend("y_offset", 99, 0));
        assert_eq!(a.animations()[1].from, AnimValue::Int(10));
        assert!(a.animations()[1].waiting);
        a.lock_all();
        a.cancel_locked(AnimTarget::Bar, "y_offset");
        assert!(a.is_empty());
        a.add(pend("margin", 0, 5));
        a.add(pend("y_offset", 0, 3));
        assert_eq!(a.cancel(AnimTarget::Bar, "margin"), vec![AnimValue::Int(5)]);
        assert_eq!(a.len(), 1);
        a.cancel_target(AnimTarget::Bar);
        assert!(a.is_empty());
    }
}
