//! Property setter context shared by every component (`docs/DESIGN-CORE.md`, "Property
//! setting & animation").
//!
//! Every `--set`/`--default`/`--bar` key ends in a component's `set_prop(key, value, cx)`.
//! Animatable setters go through [`PropCx::animate`], which reproduces SketchyBar's
//! `ANIMATE`/`ANIMATE_FLOAT`/`ANIMATE_BYTES` macros (`animation.h`,
//! `docs/spec/events.md` §10.5):
//!
//! * inside `--animate <curve> <n>` with `n > 0`: cancel *locked* animations of the same
//!   (target, setter), queue a new animation from the current value to the target value
//!   (chained after an unlocked one created earlier in the same message), report "no
//!   refresh";
//! * otherwise: cancel **all** animations of the (target, setter) and snap them to their
//!   final values (calling the setter), then call the setter with the new value; report
//!   "changed" if either did.
//!
//! The identity of a (target, setter) pair is `(AnimTarget, path)` where `path` is the
//! canonical dotted property path of the setter relative to the target, e.g.
//! `icon.background.color`, `label.color.hex`, `background.padding_left` (the item-level
//! `padding_left` key uses the same path, because in C both reach
//! `background_set_padding_left` on the same struct). Different syntaxes that reach
//! different C setters stay different paths (`icon.color` vs `icon.color.hex`, Quirk Q10).
//!
//! Every frame the animator produces `(target, path, value)` steps; the runtime resolves the
//! target and calls the owner's `anim_set(path, value, fx)`, which runs the same setter (with
//! its side effects, e.g. `background.color` enabling the background).
//!
//! **Design note.** `DESIGN-CORE.md` sketched `anim_slot(key) -> AnimSlot<&mut field>`.
//! Raw field references cannot run setter side effects (shadow offset recomputation,
//! background enabling, font relayout flags, clip flags, bar resize flags), so components
//! expose `anim_set(path, AnimValue, &mut PropEffects) -> bool` instead.

use crate::animation::{Animator, Curve};
use crate::item::ItemId;
use crate::platform::Resources;
use crate::value;
use std::fmt;

/// A value as stored in a SketchyBar animation (`initial_value`/`final_value` are 32-bit
/// ints; floats are bit-cast, colours are the `0xAARRGGBB` hex).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimValue {
    /// `ANIMATE`: integer interpolation `(int)((1-s)*a + s*b + 0.5)`, exact on the last frame.
    /// Unsigned C fields (`uint32_t`) travel through here as their `int` bit pattern.
    Int(i32),
    /// `ANIMATE_FLOAT`: `(float)((1-s)*a + s*b)`.
    Float(f32),
    /// `ANIMATE_BYTES`: each of the 4 bytes interpolated independently (truncation).
    Color(u32),
}

impl AnimValue {
    /// The value as C `int` (floats are converted, colours reinterpreted).
    pub fn as_i32(self) -> i32 {
        match self {
            AnimValue::Int(v) => v,
            AnimValue::Float(f) => f as i32,
            AnimValue::Color(c) => c as i32,
        }
    }
    /// The value as C `uint32_t` (an `int` argument passed to a `uint32_t` setter).
    pub fn as_u32(self) -> u32 {
        match self {
            AnimValue::Int(v) => v as u32,
            AnimValue::Float(f) => f as u32,
            AnimValue::Color(c) => c,
        }
    }
    pub fn as_f32(self) -> f32 {
        match self {
            AnimValue::Int(v) => v as f32,
            AnimValue::Float(f) => f,
            AnimValue::Color(c) => c as f32,
        }
    }
}

/// Owner of an animated property. Decides what a "changed" frame marks dirty
/// (`docs/spec/events.md` §10.7): an item → that item's `needs_update`; the bar or the
/// default item → `bar_needs_update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimTarget {
    /// `--bar` properties (`g_bar_manager`, including its background).
    Bar,
    /// The `--default` template item.
    Default,
    /// A bar item (stable id, survives reordering).
    Item(ItemId),
}

/// The `--animate <curve> <duration>` setting active for the rest of the current message
/// (`docs/spec/cli.md` §8). `duration` is in 60 Hz frames (D12: `n/60` s wall clock).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimSpec {
    pub curve: Curve,
    pub duration: u32,
}

/// An animation to be added to the [`Animator`] (`animation_setup` + `animator_add`).
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAnim {
    pub target: AnimTarget,
    /// Canonical setter path relative to the target (see module docs).
    pub path: String,
    pub from: AnimValue,
    pub to: AnimValue,
    /// Duration in 60 Hz frames (0 = completes on its first stepped frame).
    pub duration: u32,
    pub curve: Curve,
}

/// Global side effects of setters (fields of `g_bar_manager` in C). Collected per setter
/// call and merged by the runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PropEffects {
    /// The bar background must be redrawn and every bar re-laid-out.
    pub bar_needs_update: bool,
    /// Bar window frames must be recomputed (`margin`, `y_offset`, `height`, `notch_*`,
    /// `position`).
    pub bar_needs_resize: bool,
    /// Some background uses `clip` (sticky, never reset in C).
    pub might_need_clipping: bool,
    /// `image=media.artwork` was used: start receiving media events
    /// (`begin_receiving_media_events`).
    pub begin_media_events: bool,
}

impl PropEffects {
    pub fn merge(&mut self, other: PropEffects) {
        self.bar_needs_update |= other.bar_needs_update;
        self.bar_needs_resize |= other.bar_needs_resize;
        self.might_need_clipping |= other.might_need_clipping;
        self.begin_media_events |= other.begin_media_events;
    }
}

/// `--bar hidden=...` needs the per-display bar list, so it is executed by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenRequest {
    /// `hidden=current`: toggle the bar at index `active_adid - 1` (`bar.md` §2.3 quirk).
    Current,
    /// `hidden=<bool>` (already evaluated against `any_bar_hidden`): apply to all bars.
    All(bool),
}

/// Work a setter cannot do on its own because it touches other items or platform state.
/// The runtime (WP-C) executes these in order right after the `set_prop` call that pushed
/// them, with the item the property was set on as context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropRequest {
    /// `position=` succeeded while the item had a popup `parent`: remove it from
    /// `parent.popup.items` (`popup_remove_item`; `parent` itself stays set, C quirk).
    RemoveFromParentPopup,
    /// `position=p….<host>`: look up `host`; not found → respond
    /// `[!] Item Position (<name>): Item '<host>' is not a valid popup host\n` and **do not**
    /// mark the item dirty; found → `popup_add_item(host.popup, item)`
    /// (`item.md` §2.3, `cli.md` §6.11.1).
    AddToPopup { host: String },
    /// `reset=<anything>`: re-initialise the **default** item (`bar_item_init(&default, NULL)`,
    /// name becomes NULL; `item.md` §1.7 quirk 18).
    ResetDefaultItem,
    /// The item's `provider*` configuration changed (extension, `docs/EXTENSIONS.md`).
    ProviderChanged,
    /// `--bar hidden=...` (`bar.md` §2.3).
    BarHidden(HiddenRequest),
    /// `--bar display|shadow|sticky|topmost` changed: `bar_manager_reset` (recreate all
    /// bar windows; `bar.md` §2.4/§2.5/§6.6).
    ResetBars,
    /// `--bar hide_menubar=<bool>` (extension).
    MenuBarHidden(bool),
    /// `--bar privacy_indicator_inset=on` (extension): start the detection.
    StartPrivacyIndicator,
}

/// An error response of a setter. `Display` yields SketchyBar's exact message text
/// including the `[!]`/`[?]` prefix and the trailing `\n` (`docs/spec/cli.md` §13.1,
/// `docs/spec/components.md`). Item names print `(null)` when unset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropError {
    ItemInvalidProperty {
        item: Option<String>,
        key: String,
    },
    ItemInvalidSubdomain {
        item: Option<String>,
        sub: String,
    },
    NotGraph {
        item: Option<String>,
    },
    NotAlias {
        item: Option<String>,
    },
    NotSlider {
        item: Option<String>,
    },
    /// Extension: `app_menu.*` on a non-`app_menu` item.
    NotAppMenu {
        item: Option<String>,
    },
    ItemPositionInvalidHost {
        item: Option<String>,
        host: String,
    },
    /// D9: `space=` entry ≥ 32 (mbar-specific message).
    ItemInvalidSpace {
        item: Option<String>,
        entry: String,
    },
    /// D9: `display=` entry ≥ 32 (mbar-specific message).
    ItemInvalidDisplay {
        item: Option<String>,
        entry: String,
    },
    /// Extension: unknown `provider=<name>`.
    ItemInvalidProvider {
        item: Option<String>,
        name: String,
    },
    TextInvalidProperty(String),
    TextInvalidSubdomain(String),
    BackgroundInvalidProperty(String),
    BackgroundInvalidSubdomain(String),
    ShadowInvalidProperty(String),
    ShadowInvalidSubdomain(String),
    ImageUnknownProperty(String),
    ImageInvalidSubdomain(String),
    ImageInvalidAppName(String),
    ImageInvalidSpaceId(String),
    /// Prints the part after the first `.` of the value, or `(null)` (C quirk).
    ImageInvalidFormat(Option<String>),
    ImageFileNotFound(String),
    PopupInvalidProperty(String),
    PopupInvalidSubdomain(String),
    GraphInvalidProperty(String),
    GraphInvalidSubdomain(String),
    SliderInvalidProperty(String),
    SliderInvalidSubdomain(String),
    AliasInvalidProperty(String),
    AliasInvalidSubdomain(String),
    ColorInvalidProperty(String),
    /// `--bar clip=…`.
    BarInvalidClip,
    /// D9 / `cli.md` §5: `--bar display=` entry `0`, non-numeric or > 32 (mbar-specific).
    BarInvalidDisplay(String),
    /// Extension: unknown `app_menu.<p>`.
    AppMenuInvalidProperty(String),
    /// Extension: unknown `provider.<p>`.
    ProviderInvalidProperty(String),
}

fn name(n: &Option<String>) -> &str {
    n.as_deref().unwrap_or("(null)")
}

impl fmt::Display for PropError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use PropError::*;
        match self {
            ItemInvalidProperty { item, key } => {
                write!(
                    f,
                    "[!] Item ({}): Invalid property '{}' \n",
                    name(item),
                    key
                )
            }
            ItemInvalidSubdomain { item, sub } => {
                write!(
                    f,
                    "[!] Item ({}): Invalid subdomain '{}'\n",
                    name(item),
                    sub
                )
            }
            NotGraph { item } => write!(
                f,
                "[!] Item ({}): Trying to set a graph property on a non-graph item\n",
                name(item)
            ),
            NotAlias { item } => write!(
                f,
                "[!] Item ({}): Trying to set an alias property on a non-alias item\n",
                name(item)
            ),
            NotSlider { item } => write!(
                f,
                "[!] Item ({}): Trying to set a slider property on a non-slider item\n",
                name(item)
            ),
            NotAppMenu { item } => write!(
                f,
                "[!] Item ({}): Trying to set an app_menu property on a non-app_menu item\n",
                name(item)
            ),
            ItemPositionInvalidHost { item, host } => write!(
                f,
                "[!] Item Position ({}): Item '{}' is not a valid popup host\n",
                name(item),
                host
            ),
            ItemInvalidSpace { item, entry } => {
                write!(f, "[!] Item ({}): Invalid space '{}'\n", name(item), entry)
            }
            ItemInvalidDisplay { item, entry } => {
                write!(
                    f,
                    "[!] Item ({}): Invalid display '{}'\n",
                    name(item),
                    entry
                )
            }
            ItemInvalidProvider { item, name: n } => {
                write!(f, "[!] Item ({}): Invalid provider '{}'\n", name(item), n)
            }
            TextInvalidProperty(k) => write!(f, "[!] Text: Invalid property '{k}'\n"),
            TextInvalidSubdomain(s) => write!(f, "[!] Text: Invalid subdomain '{s}' \n"),
            BackgroundInvalidProperty(k) => write!(f, "[!] Background: Invalid property '{k}'\n"),
            BackgroundInvalidSubdomain(s) => {
                write!(f, "[!] Background: Invalid subdomain '{s}'\n")
            }
            ShadowInvalidProperty(k) => write!(f, "[!] Shadow: Invalid property '{k}'\n"),
            ShadowInvalidSubdomain(s) => write!(f, "[!] Shadow: Invalid subdomain '{s}'\n"),
            ImageUnknownProperty(k) => write!(f, "[?] Image: Unknown property: {k} \n"),
            ImageInvalidSubdomain(s) => write!(f, "[?] Image: Invalid subdomain: {s} \n"),
            ImageInvalidAppName(a) => write!(f, "[!] Image: Invalid application name: '{a}'\n"),
            ImageInvalidSpaceId(s) => write!(f, "[!] Image: Invalid Space ID: '{s}'\n"),
            ImageInvalidFormat(v) => write!(
                f,
                "[!] Image: Invalid Image Format: '{}'\n",
                v.as_deref().unwrap_or("(null)")
            ),
            ImageFileNotFound(p) => write!(f, "[!] Image: File '{p}' not found\n"),
            PopupInvalidProperty(k) => write!(f, "[!] Popup: Invalid property '{k}'\n"),
            PopupInvalidSubdomain(s) => write!(f, "[!] Popup: Invalid subdomain '{s}'\n"),
            GraphInvalidProperty(k) => write!(f, "[!] Graph: Invalid property '{k}'\n"),
            GraphInvalidSubdomain(s) => write!(f, "[!] Graph: Invalid subdomain '{s}'\n"),
            SliderInvalidProperty(k) => write!(f, "[!] Slider: Invalid property '{k}'\n"),
            SliderInvalidSubdomain(s) => write!(f, "[!] Slider: Invalid subdomain '{s}' \n"),
            AliasInvalidProperty(k) => write!(f, "[!] Alias: Invalid property '{k}' \n"),
            AliasInvalidSubdomain(s) => write!(f, "[!] Alias: Invalid subdomain '{s}'\n"),
            ColorInvalidProperty(k) => write!(f, "[?] Color: Invalid property '{k}'\n"),
            BarInvalidClip => write!(f, "[!] Bar: Invalid property 'clip'\n"),
            BarInvalidDisplay(e) => write!(f, "[!] Bar: Invalid display '{e}'\n"),
            AppMenuInvalidProperty(k) => write!(f, "[!] AppMenu: Invalid property '{k}'\n"),
            ProviderInvalidProperty(k) => write!(f, "[!] Provider: Invalid property '{k}'\n"),
        }
    }
}

impl std::error::Error for PropError {}

/// Result of a setter: `Ok(changed)` (C's `needs_refresh`) or an error response.
pub type PropResult = Result<bool, PropError>;

/// Context threaded through every `set_prop` call.
pub struct PropCx<'a> {
    /// Text metrics / image loading (text layout happens at set time because width
    /// animations start from `text_get_length`).
    pub res: &'a mut dyn Resources,
    /// The animator (queueing, locked-cancel, cancel-with-snap happen during parsing).
    pub animator: &'a mut Animator,
    /// The `--animate` setting of the current message; `None` or duration 0 = immediate.
    pub anim: Option<AnimSpec>,
    /// Owner of the properties being set.
    pub target: AnimTarget,
    /// `$HOME` for `~` expansion (`resolve_path`).
    pub home: &'a str,
    /// Non-error response text produced by setters (e.g. `Could not open image file at: …`,
    /// D9 messages). Appended to the IPC response by the runtime, in order.
    pub response: String,
    /// Global side effects (merged into the model by the runtime).
    pub fx: PropEffects,
    /// Work for the runtime (executed in order after the `set_prop` call).
    pub requests: Vec<PropRequest>,
    path: String,
}

impl<'a> PropCx<'a> {
    pub fn new(res: &'a mut dyn Resources, animator: &'a mut Animator, home: &'a str) -> Self {
        PropCx {
            res,
            animator,
            anim: None,
            target: AnimTarget::Bar,
            home,
            response: String::new(),
            fx: PropEffects::default(),
            requests: Vec::new(),
            path: String::new(),
        }
    }

    /// Sets the owner of the following properties and resets the path prefix.
    pub fn set_target(&mut self, target: AnimTarget) {
        self.target = target;
        self.path.clear();
    }

    /// The current path prefix (`""` or ending in `.`).
    pub fn prefix(&self) -> &str {
        &self.path
    }

    /// Descends into a sub-domain (`icon`, `background`, …). Returns a mark for [`leave`].
    ///
    /// [`leave`]: PropCx::leave
    pub fn enter(&mut self, segment: &str) -> usize {
        let mark = self.path.len();
        self.path.push_str(segment);
        self.path.push('.');
        mark
    }

    /// Restores the prefix saved by [`enter`](PropCx::enter).
    pub fn leave(&mut self, mark: usize) {
        self.path.truncate(mark);
    }

    /// Runs `f` inside sub-domain `segment`.
    pub fn scoped<R>(&mut self, segment: &str, f: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.enter(segment);
        let r = f(self);
        self.leave(mark);
        r
    }

    /// Full animation path of `leaf` under the current prefix.
    pub fn path_of(&self, leaf: &str) -> String {
        format!("{}{}", self.path, leaf)
    }

    /// True while `--animate` with a duration > 0 is active.
    pub fn is_animating(&self) -> bool {
        self.anim.is_some_and(|a| a.duration > 0)
    }

    /// The `ANIMATE*` macro. `set` is the C setter (`fn(value, effects) -> changed`).
    /// Returns C's `needs_refresh` contribution: always `false` on the animated path.
    pub fn animate(
        &mut self,
        leaf: &str,
        from: AnimValue,
        to: AnimValue,
        mut set: impl FnMut(AnimValue, &mut PropEffects) -> bool,
    ) -> bool {
        let path = self.path_of(leaf);
        match self.anim {
            Some(spec) if spec.duration > 0 => {
                self.animator.cancel_locked(self.target, &path);
                self.animator.add(PendingAnim {
                    target: self.target,
                    path,
                    from,
                    to,
                    duration: spec.duration,
                    curve: spec.curve,
                });
                false
            }
            _ => {
                let mut changed = false;
                for fin in self.animator.cancel(self.target, &path) {
                    changed |= set(fin, &mut self.fx);
                }
                changed | set(to, &mut self.fx)
            }
        }
    }

    /// `animator_cancel` without a following set: removes all animations of `leaf` and
    /// returns their final values in list order; the caller applies them (snap).
    pub fn cancel(&mut self, leaf: &str) -> Vec<AnimValue> {
        let path = self.path_of(leaf);
        self.animator.cancel(self.target, &path)
    }

    /// A raw `animation_setup` + `animator_add` (no locked-cancel), used for the chained
    /// 0-frame "back to dynamic width" step of `width=dynamic` and animated `string=`.
    pub fn queue_chained(
        &mut self,
        leaf: &str,
        from: AnimValue,
        to: AnimValue,
        duration: u32,
        curve: Curve,
    ) {
        let path = self.path_of(leaf);
        self.animator.add(PendingAnim {
            target: self.target,
            path,
            from,
            to,
            duration,
            curve,
        });
    }

    /// Appends a non-fatal response message (already formatted, incl. `\n`).
    pub fn respond(&mut self, msg: impl fmt::Display) {
        use fmt::Write;
        let _ = write!(self.response, "{msg}");
    }

    pub fn request(&mut self, r: PropRequest) {
        self.requests.push(r);
    }
}

// --- small setter helpers shared by components -------------------------------------

/// `if (field == v) return false; field = v; return true;`
pub fn set_i32(field: &mut i32, v: i32) -> bool {
    if *field == v {
        return false;
    }
    *field = v;
    true
}

pub fn set_u32(field: &mut u32, v: u32) -> bool {
    if *field == v {
        return false;
    }
    *field = v;
    true
}

pub fn set_f32(field: &mut f32, v: f32) -> bool {
    if *field == v {
        return false;
    }
    *field = v;
    true
}

pub fn set_bool(field: &mut bool, v: bool) -> bool {
    if *field == v {
        return false;
    }
    *field = v;
    true
}

/// `token_to_int` for colour values: `(int)strtol(s, NULL, 0)` reinterpreted as `u32`.
pub fn parse_color(v: &str) -> u32 {
    value::parse_int(v) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::HeadlessResources;

    #[test]
    fn error_texts() {
        let e = PropError::ItemInvalidProperty {
            item: Some("foo".into()),
            key: "lazy".into(),
        };
        assert_eq!(e.to_string(), "[!] Item (foo): Invalid property 'lazy' \n");
        let e = PropError::ItemInvalidSubdomain {
            item: None,
            sub: "x".into(),
        };
        assert_eq!(e.to_string(), "[!] Item ((null)): Invalid subdomain 'x'\n");
        assert_eq!(
            PropError::TextInvalidSubdomain("x".into()).to_string(),
            "[!] Text: Invalid subdomain 'x' \n"
        );
        assert_eq!(
            PropError::ImageUnknownProperty("x".into()).to_string(),
            "[?] Image: Unknown property: x \n"
        );
        assert_eq!(
            PropError::ImageInvalidFormat(None).to_string(),
            "[!] Image: Invalid Image Format: '(null)'\n"
        );
        assert_eq!(
            PropError::AliasInvalidProperty("x".into()).to_string(),
            "[!] Alias: Invalid property 'x' \n"
        );
        assert_eq!(
            PropError::BarInvalidClip.to_string(),
            "[!] Bar: Invalid property 'clip'\n"
        );
    }

    #[test]
    fn animate_immediate_and_queued() {
        let mut res = HeadlessResources::default();
        let mut anim = Animator::new();
        let mut field = 0i32;
        {
            let mut cx = PropCx::new(&mut res, &mut anim, "/home/u");
            cx.set_target(AnimTarget::Bar);
            assert!(
                cx.animate("margin", AnimValue::Int(0), AnimValue::Int(5), |v, _| {
                    set_i32(&mut field, v.as_i32())
                })
            );
        }
        assert_eq!(field, 5);
        {
            let mut cx = PropCx::new(&mut res, &mut anim, "/home/u");
            cx.anim = Some(AnimSpec {
                curve: Curve::Linear,
                duration: 30,
            });
            let mark = cx.enter("icon");
            assert!(
                !cx.animate("y_offset", AnimValue::Int(5), AnimValue::Int(9), |v, _| {
                    set_i32(&mut field, v.as_i32())
                })
            );
            assert_eq!(cx.path_of("x"), "icon.x");
            cx.leave(mark);
            assert_eq!(cx.prefix(), "");
        }
        assert_eq!(field, 5);
        assert_eq!(anim.len(), 1);
        // Immediate set on the same path snaps (to 9) and then sets 7.
        let mut snapped = Vec::new();
        {
            let mut cx = PropCx::new(&mut res, &mut anim, "/home/u");
            cx.enter("icon");
            assert!(
                cx.animate("y_offset", AnimValue::Int(5), AnimValue::Int(7), |v, _| {
                    snapped.push(v.as_i32());
                    set_i32(&mut field, v.as_i32())
                })
            );
        }
        assert_eq!(snapped, vec![9, 7]);
        assert_eq!(field, 7);
        assert!(anim.is_empty());
    }
}
