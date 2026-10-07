//! Subscribable events and their payloads (`docs/spec/events.md` §3, §5, §6;
//! `docs/spec/cli.md` §7, §12.2).
//!
//! * [`EventKind`], [`EventMask`] and the [`CustomEvents`] registry core are complete
//!   (the property layer, `--subscribe`, `--query events` and the runtime all depend on the
//!   bit layout).
//! * INFO/env builders are WP-D.

use crate::script::EnvVars;

/// The 18 built-in events, in registration order; the discriminant is the bit index
/// (`custom_events.c:custom_events_init`). Custom events get bits 18..63.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EventKind {
    FrontAppSwitched = 0,
    SpaceChange = 1,
    DisplayChange = 2,
    SystemWoke = 3,
    MouseEntered = 4,
    MouseExited = 5,
    MouseClicked = 6,
    MouseScrolled = 7,
    SystemWillSleep = 8,
    MouseEnteredGlobal = 9,
    MouseExitedGlobal = 10,
    MouseScrolledGlobal = 11,
    VolumeChange = 12,
    BrightnessChange = 13,
    PowerSourceChange = 14,
    WifiChange = 15,
    MediaChange = 16,
    SpaceWindowsChange = 17,
}

impl EventKind {
    pub const ALL: [EventKind; 18] = [
        EventKind::FrontAppSwitched,
        EventKind::SpaceChange,
        EventKind::DisplayChange,
        EventKind::SystemWoke,
        EventKind::MouseEntered,
        EventKind::MouseExited,
        EventKind::MouseClicked,
        EventKind::MouseScrolled,
        EventKind::SystemWillSleep,
        EventKind::MouseEnteredGlobal,
        EventKind::MouseExitedGlobal,
        EventKind::MouseScrolledGlobal,
        EventKind::VolumeChange,
        EventKind::BrightnessChange,
        EventKind::PowerSourceChange,
        EventKind::WifiChange,
        EventKind::MediaChange,
        EventKind::SpaceWindowsChange,
    ];

    /// Subscribable name (`$SENDER`).
    pub fn name(self) -> &'static str {
        match self {
            EventKind::FrontAppSwitched => "front_app_switched",
            EventKind::SpaceChange => "space_change",
            EventKind::DisplayChange => "display_change",
            EventKind::SystemWoke => "system_woke",
            EventKind::MouseEntered => "mouse.entered",
            EventKind::MouseExited => "mouse.exited",
            EventKind::MouseClicked => "mouse.clicked",
            EventKind::MouseScrolled => "mouse.scrolled",
            EventKind::SystemWillSleep => "system_will_sleep",
            EventKind::MouseEnteredGlobal => "mouse.entered.global",
            EventKind::MouseExitedGlobal => "mouse.exited.global",
            EventKind::MouseScrolledGlobal => "mouse.scrolled.global",
            EventKind::VolumeChange => "volume_change",
            EventKind::BrightnessChange => "brightness_change",
            EventKind::PowerSourceChange => "power_source_change",
            EventKind::WifiChange => "wifi_change",
            EventKind::MediaChange => "media_change",
            EventKind::SpaceWindowsChange => "space_windows_change",
        }
    }

    /// `1 << index`.
    pub fn bit(self) -> u64 {
        1u64 << (self as u8)
    }

    pub fn from_name(name: &str) -> Option<EventKind> {
        EventKind::ALL.into_iter().find(|e| e.name() == name)
    }
}

/// An item's subscriptions (`update_mask`, serialized as decimal u64).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EventMask(pub u64);

impl EventMask {
    pub fn contains(self, bit: u64) -> bool {
        self.0 & bit != 0
    }
    pub fn has(self, kind: EventKind) -> bool {
        self.contains(kind.bit())
    }
    pub fn insert(&mut self, bit: u64) {
        self.0 |= bit;
    }
}

/// One registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDef {
    pub name: String,
    /// `NSDistributedNotificationCenter` name; `None` prints `(null)`.
    pub notification: Option<String>,
}

/// Outcome of `--add event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendResult {
    /// New event with this bit; the runtime observes the notification if any.
    Added(u64),
    /// Name already registered (built-in or custom): silently ignored (Q18).
    Exists,
    /// All 64 bits in use (mbar error instead of C UB): `[!] Event: Too many events '<name>'\n`.
    Full,
}

/// Ordered event registry (`struct custom_events`). Rebuilt with only the built-ins on
/// hotload/reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEvents {
    events: Vec<EventDef>,
}

impl Default for CustomEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl CustomEvents {
    /// The 18 built-ins.
    pub fn new() -> Self {
        CustomEvents {
            events: EventKind::ALL
                .iter()
                .map(|e| EventDef {
                    name: e.name().to_string(),
                    notification: None,
                })
                .collect(),
        }
    }

    pub fn events(&self) -> &[EventDef] {
        &self.events
    }

    /// `custom_events_get_flag_for_name`: `1 << index` or `None` (C: 0).
    pub fn flag(&self, name: &str) -> Option<u64> {
        self.events
            .iter()
            .position(|e| e.name == name)
            .map(|i| 1u64 << i)
    }

    /// `custom_events_append` (`events.md` §3.2).
    pub fn append(&mut self, name: &str, notification: Option<&str>) -> AppendResult {
        if self.flag(name).is_some() {
            return AppendResult::Exists;
        }
        if self.events.len() >= 64 {
            return AppendResult::Full;
        }
        self.events.push(EventDef {
            name: name.to_string(),
            notification: notification.map(str::to_string),
        });
        AppendResult::Added(1u64 << (self.events.len() - 1))
    }

    /// First custom event whose notification equals `notification`
    /// (`bar_manager_handle_notification`; D19: delivered once).
    pub fn name_for_notification(&self, notification: &str) -> Option<&str> {
        self.events
            .iter()
            .find(|e| e.notification.as_deref() == Some(notification))
            .map(|e| e.name.as_str())
    }
}

/// An event delivery: the name subscribers are matched against (`$SENDER`) and the
/// event-specific env (`INFO` etc.) in insertion order. `env == None` reproduces
/// SketchyBar's NULL env (see `script::build_update_env`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventInfo {
    pub name: String,
    pub env: Option<EnvVars>,
}

impl EventInfo {
    pub fn new(name: impl Into<String>, env: Option<EnvVars>) -> Self {
        EventInfo { name: name.into(), env }
    }
}

/// One bar's entry for the `space_change` INFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarSpace {
    pub adid: u32,
    pub sid: u32,
}

/// `space_change` INFO (`events.md` §5.2): `"{\n" + "\t\"display-<adid>\": <sid>" lines
/// joined with ",\n" + "\n}"` — no trailing newline; zero bars → `"{\n}"`. Full string, no
/// truncation (B2).
pub fn space_change_info(bars: &[BarSpace]) -> String {
    let _ = bars;
    todo!("WP-D: events.md §5.2")
}

/// `display_change` INFO: active adid as decimal (no 2-char truncation, B3).
pub fn display_change_info(active_adid: u32) -> String {
    let _ = active_adid;
    todo!("WP-D: events.md §5.3")
}

/// `volume_change` / `brightness_change` INFO: `(int)(v*100 + 0.5)`.
pub fn level_info(v: f32) -> String {
    let _ = v;
    todo!("WP-D: events.md §5.6/§5.7")
}

/// `get_modifier_description`: comma-joined subset of `shift,ctrl,alt,cmd,fn` for
/// `kCGEventFlagMaskShift (0x20000)`, `Control (0x40000)`, `Alternate (0x80000)`,
/// `Command (0x100000)`, `SecondaryFn (0x800000)`, or `none`.
pub fn modifier_description(flags: u32) -> String {
    let _ = flags;
    todo!("WP-D: events.md §6.2")
}

/// Env of a click (`bar_item_on_click`, `events.md` §6.2), in order `INFO`, `BUTTON`,
/// `MODIFIER`. INFO is
/// `"{\n\t\"button\": \"<b>\",\n\t\"button_code\": <n>,\n\t\"modifier\": \"<m>\",\n\t\"modfier_code\": <flags>\n}\n"`
/// (typo `modfier_code` is part of the format).
pub fn click_env(button: crate::platform::MouseButton, button_code: u32, flags: u32) -> EnvVars {
    let _ = (button, button_code, flags);
    todo!("WP-D: events.md §6.2")
}

/// Env of an item scroll (`bar_item_on_scroll`): `INFO`, `SCROLL_DELTA`, `MODIFIER`, INFO
/// `"{\n\t\"delta\": <d>,\n\t\"modifier\": \"<m>\",\n\t\"modfier_code\": <flags>\n}\n"`.
pub fn scroll_env(delta: i32, flags: u32) -> EnvVars {
    let _ = (delta, flags);
    todo!("WP-D: events.md §6.3")
}

/// Env of `mouse.scrolled.global`: `SCROLL_DELTA`, `INFO` (same format), `DID=<adid>`,
/// `MODIFIER`.
pub fn scroll_global_env(delta: i32, adid: u32, flags: u32) -> EnvVars {
    let _ = (delta, adid, flags);
    todo!("WP-D: events.md §6.3")
}

/// `--trigger <event> K=V…` env: tokens with `=` and a **non-empty** value, split at the
/// first `=`, later duplicates win (`events.md` §7).
pub fn trigger_env(tokens: &[String]) -> EnvVars {
    let _ = tokens;
    todo!("WP-D: events.md §7")
}

/// Event names whose `--trigger` runs the forced OS handler and ignores the passed vars
/// (`events.md` §7, Q21).
pub fn is_forced_trigger(name: &str) -> bool {
    matches!(
        name,
        "space_change"
            | "display_change"
            | "space_windows_change"
            | "volume_change"
            | "media_change"
            | "wifi_change"
            | "power_source_change"
    )
}

/// Scroll coalescing state (`events.md` §6.3): 150 ms leading-edge throttle, an
/// accumulation older than 300 ms is dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollThrottle {
    pub last: Option<std::time::Instant>,
    pub acc: i32,
}

impl ScrollThrottle {
    /// Returns `Some(total)` when the event is delivered (the caller resets `acc = 0` after
    /// dispatch via [`ScrollThrottle::reset`]), `None` when it was swallowed.
    pub fn feed(&mut self, delta: i32, now: std::time::Instant) -> Option<i32> {
        let _ = (delta, now);
        todo!("WP-D: events.md §6.3")
    }

    pub fn reset(&mut self) {
        self.acc = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry() {
        let mut ev = CustomEvents::new();
        assert_eq!(ev.flag("space_windows_change"), Some(131072));
        assert_eq!(ev.flag("mouse.clicked"), Some(64));
        assert_eq!(ev.append("my_event", Some("com.x")), AppendResult::Added(1 << 18));
        assert_eq!(ev.append("my_event", None), AppendResult::Exists);
        assert_eq!(ev.append("space_change", None), AppendResult::Exists);
        assert_eq!(ev.name_for_notification("com.x"), Some("my_event"));
        for i in 0..45 {
            assert!(matches!(ev.append(&format!("e{i}"), None), AppendResult::Added(_)));
        }
        assert_eq!(ev.append("overflow", None), AppendResult::Full);
        assert_eq!(EventKind::from_name("mouse.exited.global"), Some(EventKind::MouseExitedGlobal));
        assert_eq!(EventKind::MediaChange.bit(), 65536);
    }
}
