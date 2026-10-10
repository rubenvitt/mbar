# Privacy Indicator Awareness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** mbar knows when macOS shows its privacy dot (microphone, camera, screen/system-audio capture), where it is and which apps caused it; it fires `privacy_indicator_change` and, with `privacy_indicator_inset=on`, moves the bar's right-hand items out of the dot's way.

**Architecture:** All logic is platform independent in `mbar-core` (`privacy` module: log-line parser, state, event payload, query JSON, and a `Tracker` state machine for timing/merging) and unit tested on any host. The runtime wires event, bar property, query, reload and lazy start like the AeroSpace integration; `layout::horizontal_pass` applies the inset. A thin macOS worker (`mbar-macos/src/sys/privacy.rs`) runs `/usr/bin/log stream`, looks at `CGWindowList` when the `Tracker` asks and posts samples through the `Sink`.

**Tech Stack:** Rust (workspace), serde_json, objc2-core-graphics 0.3 (`CGWindowListCopyWindowInfo`), `/usr/bin/log`, mlua (docs test).

**Spec:** `docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`

## Where each task can run

| Host | Tasks |
|---|---|
| any (Linux or macOS) | 1, 2, 3, 4, 6 |
| macOS required | 5, 7 |

The workspace's `cargo test --workspace` compiles `mbar-macos` only on macOS. On Linux, tasks 1–4 and 6 run with `cargo test -p mbar-core` / `-p mbar-lua`.

## Global Constraints

- Event name: `privacy_indicator_change`. Bar property: `privacy_indicator_inset=on|off`, default `off`. Query keyword: `privacy_indicator`.
- Script variables: `VISIBLE` (`on`/`off`), `MIC`, `CAMERA`, `SCREEN`, `AUDIO`, `LOCATION` (comma-separated, sorted bundle ids; empty when none).
- `INFO` keys: `visible`, `frame` (`{x,y,w,h}` integers, omitted while hidden), `mic`, `camera`, `screen`, `audio`, `location` (arrays), `attribution` (`on`/`off`). `--query privacy_indicator` adds `active` and `inset`.
- Log kinds → lists: `mic`→mic, `cam`→camera, `scr`→screen, `aud`→audio, `loc`→location; other kinds ignored.
- Log predicate (exact): `subsystem == "com.apple.controlcenter" AND category == "sensor-indicators" AND (eventMessage BEGINSWITH "Active activity attributions changed to " OR eventMessage BEGINSWITH "Sorted active attributions from SystemStatus update: ")`.
- Indicator window: owner `Window Server`, layer `2147483630`, `0 < width, height <= 64` pt.
- `--query bar` output must stay byte-identical to SketchyBar's (the `default_query` test in `bar.rs` pins it). The inset is **not** added there.
- Nothing starts before the first `--subscribe` to the event or `privacy_indicator_inset=on`; `--query privacy_indicator` never starts it.
- Code, identifiers, comments and docs in English (repo convention). Commit messages in English, ending with the attribution lines below.

Every commit message ends with:

```
Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01TFbFwBojJj8T6Zp3UGdojW
```

## Review Focus

1. **Indicator frame shifts for a while** (probe: 2025 → 2029 for ~10 s → 2025, cause unknown, possibly the menu bar revealed on hover): the stored frame must end at the returned value, not stay at 2029 → `Tracker` test `moving_frame_settles` (Task 2).
2. **`log show` result arriving after a live `log stream` line** must not overwrite it → `Tracker` test `history_after_live_line_is_ignored` (Task 2).
3. **Dot already visible at launch** (always-on ARK) must not flip `attribution` off → `Tracker` test `visible_at_startup_does_not_trip_format_check` (Task 2).
4. **`log stream` missing or exiting repeatedly**: backoff to 30 s, `attribution` off, window check every 2 s so the inset keeps working → `Tracker` test `stream_exits_back_off_and_poll_fast` (Task 2).
5. **Indicator left edge closer to the bar's left edge than `padding_right`** (`indicator.x - bar.x < padding_right`): no panic or wrap; right items follow SketchyBar's overflow rule → layout test `inset_smaller_than_padding_does_not_panic` (Task 4).

---

### Task 1: Core `privacy` module: parser, sample, state, payloads

**Files:**
- Create: `crates/mbar-core/src/privacy.rs`
- Modify: `crates/mbar-core/src/lib.rs` (add `pub mod privacy;` after `pub mod popup;`)

**Interfaces:**
- Produces:
  - `pub const EVENT_NAME: &str = "privacy_indicator_change";`
  - `pub const LOG_PREDICATE: &str` (the exact predicate above)
  - `pub struct Attributions { pub mic, pub camera, pub screen, pub audio, pub location: Vec<String> }` (`Debug, Clone, PartialEq, Eq, Default`)
  - `pub fn parse_log_message(msg: &str) -> Option<Attributions>`
  - `pub enum StreamLine { Attributions(Attributions), Unparsed, Other }` and `pub fn classify_stream_line(line: &str) -> StreamLine`
  - `pub struct PrivacySample { pub visible: bool, pub frames: Vec<Rect>, pub attributions: Option<Attributions> }` (`Debug, Clone, PartialEq, Default`)
  - `pub struct PrivacyState { pub active: bool, pub known: bool, pub visible: bool, pub frames: Vec<Rect>, pub attributions: Attributions, pub attribution: bool }` (`Debug, Clone, PartialEq, Default`)
  - `pub struct PrivacyChange { pub any: bool, pub geometry: bool }`
  - `impl PrivacyState { pub fn apply(&mut self, s: PrivacySample) -> PrivacyChange; pub fn frame(&self) -> Option<Rect>; pub fn env(&self) -> Vec<(String, String)>; pub fn info_json(&self) -> String; pub fn to_json(&self, inset: bool) -> String }`

- [ ] **Step 1: Write the module with its tests first (tests at the bottom), implementation stubs returning defaults**

Create `crates/mbar-core/src/privacy.rs` with this test module (the implementation follows in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn a(mic: &[&str], camera: &[&str], screen: &[&str], audio: &[&str], location: &[&str]) -> Attributions {
        let v = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        Attributions {
            mic: v(mic),
            camera: v(camera),
            screen: v(screen),
            audio: v(audio),
            location: v(location),
        }
    }

    // Real lines from `log show`, macOS 27.0.1, 2026-10-10.
    #[test]
    fn parses_change_lines() {
        assert_eq!(
            parse_log_message(r#"Active activity attributions changed to ["aud:com.rogueamoeba.arkaudiod", "cam:com.apple.PhotoBooth", "mic:com.goodsnooze.MacWhisper"]"#),
            Some(a(&["com.goodsnooze.MacWhisper"], &["com.apple.PhotoBooth"], &[], &["com.rogueamoeba.arkaudiod"], &[]))
        );
        // Duplicated entry (seen for CleanShot), sorted output.
        assert_eq!(
            parse_log_message(r#"Active activity attributions changed to ["scr:pl.maketheweb.cleanshotx", "aud:com.rogueamoeba.arkaudiod", "scr:pl.maketheweb.cleanshotx", "scr:app.cotypist.Cotypist"]"#),
            Some(a(&[], &[], &["app.cotypist.Cotypist", "pl.maketheweb.cleanshotx"], &["com.rogueamoeba.arkaudiod"], &[]))
        );
        assert_eq!(
            parse_log_message("Active activity attributions changed to []"),
            Some(Attributions::default())
        );
        assert_eq!(
            parse_log_message(r#"Active activity attributions changed to ["loc:com.apple.weather", "xyz:com.example.new"]"#),
            Some(a(&[], &[], &[], &[], &["com.apple.weather"]))
        );
    }

    #[test]
    fn parses_sorted_lines() {
        assert_eq!(
            parse_log_message("Sorted active attributions from SystemStatus update: [[mic] MacWhisper (com.goodsnooze.MacWhisper), [aud] Audio Routing Kit (ARK) (com.rogueamoeba.arkaudiod), [scr] MacWhisper (com.goodsnooze.MacWhisper)]"),
            Some(a(&["com.goodsnooze.MacWhisper"], &[], &["com.goodsnooze.MacWhisper"], &["com.rogueamoeba.arkaudiod"], &[]))
        );
        assert_eq!(
            parse_log_message("Sorted active attributions from SystemStatus update: [[loc] Fantastical Helper (85C27NK92C.com.flexibits.fantastical2.mac.helper)]"),
            Some(a(&[], &[], &[], &[], &["85C27NK92C.com.flexibits.fantastical2.mac.helper"]))
        );
        assert_eq!(
            parse_log_message("Sorted active attributions from SystemStatus update: []"),
            Some(Attributions::default())
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_log_message("Active activity attributions changed to [broken"), None);
        assert_eq!(parse_log_message("Sorted active attributions from SystemStatus update: [aud] x (y)"), None);
        assert_eq!(parse_log_message("Sorted active attributions from SystemStatus update: [[aud] no bundle]"), None);
        assert_eq!(parse_log_message("Dependent controller changed: sensor indicators"), None);
    }

    #[test]
    fn classifies_stream_lines() {
        let line = r#"{"eventMessage":"Active activity attributions changed to [\"cam:com.apple.PhotoBooth\"]","subsystem":"com.apple.controlcenter"}"#;
        assert_eq!(
            classify_stream_line(line),
            StreamLine::Attributions(a(&[], &["com.apple.PhotoBooth"], &[], &[], &[]))
        );
        assert_eq!(
            classify_stream_line(r#"Filtering the log data using "subsystem == "com.apple.controlcenter" AND category == "sensor-indicators"""#),
            StreamLine::Other
        );
        let renamed = r#"{"eventMessage":"Active activity attributions changed to {\"mic\":[]}"}"#;
        assert_eq!(classify_stream_line(renamed), StreamLine::Unparsed);
        assert_eq!(classify_stream_line(r#"{"eventMessage":"unrelated"}"#), StreamLine::Other);
        assert_eq!(classify_stream_line(""), StreamLine::Other);
    }

    fn sample(visible: bool, frames: &[Rect], at: Option<Attributions>) -> PrivacySample {
        PrivacySample { visible, frames: frames.to_vec(), attributions: at }
    }

    #[test]
    fn apply_reports_changes() {
        let f = Rect::new(2025.0, 3.0, 28.0, 28.0);
        let mut st = PrivacyState::default();
        let c = st.apply(sample(true, &[f], Some(a(&["m"], &[], &[], &[], &[]))));
        assert!(c.any && c.geometry);
        assert!(st.known && st.visible && st.attribution);
        let c = st.apply(sample(true, &[f], Some(a(&["m"], &[], &[], &[], &[]))));
        assert!(!c.any && !c.geometry);
        let c = st.apply(sample(true, &[f], Some(a(&["m", "n"], &[], &[], &[], &[]))));
        assert!(c.any && !c.geometry);
        // Unknown attributions: lists cleared, flag off.
        let c = st.apply(sample(true, &[f], None));
        assert!(c.any && !c.geometry);
        assert!(!st.attribution && st.attributions == Attributions::default());
        let c = st.apply(sample(false, &[], None));
        assert!(c.any && c.geometry);
    }

    #[test]
    fn env_info_and_query() {
        let mut st = PrivacyState::default();
        st.apply(sample(
            true,
            &[Rect::new(2025.0, 3.0, 28.0, 28.0), Rect::new(2025.0, 3.0, 28.0, 28.0)],
            Some(a(&["b", "a"], &[], &[], &["ark"], &[])),
        ));
        let env: std::collections::HashMap<_, _> = st.env().into_iter().collect();
        assert_eq!(env["VISIBLE"], "on");
        assert_eq!(env["MIC"], "b,a");
        assert_eq!(env["CAMERA"], "");
        assert_eq!(env["AUDIO"], "ark");
        let info: serde_json::Value = serde_json::from_str(&st.info_json()).unwrap();
        assert_eq!(info["visible"], "on");
        assert_eq!(info["frame"], serde_json::json!({"x": 2025, "y": 3, "w": 28, "h": 28}));
        assert_eq!(info["mic"], serde_json::json!(["b", "a"]));
        assert_eq!(info["screen"], serde_json::json!([]));
        assert_eq!(info["attribution"], "on");
        assert!(info.get("active").is_none());

        st.apply(sample(false, &[], None));
        let info: serde_json::Value = serde_json::from_str(&st.info_json()).unwrap();
        assert!(info.get("frame").is_none(), "frame omitted while hidden");
        assert_eq!(info["attribution"], "off");

        st.active = true;
        let q = st.to_json(true);
        assert!(q.ends_with("}\n"), "{q}");
        assert!(q.contains("\n\t\"active\": \"on\""), "{q}");
        let q: serde_json::Value = serde_json::from_str(&q).unwrap();
        assert_eq!(q["inset"], "on");
        assert_eq!(q["visible"], "off");
    }
}
```

Above the tests, add the module header and empty stubs so the file compiles:

```rust
//! Privacy indicator awareness (`docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`):
//! Control Center's log lines, the state behind `privacy_indicator_change`,
//! `--query privacy_indicator` and the bar's `privacy_indicator_inset`, and the
//! [`Tracker`] that times the platform's window checks.

use crate::geometry::Rect;
use crate::value::format_bool;
use serde::Serialize;
use serde_json::{json, Map, Value};
```

(Declare the types and functions from **Interfaces** with `todo!()` bodies for this step only.)

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p mbar-core --lib privacy::`
Expected: compiles, every test panics with `not yet implemented`.

- [ ] **Step 3: Implement**

Replace the stubs with:

```rust
/// The built-in event.
pub const EVENT_NAME: &str = "privacy_indicator_change";

const CHANGED_PREFIX: &str = "Active activity attributions changed to ";
const SORTED_PREFIX: &str = "Sorted active attributions from SystemStatus update: ";

/// `log stream` / `log show` predicate for Control Center's attribution lines
/// (subsystem `com.apple.controlcenter`, category `sensor-indicators`).
pub const LOG_PREDICATE: &str = "subsystem == \"com.apple.controlcenter\" AND category == \"sensor-indicators\" AND (eventMessage BEGINSWITH \"Active activity attributions changed to \" OR eventMessage BEGINSWITH \"Sorted active attributions from SystemStatus update: \")";

/// Apps per sensor, each list sorted and without duplicates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attributions {
    pub mic: Vec<String>,
    pub camera: Vec<String>,
    pub screen: Vec<String>,
    pub audio: Vec<String>,
    pub location: Vec<String>,
}

impl Attributions {
    /// Adds `bundle` to the list of Control Center's `kind` (`mic`, `cam`, `scr`, `aud`,
    /// `loc`); other kinds are ignored.
    fn push(&mut self, kind: &str, bundle: &str) {
        let list = match kind {
            "mic" => &mut self.mic,
            "cam" => &mut self.camera,
            "scr" => &mut self.screen,
            "aud" => &mut self.audio,
            "loc" => &mut self.location,
            _ => return,
        };
        let bundle = bundle.trim();
        if !bundle.is_empty() {
            list.push(bundle.to_string());
        }
    }

    fn normalized(mut self) -> Self {
        for list in [
            &mut self.mic,
            &mut self.camera,
            &mut self.screen,
            &mut self.audio,
            &mut self.location,
        ] {
            list.sort();
            list.dedup();
        }
        self
    }

    /// `(variable, INFO key, list)` in output order.
    fn fields(&self) -> [(&'static str, &'static str, &[String]); 5] {
        [
            ("MIC", "mic", self.mic.as_slice()),
            ("CAMERA", "camera", self.camera.as_slice()),
            ("SCREEN", "screen", self.screen.as_slice()),
            ("AUDIO", "audio", self.audio.as_slice()),
            ("LOCATION", "location", self.location.as_slice()),
        ]
    }
}

/// Parses one of Control Center's two messages (design §Findings):
/// `Active activity attributions changed to ["aud:<bundle>", …]` and
/// `Sorted active attributions from SystemStatus update: [[aud] <name> (<bundle>), …]`.
/// In the second form the bundle id is the last parenthesised group of an entry
/// (names may contain parentheses: `Audio Routing Kit (ARK)`). `None` for any other text.
pub fn parse_log_message(msg: &str) -> Option<Attributions> {
    let msg = msg.trim();
    let mut out = Attributions::default();
    if let Some(rest) = msg.strip_prefix(CHANGED_PREFIX) {
        let entries: Vec<String> = serde_json::from_str(rest).ok()?;
        for e in &entries {
            let (kind, bundle) = e.split_once(':')?;
            out.push(kind, bundle);
        }
        return Some(out.normalized());
    }
    let inner = msg
        .strip_prefix(SORTED_PREFIX)?
        .strip_prefix('[')?
        .strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(out);
    }
    for (i, part) in inner.split(", [").enumerate() {
        let entry = if i == 0 { part.strip_prefix('[')? } else { part };
        let (kind, rest) = entry.split_once(']')?;
        let open = rest.rfind('(')?;
        let bundle = rest[open + 1..].strip_suffix(')')?;
        out.push(kind, bundle);
    }
    Some(out.normalized())
}

/// One line of `log stream` / `log show` with `--style ndjson`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamLine {
    Attributions(Attributions),
    /// The message starts with one of the two prefixes but the rest does not parse:
    /// the format changed.
    Unparsed,
    /// Anything else (the leading `Filtering the log data using …` line, …).
    Other,
}

pub fn classify_stream_line(line: &str) -> StreamLine {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return StreamLine::Other;
    };
    let Some(msg) = v.get("eventMessage").and_then(Value::as_str) else {
        return StreamLine::Other;
    };
    match parse_log_message(msg) {
        Some(a) => StreamLine::Attributions(a),
        None if msg.starts_with(CHANGED_PREFIX) || msg.starts_with(SORTED_PREFIX) => {
            StreamLine::Unparsed
        }
        None => StreamLine::Other,
    }
}

/// What the platform reports (`Input::PrivacyIndicator`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PrivacySample {
    pub visible: bool,
    /// The indicator windows (global points, top-left origin), sorted, no duplicates.
    pub frames: Vec<Rect>,
    /// `None`: unknown (the log stream is not running or its lines stopped parsing).
    pub attributions: Option<Attributions>,
}

/// `Model::privacy`. Survives `--reload`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PrivacyState {
    /// `PlatformRequest::StartPrivacyIndicator` was emitted.
    pub active: bool,
    /// A sample arrived.
    pub known: bool,
    pub visible: bool,
    pub frames: Vec<Rect>,
    pub attributions: Attributions,
    /// The attributions are known (`INFO` `attribution`).
    pub attribution: bool,
}

/// What [`PrivacyState::apply`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyChange {
    /// Anything (fire the event).
    pub any: bool,
    /// `visible` or the frames (lay the bars out again).
    pub geometry: bool,
}

impl PrivacyState {
    pub fn apply(&mut self, s: PrivacySample) -> PrivacyChange {
        let attribution = s.attributions.is_some();
        let attributions = s.attributions.unwrap_or_default();
        let geometry = self.visible != s.visible || self.frames != s.frames;
        let any = geometry
            || self.attribution != attribution
            || self.attributions != attributions;
        self.known = true;
        self.visible = s.visible;
        self.frames = s.frames;
        self.attribution = attribution;
        self.attributions = attributions;
        PrivacyChange { any, geometry }
    }

    /// Bounding box of the indicator windows while visible.
    pub fn frame(&self) -> Option<Rect> {
        if !self.visible {
            return None;
        }
        let r = self.frames.iter().fold(Rect::ZERO, |acc, f| acc.union(f));
        (!r.is_empty()).then_some(r)
    }

    /// Script variables besides `NAME`/`SENDER`/`INFO`.
    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![("VISIBLE".to_string(), format_bool(self.visible).to_string())];
        for (var, _, list) in self.attributions.fields() {
            env.push((var.to_string(), list.join(",")));
        }
        env
    }

    fn info_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("visible".into(), format_bool(self.visible).into());
        if let Some(f) = self.frame() {
            m.insert(
                "frame".into(),
                json!({
                    "x": f.x.round() as i64,
                    "y": f.y.round() as i64,
                    "w": f.width.round() as i64,
                    "h": f.height.round() as i64,
                }),
            );
        }
        for (_, key, list) in self.attributions.fields() {
            m.insert(key.into(), json!(list));
        }
        m.insert("attribution".into(), format_bool(self.attribution).into());
        Value::Object(m)
    }

    /// `INFO` (compact JSON).
    pub fn info_json(&self) -> String {
        self.info_value().to_string()
    }

    /// `--query privacy_indicator`: `INFO` plus `active` and `inset`, TAB indented,
    /// trailing newline.
    pub fn to_json(&self, inset: bool) -> String {
        let mut v = self.info_value();
        v["active"] = format_bool(self.active).into();
        v["inset"] = format_bool(inset).into();
        let mut out = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b"\t");
        let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
        v.serialize(&mut ser).expect("JSON value serializes");
        let mut s = String::from_utf8(out).expect("serde_json writes UTF-8");
        s.push('\n');
        s
    }
}
```

Add `pub mod privacy;` to `crates/mbar-core/src/lib.rs` after `pub mod popup;`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mbar-core --lib privacy::`
Expected: all 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/mbar-core/src/privacy.rs crates/mbar-core/src/lib.rs
git commit -m "feat(core): privacy indicator log parser, state and payloads"
```

---

### Task 2: Core `Tracker`: timing and merging rules

**Files:**
- Modify: `crates/mbar-core/src/privacy.rs` (append the `Tracker` section above `#[cfg(test)]`, append tests to the test module)

**Interfaces:**
- Consumes: `Attributions`, `PrivacySample` (Task 1), `Rect`.
- Produces:
  - `pub enum TrackerInput { StreamStarted, StreamExited, Line(Attributions), Unparsed, History(Option<Attributions>), Windows(Vec<Rect>), Nudge }`
  - `pub struct Tracker` with `pub fn new(now: Instant) -> Tracker`, `pub fn handle(&mut self, input: TrackerInput, now: Instant)`, `pub fn restart_at(&self) -> Option<Instant>`, `pub fn next_check(&self) -> Option<Instant>`, `pub fn ready(&self) -> bool`, `pub fn sample(&self) -> PrivacySample`
  - constants `BURST`, `POLL`, `POLL_SLOW`, `LINE_WINDOW`, `BACKOFF_MIN`, `BACKOFF_MAX`

- [ ] **Step 1: Write the failing tests** (append inside `mod tests`)

```rust
    use std::time::{Duration, Instant};

    fn ms(t: Instant, ms: u64) -> Instant {
        t + Duration::from_millis(ms)
    }
    fn f(x: f32) -> Rect {
        Rect::new(x, 3.0, 28.0, 28.0)
    }
    fn started(t: Instant) -> Tracker {
        let mut tr = Tracker::new(t);
        tr.handle(TrackerInput::StreamStarted, t);
        tr
    }

    #[test]
    fn starts_by_spawning_and_checking() {
        let t = Instant::now();
        let tr = Tracker::new(t);
        assert_eq!(tr.restart_at(), Some(t));
        assert_eq!(tr.next_check(), Some(t));
        assert!(!tr.ready());
    }

    #[test]
    fn history_after_live_line_is_ignored() {
        let t = Instant::now();
        let new = a(&["new"], &[], &[], &[], &[]);
        let old = a(&["old"], &[], &[], &[], &[]);
        let mut tr = started(t);
        tr.handle(TrackerInput::Line(new.clone()), t);
        tr.handle(TrackerInput::History(Some(old.clone())), ms(t, 500));
        tr.handle(TrackerInput::Windows(vec![]), ms(t, 600));
        assert_eq!(tr.sample().attributions, Some(new));
        // Before any live line the history applies.
        let mut tr = started(t);
        tr.handle(TrackerInput::History(Some(old.clone())), t);
        tr.handle(TrackerInput::Windows(vec![]), t);
        assert_eq!(tr.sample().attributions, Some(old));
    }

    #[test]
    fn line_schedules_a_burst_of_checks() {
        let t = Instant::now();
        let mut tr = started(t);
        tr.handle(TrackerInput::Windows(vec![]), t);
        tr.handle(TrackerInput::Line(Attributions::default()), ms(t, 5_000));
        for at in [5_000, 5_300, 6_000, 7_000] {
            assert_eq!(tr.next_check(), Some(ms(t, at)));
            tr.handle(TrackerInput::Windows(vec![]), ms(t, at));
        }
        // Then only the safety poll, 10 s after the last check.
        assert_eq!(tr.next_check(), Some(ms(t, 17_000)));
    }

    #[test]
    fn moving_frame_settles() {
        let t = Instant::now();
        let mut tr = started(t);
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), t);
        tr.handle(TrackerInput::Line(Attributions::default()), ms(t, 1_000));
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 1_000));
        tr.handle(TrackerInput::Windows(vec![f(2029.0)]), ms(t, 1_300));
        // Changed: checked again 2 s later (and the burst continues).
        tr.handle(TrackerInput::Windows(vec![f(2029.0)]), ms(t, 2_000));
        tr.handle(TrackerInput::Windows(vec![f(2029.0)]), ms(t, 3_000));
        tr.handle(TrackerInput::Windows(vec![f(2029.0)]), ms(t, 3_300));
        assert_eq!(tr.next_check(), Some(ms(t, 13_300)), "stable: safety poll");
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 13_300));
        assert_eq!(tr.sample().frames, vec![f(2025.0)]);
        assert_eq!(tr.next_check(), Some(ms(t, 15_300)), "changed again: re-check in 2 s");
    }

    #[test]
    fn visible_at_startup_does_not_trip_format_check() {
        let t = Instant::now();
        let mut tr = started(t);
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), t);
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 10_000));
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 20_000));
        assert!(tr.sample().attributions.is_some());
    }

    #[test]
    fn renamed_message_trips_format_check() {
        let t = Instant::now();
        let mut tr = started(t);
        tr.handle(TrackerInput::Windows(vec![]), t);
        // The safety poll finds the dot; no line came.
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 10_000));
        assert_eq!(tr.next_check(), Some(ms(t, 12_000)), "frames changed: re-check");
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 12_000));
        assert_eq!(tr.next_check(), Some(ms(t, 13_000)), "format check 3 s after");
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 13_000));
        assert_eq!(tr.sample().attributions, None);
        // The next parsed line turns it back on.
        tr.handle(TrackerInput::Line(Attributions::default()), ms(t, 20_000));
        assert!(tr.sample().attributions.is_some());
    }

    #[test]
    fn working_stream_does_not_trip_format_check() {
        let t = Instant::now();
        let mut tr = started(t);
        tr.handle(TrackerInput::Windows(vec![]), t);
        // The change line comes first, the window right after.
        tr.handle(TrackerInput::Line(a(&[], &["cam"], &[], &[], &[])), ms(t, 9_500));
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 9_500));
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 12_500));
        assert!(tr.sample().attributions.is_some());
        // A line seen shortly before the previous (hidden) check counts too.
        let mut tr = started(t);
        tr.handle(TrackerInput::Windows(vec![]), t);
        tr.handle(TrackerInput::Line(Attributions::default()), ms(t, 8_000));
        tr.handle(TrackerInput::Windows(vec![]), ms(t, 10_000));
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 20_000));
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 23_000));
        assert!(tr.sample().attributions.is_some());
    }

    #[test]
    fn unparsed_line_trips_at_once() {
        let t = Instant::now();
        let mut tr = started(t);
        tr.handle(TrackerInput::Windows(vec![]), t);
        tr.handle(TrackerInput::Unparsed, ms(t, 1_000));
        assert_eq!(tr.sample().attributions, None);
        assert_eq!(tr.next_check(), Some(ms(t, 1_000)), "and the window is checked");
    }

    #[test]
    fn stream_exits_back_off_and_poll_fast() {
        let t = Instant::now();
        let mut tr = Tracker::new(t);
        tr.handle(TrackerInput::StreamExited, t);
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), t);
        assert_eq!(tr.sample().attributions, None);
        assert_eq!(tr.next_check(), Some(ms(t, 2_000)), "2 s while the stream is down");
        let mut expected = [1u64, 2, 4, 8, 16, 30, 30].into_iter();
        let mut now = t;
        assert_eq!(tr.restart_at(), Some(ms(now, 1_000 * expected.next().unwrap())));
        for delay in expected {
            now = tr.restart_at().unwrap();
            tr.handle(TrackerInput::StreamStarted, now);
            tr.handle(TrackerInput::StreamExited, now);
            assert_eq!(tr.restart_at(), Some(ms(now, 1_000 * delay)));
        }
        // A parsed line resets the backoff.
        tr.handle(TrackerInput::StreamStarted, now);
        tr.handle(TrackerInput::Line(Attributions::default()), now);
        tr.handle(TrackerInput::StreamExited, now);
        assert_eq!(tr.restart_at(), Some(ms(now, 1_000)));
    }

    #[test]
    fn nudge_checks_now_and_frames_are_normalized() {
        let t = Instant::now();
        let mut tr = started(t);
        tr.handle(TrackerInput::Windows(vec![f(2025.0), f(2025.0), Rect::ZERO]), t);
        assert_eq!(tr.sample().frames, vec![f(2025.0)]);
        assert!(tr.ready() && tr.sample().visible);
        tr.handle(TrackerInput::Nudge, ms(t, 4_000));
        assert_eq!(tr.next_check(), Some(ms(t, 4_000)));
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p mbar-core --lib privacy::`
Expected: compile error `cannot find type Tracker`.

- [ ] **Step 3: Implement** (insert above `#[cfg(test)]`)

```rust
// ----------------------------------------------------------------------------------
// Tracker (design §mbar-core: `Tracker`)
// ----------------------------------------------------------------------------------

use std::time::{Duration, Instant};

/// Window checks after a parsed line: the window fades in and out (about 0.8 s).
pub const BURST: [Duration; 4] = [
    Duration::from_millis(0),
    Duration::from_millis(300),
    Duration::from_millis(1_000),
    Duration::from_millis(2_000),
];
/// Re-check after frames changed, and the safety poll while the stream is down.
pub const POLL: Duration = Duration::from_secs(2);
/// Safety poll while the stream runs (bounds a stale frame, notices a dot that
/// Control Center stopped logging).
pub const POLL_SLOW: Duration = Duration::from_secs(10);
/// A hidden→visible change needs a parsed line within this distance (format check).
pub const LINE_WINDOW: Duration = Duration::from_secs(3);
pub const BACKOFF_MIN: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// What the platform tells the [`Tracker`].
#[derive(Debug, Clone, PartialEq)]
pub enum TrackerInput {
    /// `log stream` was spawned.
    StreamStarted,
    /// `log stream` exited or could not be spawned.
    StreamExited,
    /// A parsed line from `log stream`.
    Line(Attributions),
    /// A line with a known prefix that did not parse.
    Unparsed,
    /// The newest parsed line of the `log show` run after a spawn (`None`: none found).
    History(Option<Attributions>),
    /// The indicator windows found by a window check.
    Windows(Vec<Rect>),
    /// Display reconfiguration or wake: check the windows now.
    Nudge,
}

/// The detection's rules, driven with explicit times so they are testable anywhere.
/// The platform spawns `log stream` at [`Tracker::restart_at`], looks at the windows at
/// [`Tracker::next_check`] and posts [`Tracker::sample`] when it changed.
#[derive(Debug, Clone)]
pub struct Tracker {
    stream_running: bool,
    restart_at: Option<Instant>,
    backoff: Duration,
    /// A stream line (or the history) parsed since the stream (re)started.
    line_parsed: bool,
    last_line_at: Option<Instant>,
    attributions: Attributions,
    /// The format check failed; cleared by the next parsed line.
    drift: bool,
    /// At least one window check ran.
    checked: bool,
    last_check_at: Option<Instant>,
    visible: bool,
    frames: Vec<Rect>,
    /// Pending window checks.
    checks: Vec<Instant>,
    /// Hidden→visible seen: `(earliest acceptable line, evaluate at)`.
    pending_format_check: Option<(Instant, Instant)>,
}

impl Tracker {
    pub fn new(now: Instant) -> Tracker {
        Tracker {
            stream_running: false,
            restart_at: Some(now),
            backoff: BACKOFF_MIN,
            line_parsed: false,
            last_line_at: None,
            attributions: Attributions::default(),
            drift: false,
            checked: false,
            last_check_at: None,
            visible: false,
            frames: Vec::new(),
            checks: vec![now],
            pending_format_check: None,
        }
    }

    pub fn handle(&mut self, input: TrackerInput, now: Instant) {
        match input {
            TrackerInput::StreamStarted => {
                self.stream_running = true;
                self.restart_at = None;
                self.line_parsed = false;
                self.drift = false;
            }
            TrackerInput::StreamExited => {
                self.stream_running = false;
                self.pending_format_check = None;
                self.restart_at = Some(now + self.backoff);
                self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
            }
            TrackerInput::Line(a) => {
                self.attributions = a;
                self.line_parsed = true;
                self.last_line_at = Some(now);
                self.drift = false;
                self.backoff = BACKOFF_MIN;
                for d in BURST {
                    self.schedule(now + d);
                }
            }
            TrackerInput::Unparsed => {
                if self.stream_running {
                    self.drift = true;
                }
                self.schedule(now);
            }
            TrackerInput::History(Some(a)) if !self.line_parsed => {
                self.attributions = a;
                self.line_parsed = true;
            }
            TrackerInput::History(_) => {}
            TrackerInput::Windows(frames) => self.windows(normalize(frames), now),
            TrackerInput::Nudge => self.schedule(now),
        }
    }

    fn schedule(&mut self, at: Instant) {
        if !self.checks.contains(&at) {
            self.checks.push(at);
        }
    }

    fn windows(&mut self, frames: Vec<Rect>, now: Instant) {
        self.checks.retain(|t| *t > now);
        let visible = !frames.is_empty();
        if !visible {
            self.pending_format_check = None;
        } else if !self.visible && self.checked && self.stream_running {
            let prev = self.last_check_at.unwrap_or(now);
            let since = prev.checked_sub(LINE_WINDOW).unwrap_or(prev);
            let at = now + LINE_WINDOW;
            self.pending_format_check = Some((since, at));
            self.schedule(at);
        }
        if let Some((since, at)) = self.pending_format_check {
            if now >= at {
                self.pending_format_check = None;
                if self.stream_running && !self.last_line_at.is_some_and(|l| l >= since) {
                    self.drift = true;
                }
            }
        }
        if self.checked && frames != self.frames {
            self.schedule(now + POLL);
        }
        self.visible = visible;
        self.frames = frames;
        self.checked = true;
        self.last_check_at = Some(now);
    }

    /// When to spawn `log stream` (again).
    pub fn restart_at(&self) -> Option<Instant> {
        self.restart_at
    }

    /// When to look at the windows next.
    pub fn next_check(&self) -> Option<Instant> {
        let interval = if self.stream_running { POLL_SLOW } else { POLL };
        let poll = self.last_check_at.map(|t| t + interval);
        self.checks.iter().copied().chain(poll).min()
    }

    /// A window check ran (before that, [`Tracker::sample`] knows nothing).
    pub fn ready(&self) -> bool {
        self.checked
    }

    pub fn sample(&self) -> PrivacySample {
        PrivacySample {
            visible: self.visible,
            frames: self.frames.clone(),
            attributions: (self.stream_running && !self.drift).then(|| self.attributions.clone()),
        }
    }
}

/// Non-empty frames, sorted, without duplicates.
fn normalize(mut frames: Vec<Rect>) -> Vec<Rect> {
    frames.retain(|f| !f.is_empty());
    frames.sort_by(|a, b| {
        (a.x, a.y, a.width, a.height)
            .partial_cmp(&(b.x, b.y, b.width, b.height))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    frames.dedup();
    frames
}
```

Move the `use std::time::{Duration, Instant};` line to the top `use` block of the file (clippy flags `use` after items otherwise), and remove the duplicate `use` from the test module.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mbar-core --lib privacy::`
Expected: all 16 tests pass. If `moving_frame_settles` fails on the `13_300` assertion, check that `windows()` drops due checks (`retain(|t| *t > now)`) before scheduling new ones.

- [ ] **Step 5: Commit**

```bash
git add crates/mbar-core/src/privacy.rs
git commit -m "feat(core): privacy indicator tracker (check timing, history order, format check, backoff)"
```

---

### Task 3: Runtime wiring: event, property, query, reload, lazy start

**Files:**
- Modify: `crates/mbar-core/src/platform.rs` (`Input`, `PlatformRequest`)
- Modify: `crates/mbar-core/src/props.rs` (`PropRequest`)
- Modify: `crates/mbar-core/src/bar.rs` (`BarProps` field, default, `set_prop` arm)
- Modify: `crates/mbar-core/src/model.rs` (`Model::privacy`)
- Modify: `crates/mbar-core/src/command.rs` (`QueryTarget::PrivacyIndicator`, keyword)
- Modify: `crates/mbar-core/src/query.rs` (dispatch)
- Modify: `crates/mbar-core/src/runtime.rs`
- Modify: `crates/mbar-macos/src/platform/services.rs` (exhaustive `execute`: interim no-op arm, replaced in Task 5)
- Create: `crates/mbar-core/tests/wpb_privacy.rs`

**Interfaces:**
- Consumes: `privacy::{EVENT_NAME, PrivacySample, PrivacyState}` (Task 1).
- Produces: `Input::PrivacyIndicator(PrivacySample)`, `PlatformRequest::StartPrivacyIndicator`, `PropRequest::StartPrivacyIndicator`, `BarProps::privacy_indicator_inset: bool`, `Model::privacy: PrivacyState`, `QueryTarget::PrivacyIndicator`.

- [ ] **Step 1: Write the failing integration tests**

Create `crates/mbar-core/tests/wpb_privacy.rs`:

```rust
//! Privacy indicator in the core runtime: built-in `privacy_indicator_change`, lazy
//! `StartPrivacyIndicator`, `privacy_indicator_inset`, `--query privacy_indicator`
//! and `--reload` (`docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`).

mod wpc_common;
use mbar_core::geometry::Rect;
use mbar_core::platform::{Effect, Input, PlatformRequest};
use mbar_core::privacy::{Attributions, PrivacySample};
use wpc_common::*;

const DOT: Rect = Rect::new(1892.0, 0.0, 28.0, 100.0);

fn sample(visible: bool, mic: &[&str]) -> Input {
    Input::PrivacyIndicator(PrivacySample {
        visible,
        frames: if visible { vec![DOT] } else { vec![] },
        attributions: Some(Attributions {
            mic: mic.iter().map(|s| s.to_string()).collect(),
            ..Attributions::default()
        }),
    })
}

fn starts(fx: &[Effect]) -> usize {
    platform(fx)
        .iter()
        .filter(|p| matches!(p, PlatformRequest::StartPrivacyIndicator))
        .count()
}

fn feed(h: &mut H, input: Input) -> Vec<Effect> {
    let fx = h.input(input);
    h.frame();
    fx
}

fn add_watcher(h: &mut H, name: &str) -> Vec<Effect> {
    h.msg_fx(&[
        "--add", "item", name, "right",
        "--set", name, "script=dot.sh",
        "--subscribe", name, "privacy_indicator_change",
    ])
    .1
}

#[test]
fn subscription_registers_the_event_and_starts_once() {
    let mut h = H::new();
    let (rsp, fx) = h.msg_fx(&[
        "--add", "item", "a", "right",
        "--set", "a", "script=dot.sh",
        "--subscribe", "a", "privacy_indicator_change",
    ]);
    assert_eq!(rsp.unwrap_or_default(), "", "no '[?] Event: ... not found'");
    assert_eq!(starts(&fx), 1);
    assert!(h.query(&["events"]).get("privacy_indicator_change").is_some());
    assert_eq!(starts(&add_watcher(&mut h, "b")), 0, "started once");
}

#[test]
fn inset_property_starts_detection_once() {
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&["--bar", "privacy_indicator_inset=on"]);
    assert_eq!(starts(&fx), 1);
    h.msg(&["--bar", "privacy_indicator_inset=off"]);
    let (_, fx) = h.msg_fx(&["--bar", "privacy_indicator_inset=on"]);
    assert_eq!(starts(&fx), 0);
    // `--query bar` stays SketchyBar's output.
    assert!(h.query(&["bar"]).get("privacy_indicator_inset").is_none());
}

#[test]
fn query_reports_without_starting() {
    let mut h = H::new();
    let (text, fx) = h.msg_fx(&["--query", "privacy_indicator"]);
    assert_eq!(starts(&fx), 0);
    let q: serde_json::Value = serde_json::from_str(&text.unwrap()).unwrap();
    assert_eq!(q["active"], "off");
    assert_eq!(q["visible"], "off");
    assert_eq!(q["inset"], "off");
    assert_eq!(q["attribution"], "off");
    // An item with that name wins.
    h.msg(&["--add", "item", "privacy_indicator", "left"]);
    assert_eq!(h.query(&["privacy_indicator"])["name"], "privacy_indicator");
}

#[test]
fn sample_triggers_the_event_once_per_change() {
    let mut h = H::new();
    add_watcher(&mut h, "a");
    let fx = feed(&mut h, sample(true, &["com.b", "com.a"]));
    let r = runs_of(&fx, "a");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].sender(), Some("privacy_indicator_change"));
    assert_eq!(r[0].get("VISIBLE"), Some("on"));
    assert_eq!(r[0].get("MIC"), Some("com.b,com.a"));
    assert_eq!(r[0].get("SCREEN"), Some(""));
    let info: serde_json::Value = serde_json::from_str(r[0].get("INFO").unwrap()).unwrap();
    assert_eq!(info["frame"]["x"], 1892);
    assert_eq!(info["attribution"], "on");

    assert!(runs(&feed(&mut h, sample(true, &["com.b", "com.a"]))).is_empty(), "unchanged");
    let fx = feed(&mut h, sample(false, &[]));
    assert_eq!(runs_of(&fx, "a")[0].get("VISIBLE"), Some("off"));
    assert_eq!(h.query(&["privacy_indicator"])["visible"], "off");
}

#[test]
fn late_subscriber_gets_the_state_once() {
    let mut h = H::new();
    h.msg(&["--bar", "privacy_indicator_inset=on"]);
    feed(&mut h, sample(true, &["com.a"]));
    let fx = add_watcher(&mut h, "late");
    let r = runs_of(&fx, "late");
    assert_eq!(r.len(), 1, "{fx:?}");
    assert_eq!(r[0].get("MIC"), Some("com.a"));
    let (_, fx) = h.msg_fx(&["--subscribe", "late", "privacy_indicator_change"]);
    assert!(runs_of(&fx, "late").is_empty(), "already subscribed: no second delivery");
    // Before any sample there is nothing to deliver.
    let mut h = H::new();
    assert!(runs_of(&add_watcher(&mut h, "early"), "early").is_empty());
}

#[test]
fn reload_keeps_state_and_detection() {
    let mut h = H::new();
    add_watcher(&mut h, "a");
    feed(&mut h, sample(true, &["com.a"]));
    let before = h.msg(&["--query", "privacy_indicator"]);
    h.msg(&["--reload"]);
    assert_eq!(h.msg(&["--query", "privacy_indicator"]), before);
    let fx = add_watcher(&mut h, "a");
    assert_eq!(starts(&fx), 0, "detection survives --reload");
    assert_eq!(runs_of(&fx, "a").len(), 1, "re-run config gets the state");
}

#[test]
fn manual_trigger_reaches_subscribers_but_keeps_state() {
    let mut h = H::new();
    add_watcher(&mut h, "a");
    let (_, fx) = h.msg_fx(&["--trigger", "privacy_indicator_change", "VISIBLE=on"]);
    assert_eq!(runs_of(&fx, "a")[0].get("VISIBLE"), Some("on"));
    assert_eq!(h.query(&["privacy_indicator"])["visible"], "off");
}

#[test]
fn add_event_with_a_notification_observes_nothing() {
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&["--add", "event", "privacy_indicator_change", "com.example.note"]);
    assert!(!platform(&fx)
        .iter()
        .any(|p| matches!(p, PlatformRequest::ObserveNotification(_))));
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test -p mbar-core --test wpb_privacy`
Expected: compile errors (`Input::PrivacyIndicator`, `PlatformRequest::StartPrivacyIndicator` not found).

- [ ] **Step 3: Add the variants and the model field**

`crates/mbar-core/src/platform.rs`, in `enum Input` after `AerospaceStatus(..)`:

```rust
    /// The privacy indicator changed (`crate::privacy`; macOS `sys::privacy`).
    PrivacyIndicator(crate::privacy::PrivacySample),
```

and in `enum PlatformRequest` after `StartAerospace`:

```rust
    /// First use of the privacy indicator (a `privacy_indicator_change` subscription or
    /// `privacy_indicator_inset=on`; never `--query privacy_indicator`): start detecting.
    StartPrivacyIndicator,
```

`crates/mbar-core/src/props.rs`, in `enum PropRequest` after `MenuBarHidden(bool)`:

```rust
    /// `--bar privacy_indicator_inset=on` (extension): start the detection.
    StartPrivacyIndicator,
```

`crates/mbar-core/src/bar.rs`: field after `hide_menubar`:

```rust
    /// Extension `privacy_indicator_inset=on|off`: right items avoid the privacy dot.
    /// Not part of `--query bar` (SketchyBar's exact output); shown by
    /// `--query privacy_indicator`.
    pub privacy_indicator_inset: bool,
```

default `privacy_indicator_inset: false,` after `hide_menubar: false,`, and in `set_prop` after the `"hide_menubar"` arm:

```rust
            "privacy_indicator_inset" => {
                let on = value::parse_bool(v, self.privacy_indicator_inset);
                let changed = set_bool(&mut self.privacy_indicator_inset, on);
                if changed {
                    cx.fx.bar_needs_update = true;
                    if on {
                        cx.request(PropRequest::StartPrivacyIndicator);
                    }
                }
                changed
            }
```

`crates/mbar-core/src/model.rs`: `use crate::privacy::PrivacyState;`, field after `aerospace`:

```rust
    /// Privacy indicator state (`Input::PrivacyIndicator`, `--query privacy_indicator`,
    /// the bar inset). Survives `--reload`.
    pub privacy: PrivacyState,
```

and `privacy: PrivacyState::default(),` in `Model::new` after `aerospace: …`.

`crates/mbar-core/src/command.rs`: variant `PrivacyIndicator,` after `Aerospace,` in `enum QueryTarget` (doc comment `/// \`--query privacy_indicator\` (extension).`), and `"privacy_indicator" => QueryTarget::PrivacyIndicator,` after the `"aerospace"` arm.

`crates/mbar-core/src/query.rs`: after the `QueryTarget::Aerospace` arm:

```rust
        QueryTarget::PrivacyIndicator => match item_by_name("privacy_indicator") {
            Some(item) => item_json(model, item),
            None => model.privacy.to_json(model.bar.privacy_indicator_inset),
        },
```

and add `privacy_indicator` to the list of extension keywords in the doc comment of `query()` and the module header (`--query privacy_indicator`: privacy design).

`crates/mbar-macos/src/platform/services.rs`, in `execute` after `StartAerospace`:

```rust
            // Wired to `sys::privacy` in the next step of the privacy plan (Task 5).
            PlatformRequest::StartPrivacyIndicator => {}
```

- [ ] **Step 4: Runtime**

In `crates/mbar-core/src/runtime.rs`:

1. Imports: `use crate::privacy::{self, PrivacySample};`.
2. Rename the field `aerospace_initial` → `initial_events` (doc: "Late subscribers to built-in events (`aerospace_*`, `privacy_indicator_change`) that get the stored state …"), and the methods `queue_aerospace_initial` → `queue_initial`, `flush_aerospace_initial` → `flush_initial` (all call sites: `handle`, `reload`, `exec_subscribe`, `register_global_handler`). `Listener` doc: "Who gets a synthetic built-in event".
3. Replace the body lookups in `queue_initial` / `flush_initial` with a shared helper:

```rust
    /// The env of the synthetic event `name` for a late subscriber, if its state is known.
    fn initial_env(&self, name: &str) -> Option<EnvVars> {
        if name == privacy::EVENT_NAME {
            return self.model.privacy.known.then(|| self.privacy_env());
        }
        self.model
            .aerospace
            .synthetic_event(name)
            .map(|ev| Self::aerospace_env(&ev))
    }
```

`queue_initial`: `if self.initial_env(name).is_some() && !self.initial_events.contains(&(who, name)) { push }`.
`flush_initial`: `let Some(env) = self.initial_env(name) else { continue; };` instead of the `synthetic_event` + `aerospace_env` pair.

4. `handle`: new arm `Input::PrivacyIndicator(s) => self.privacy_sample(s, &mut effects),`.
5. Prop requests (the loop with `PropRequest::MenuBarHidden`):

```rust
                PropRequest::StartPrivacyIndicator => self.start_privacy(effects),
```

6. `Command::AddEvent`: the filter becomes
`let notification = notification.filter(|_| !aerospace::is_event_name(&name) && name != privacy::EVENT_NAME);` and the comment mentions the privacy event.
7. `exec_subscribe`: replace the `aerospace_event` lookup with

```rust
            let builtin: Option<&'static str> = aerospace::EVENT_NAMES
                .iter()
                .copied()
                .find(|n| *n == ev.as_str())
                .or_else(|| (ev == privacy::EVENT_NAME).then_some(privacy::EVENT_NAME));
            if let Some(name) = builtin {
                // Built-in events: registered on first use (same registry entry as
                // `--add event`), and their source is started.
                if self.model.events.flag(ev).is_none() {
                    self.model.events.append(ev, None);
                }
                if name == privacy::EVENT_NAME {
                    self.start_privacy(effects);
                } else {
                    self.start_aerospace(effects);
                }
            }
```

and at the end `if let Some(name) = builtin.filter(|_| new) { self.queue_initial(Listener::Item(id), name); }` (comment: the state may have arrived before this item existed).
8. Reload (next to `aerospace`): `let privacy = std::mem::take(&mut self.model.privacy);` … `self.model.privacy = privacy;`.
9. New section after the AeroSpace section:

```rust
    // ------------------------------------------------------------------------------
    // Privacy indicator (extension, `docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`).
    // ------------------------------------------------------------------------------

    /// Emits `PlatformRequest::StartPrivacyIndicator` the first time it is needed.
    fn start_privacy(&mut self, effects: &mut Vec<Effect>) {
        if !self.model.privacy.active {
            self.model.privacy.active = true;
            effects.push(Effect::Platform(PlatformRequest::StartPrivacyIndicator));
        }
    }

    /// `INFO` + the event's variables.
    fn privacy_env(&self) -> EnvVars {
        let mut env = EnvVars::new();
        env.set("INFO", self.model.privacy.info_json());
        for (k, v) in self.model.privacy.env() {
            env.set(k, v);
        }
        env
    }

    /// `Input::PrivacyIndicator`: stores the sample; on a change fires
    /// `privacy_indicator_change` and, when the dot moved and the inset is on, lays the
    /// bars out again.
    fn privacy_sample(&mut self, s: PrivacySample, effects: &mut Vec<Effect>) {
        let change = self.model.privacy.apply(s);
        if change.geometry && self.model.bar.privacy_indicator_inset {
            self.model.bar_needs_update = true;
        }
        if change.any {
            let env = self.privacy_env();
            self.trigger_event(EventInfo::new(privacy::EVENT_NAME, Some(env)), effects);
        }
    }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p mbar-core --test wpb_privacy && cargo test -p mbar-core`
Expected: the 8 new tests pass; all existing core tests (including `wpb_aerospace` after the rename, and `bar::tests::default_query`) still pass.

On macOS also: `cargo build --workspace`
Expected: builds (the interim `StartPrivacyIndicator => {}` arm keeps `Services::execute` exhaustive).

- [ ] **Step 6: Commit**

```bash
git add crates/mbar-core crates/mbar-macos/src/platform/services.rs
git commit -m "feat(core): privacy_indicator_change event, inset property, query and lazy start"
```

---

### Task 4: Layout: the inset

**Files:**
- Modify: `crates/mbar-core/src/layout.rs` (`horizontal_pass`, new helper `privacy_right_edge`)
- Test: `crates/mbar-core/tests/wpb_privacy.rs` (append)

**Interfaces:**
- Consumes: `Model::privacy` (`visible`, `frames`), `BarProps::privacy_indicator_inset` (Task 3), `BarState::frame`.
- Produces: right-hand items laid out against `min(w, dot.x - bar.x) - padding_right`.

- [ ] **Step 1: Write the failing tests** (append to `wpb_privacy.rs`)

```rust
fn x_of(h: &mut H, item: &str) -> f64 {
    h.query(&["item", item])["bounding_rects"]["display-1"]["origin"][0]
        .as_f64()
        .unwrap()
}

fn clock_bar(h: &mut H) {
    h.msg(&["--add", "item", "clock", "right", "--set", "clock", "label=12:00"]);
    h.msg(&["--add", "item", "left", "left", "--set", "left", "label=L"]);
}

#[test]
fn inset_moves_right_items_left_of_the_dot() {
    let mut h = H::new();
    clock_bar(&mut h);
    let (x0, l0) = (x_of(&mut h, "clock"), x_of(&mut h, "left"));
    h.msg(&["--bar", "privacy_indicator_inset=on"]);
    feed(&mut h, sample(true, &[]));
    // Display 1920 wide, dot at 1892: the right edge moves 28 pt left.
    assert_eq!(x_of(&mut h, "clock"), x0 - 28.0);
    assert_eq!(x_of(&mut h, "left"), l0, "left items stay");
    // padding_right stays the gap to the dot.
    h.msg(&["--bar", "padding_right=10"]);
    let with_dot = x_of(&mut h, "clock");
    feed(&mut h, sample(false, &[]));
    assert_eq!(x_of(&mut h, "clock"), with_dot + 28.0, "dot gone: back to the edge");
}

#[test]
fn no_inset_when_off_or_not_overlapping() {
    let mut h = H::new();
    clock_bar(&mut h);
    let x0 = x_of(&mut h, "clock");
    // Detection started by a subscription; the property stays off. (`w` is added after
    // `clock`, so it sits left of it and does not move it.)
    add_watcher(&mut h, "w");
    feed(&mut h, sample(true, &[]));
    assert_eq!(x_of(&mut h, "clock"), x0, "property off");
    // Property on, but the dot does not intersect the bar (another display).
    h.msg(&["--bar", "privacy_indicator_inset=on"]);
    feed(
        &mut h,
        Input::PrivacyIndicator(PrivacySample {
            visible: true,
            frames: vec![Rect::new(3000.0, 500.0, 28.0, 28.0)],
            attributions: None,
        }),
    );
    assert_eq!(x_of(&mut h, "clock"), x0, "no overlap");
    // A bottom bar does not meet a dot at the top.
    feed(&mut h, sample(true, &[]));
    h.msg(&["--bar", "position=bottom"]);
    let mut plain = H::new();
    clock_bar(&mut plain);
    plain.msg(&["--add", "item", "w", "right"]);
    plain.msg(&["--bar", "position=bottom"]);
    assert_eq!(x_of(&mut h, "clock"), x_of(&mut plain, "clock"), "bottom bar");
}

#[test]
fn inset_smaller_than_padding_does_not_panic() {
    let mut h = H::new();
    clock_bar(&mut h);
    h.msg(&["--bar", "privacy_indicator_inset=on"]);
    feed(
        &mut h,
        Input::PrivacyIndicator(PrivacySample {
            visible: true,
            frames: vec![Rect::new(5.0, 0.0, 28.0, 100.0)],
            attributions: None,
        }),
    );
    // Right items overflow; SketchyBar's rule puts them at the edge. No panic, finite x.
    assert!(x_of(&mut h, "clock").is_finite());
}
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p mbar-core --test wpb_privacy inset`
Expected: `inset_moves_right_items_left_of_the_dot` fails (`x0 - 28` expected, got `x0`).

- [ ] **Step 3: Implement**

In `crates/mbar-core/src/layout.rs`, in `horizontal_pass` replace

```rust
    let mut cur_r = to_u32(w - bbg.padding_right.max(0) as f64);
```

with

```rust
    let right_edge = privacy_right_edge(model, &bar).unwrap_or(w);
    let mut cur_r = to_u32(right_edge - bbg.padding_right.max(0) as f64);
```

Note `bbg` borrows `model.bar.background`; compute `right_edge` **before** `let bbg = &model.bar.background;` if the borrow checker complains (both are shared borrows, so it should not).

Add the helper next to `is_builtin`:

```rust
/// Privacy indicator extension (`privacy_indicator_inset`): the left edge of the
/// leftmost indicator window that overlaps `bar`, relative to the bar, clamped to
/// `0..=width`. `None` when the inset is off, the dot is hidden or does not overlap.
fn privacy_right_edge(model: &Model, bar: &BarState) -> Option<f64> {
    if !model.bar.privacy_indicator_inset || !model.privacy.visible {
        return None;
    }
    model
        .privacy
        .frames
        .iter()
        .filter(|f| f.intersection(&bar.frame).is_some())
        .map(|f| (f.x - bar.frame.x) as f64)
        .reduce(f64::min)
        .map(|x| x.clamp(0.0, bar.frame.width as f64))
}
```

(`BarState` is `crate::bar::BarState`; import it if `layout.rs` does not already.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mbar-core`
Expected: all pass, including the existing layout suites (`wpa_layout`, `review_layout`).

- [ ] **Step 5: Commit**

```bash
git add crates/mbar-core/src/layout.rs crates/mbar-core/tests/wpb_privacy.rs
git commit -m "feat(core): right-hand items avoid the privacy dot (privacy_indicator_inset)"
```

---

### Task 5: macOS worker and wiring

**Host:** macOS.

**Files:**
- Create: `crates/mbar-macos/src/sys/privacy.rs`
- Modify: `crates/mbar-macos/src/sys/mod.rs` (`pub mod privacy;`, `SysEvent::PrivacyIndicator`)
- Modify: `crates/mbar-macos/src/sys/alias.rs` (`rect_from_bounds` → `pub(crate)`)
- Modify: `crates/mbar-macos/src/platform/convert.rs` (mapping + table + test)
- Modify: `crates/mbar-macos/src/platform/services.rs` (start, nudge, stop; module table)

**Interfaces:**
- Consumes: `mbar_core::privacy::{classify_stream_line, StreamLine, Tracker, TrackerInput, PrivacySample, LOG_PREDICATE}`.
- Produces: `pub fn start(sink: Sink)`, `pub fn nudge()`, `pub fn stop()`, `pub(crate) struct WindowRecord`, `pub(crate) fn indicator_frames(&[WindowRecord]) -> Vec<Rect>`, `SysEvent::PrivacyIndicator(PrivacySample)`.

- [ ] **Step 1: Write the failing tests** (bottom of the new `sys/privacy.rs`; create the file with the header and these tests first)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn w(owner: &str, layer: i64, x: f32, size: f32) -> WindowRecord {
        WindowRecord {
            owner: owner.into(),
            layer,
            frame: Rect::new(x, 3.0, size, size),
        }
    }

    #[test]
    fn filters_indicator_windows() {
        // As measured: two identical StatusIndicator windows, plus other Window Server
        // windows (backstop, underbelly) and an app window on a high layer.
        let list = [
            w("Window Server", INDICATOR_LAYER, 2025.0, 28.0),
            w("Window Server", INDICATOR_LAYER, 2025.0, 28.0),
            w("Window Server", -2147483626, 0.0, 2056.0),
            w("Window Server", INDICATOR_LAYER, 0.0, 2056.0),
            w("mbar", 3, 0.0, 28.0),
            w("Window Server", INDICATOR_LAYER, 10.0, 0.0),
        ];
        assert_eq!(
            indicator_frames(&list),
            vec![Rect::new(2025.0, 3.0, 28.0, 28.0), Rect::new(2025.0, 3.0, 28.0, 28.0)]
        );
    }
}
```

Run: `cargo test -p mbar-macos --lib sys::privacy`
Expected: compile error (`WindowRecord`, `indicator_frames` not found).

- [ ] **Step 2: Implement the worker**

`crates/mbar-macos/src/sys/privacy.rs` above the tests:

```rust
//! Privacy indicator detection (`docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`,
//! §mbar-macos). One worker thread drives [`mbar_core::privacy::Tracker`]: it spawns
//! `log stream` (Control Center's attribution lines) when the tracker asks, runs one
//! `log show` after every spawn for the starting state, looks at the indicator windows
//! (`CGWindowListCopyWindowInfo`) when the tracker asks and posts
//! [`SysEvent::PrivacyIndicator`] whenever the sample changed. Nothing runs on the main
//! thread.

use super::{alias, util, Sink, SysEvent};
use mbar_core::geometry::Rect;
use mbar_core::privacy::{
    classify_stream_line, PrivacySample, StreamLine, Tracker, TrackerInput, LOG_PREDICATE,
};
use objc2_core_foundation::{CFArray, CFDictionary, CFRetained};
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Instant;

const LOG_BIN: &str = "/usr/bin/log";
/// Layer of WindowServer's `StatusIndicator` windows (measured on macOS 27.0.1:
/// owner `Window Server`, name `StatusIndicator`, 28×28 pt at the top-right corner).
pub(crate) const INDICATOR_LAYER: i64 = 2_147_483_630;
/// Larger windows on that layer are not the dot.
const MAX_SIZE: f32 = 64.0;

/// One on-screen window, as far as the filter needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowRecord {
    pub owner: String,
    pub layer: i64,
    /// Global points, top-left origin.
    pub frame: Rect,
}

/// The indicator windows among `windows`. The name (`StatusIndicator`) is not used: it
/// needs the Screen Recording permission, owner, layer and bounds do not.
pub(crate) fn indicator_frames(windows: &[WindowRecord]) -> Vec<Rect> {
    windows
        .iter()
        .filter(|w| {
            w.owner == "Window Server"
                && w.layer == INDICATOR_LAYER
                && w.frame.width > 0.0
                && w.frame.height > 0.0
                && w.frame.width <= MAX_SIZE
                && w.frame.height <= MAX_SIZE
        })
        .map(|w| w.frame)
        .collect()
}

fn list_windows() -> Vec<WindowRecord> {
    let Some(list): Option<CFRetained<CFArray>> =
        CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0)
    else {
        return Vec::new();
    };
    util::array_items(&list)
        .into_iter()
        .filter_map(util::downcast::<CFDictionary>)
        .filter_map(|d| {
            let owner = util::dict_string(&d, "kCGWindowOwnerName")?;
            let layer = util::dict_i64(&d, "kCGWindowLayer")?;
            let bounds =
                util::dict_get(&d, "kCGWindowBounds").and_then(util::downcast::<CFDictionary>)?;
            let r = alias::rect_from_bounds(&bounds)?;
            Some(WindowRecord {
                owner,
                layer,
                frame: Rect::new(
                    r.origin.x as f32,
                    r.origin.y as f32,
                    r.size.width as f32,
                    r.size.height as f32,
                ),
            })
        })
        .collect()
}

enum Msg {
    Input(TrackerInput),
    Stop,
}

struct Shared {
    tx: Sender<Msg>,
    child: Option<Child>,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

fn with_shared<R>(f: impl FnOnce(&mut Option<Shared>) -> R) -> R {
    let mut guard = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

fn send(msg: Msg) {
    with_shared(|s| {
        if let Some(s) = s {
            let _ = s.tx.send(msg);
        }
    });
}

/// Starts the detection (idempotent; `PlatformRequest::StartPrivacyIndicator`).
pub fn start(sink: Sink) {
    let (tx, rx) = mpsc::channel();
    let first = with_shared(|s| {
        if s.is_some() {
            return false;
        }
        *s = Some(Shared { tx, child: None });
        true
    });
    if !first {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mbar-privacy".into())
        .spawn(move || worker(sink, rx));
    if let Err(e) = spawned {
        log::warn!("privacy indicator: worker thread not started: {e}");
    }
}

/// Display reconfiguration or wake: check the windows now.
pub fn nudge() {
    send(Msg::Input(TrackerInput::Nudge));
}

/// Stops the worker and kills `log stream` (call on exit).
pub fn stop() {
    let child = with_shared(|s| {
        let s = s.as_mut()?;
        let _ = s.tx.send(Msg::Stop);
        s.child.take()
    });
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn worker(sink: Sink, rx: mpsc::Receiver<Msg>) {
    let mut tracker = Tracker::new(Instant::now());
    let mut last: Option<PrivacySample> = None;
    loop {
        let now = Instant::now();
        if tracker.restart_at().is_some_and(|t| t <= now) {
            let input = if spawn_stream() {
                spawn_history();
                TrackerInput::StreamStarted
            } else {
                TrackerInput::StreamExited
            };
            tracker.handle(input, now);
        }
        if tracker.next_check().is_some_and(|t| t <= now) {
            let frames = indicator_frames(&list_windows());
            tracker.handle(TrackerInput::Windows(frames), now);
        }
        if tracker.ready() {
            let sample = tracker.sample();
            if last.as_ref() != Some(&sample) {
                if sample.attributions.is_none()
                    && last.as_ref().is_some_and(|l| l.attributions.is_some())
                {
                    log::warn!("privacy indicator: app attribution unavailable (log stream down or its format changed)");
                }
                sink(SysEvent::PrivacyIndicator(sample.clone()));
                last = Some(sample);
            }
        }
        let deadline = [tracker.restart_at(), tracker.next_check()]
            .into_iter()
            .flatten()
            .min();
        let msg = match deadline {
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            },
        };
        match msg {
            Some(Msg::Stop) => return,
            Some(Msg::Input(input)) => tracker.handle(input, Instant::now()),
            None => {}
        }
    }
}

/// Spawns `log stream` and its reader thread. `false` if it could not be started.
fn spawn_stream() -> bool {
    let mut child = match Command::new(LOG_BIN)
        .args(["stream", "--style", "ndjson", "--predicate", LOG_PREDICATE])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("privacy indicator: `log stream` not started: {e}");
            return false;
        }
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };
    with_shared(|s| {
        if let Some(s) = s {
            s.child = Some(child);
        }
    });
    let reader = std::thread::Builder::new()
        .name("mbar-privacy-log".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match classify_stream_line(&line) {
                    StreamLine::Attributions(a) => send(Msg::Input(TrackerInput::Line(a))),
                    StreamLine::Unparsed => send(Msg::Input(TrackerInput::Unparsed)),
                    StreamLine::Other => {}
                }
            }
            let child = with_shared(|s| s.as_mut().and_then(|s| s.child.take()));
            if let Some(mut c) = child {
                let _ = c.kill();
                let _ = c.wait();
            }
            send(Msg::Input(TrackerInput::StreamExited));
        });
    reader.is_ok()
}

/// One `log show --last 1h` with the same predicate; the newest parsed line becomes
/// `History` (the tracker ignores it if a live line came first).
fn spawn_history() {
    let _ = std::thread::Builder::new()
        .name("mbar-privacy-history".into())
        .spawn(|| {
            let out = Command::new(LOG_BIN)
                .args(["show", "--last", "1h", "--style", "ndjson", "--predicate", LOG_PREDICATE])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output();
            let newest = out.ok().and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| match classify_stream_line(l) {
                        StreamLine::Attributions(a) => Some(a),
                        _ => None,
                    })
                    .last()
            });
            send(Msg::Input(TrackerInput::History(newest)));
        });
}
```

`crates/mbar-macos/src/sys/alias.rs`: change `fn rect_from_bounds` to `pub(crate) fn rect_from_bounds`.

`crates/mbar-macos/src/sys/mod.rs`: `pub mod privacy;` between `pub mod mouse;` and `pub mod providers;`, and in `enum SysEvent`:

```rust
    /// Privacy indicator sample from [`privacy`] (worker thread), de-duplicated.
    PrivacyIndicator(mbar_core::privacy::PrivacySample),
```

- [ ] **Step 3: Map and wire**

`crates/mbar-macos/src/platform/convert.rs` in `sys_event_to_input`:

```rust
        SysEvent::PrivacyIndicator(s) => Input::PrivacyIndicator(s),
```

add the row `/// | \`PrivacyIndicator\` | \`PrivacyIndicator\` |` to the table above it, and to the mapping test:

```rust
        let s = mbar_core::privacy::PrivacySample::default();
        assert_eq!(
            map(SysEvent::PrivacyIndicator(s.clone())),
            Some(Input::PrivacyIndicator(s))
        );
```

`crates/mbar-macos/src/platform/services.rs`:
- `use crate::sys::privacy;`
- `execute`: replace the interim arm with `PlatformRequest::StartPrivacyIndicator => privacy::start(self.sink.clone()),`
- `translate`: in the `DisplaysReconfigured` and `SystemWoke` arms add `privacy::nudge();`
- `shutdown`: add `privacy::stop();`
- module table: `//! | \`StartPrivacyIndicator\` | \`privacy::start\` (worker thread: \`log stream\` + window checks; reply \`SysEvent::PrivacyIndicator\`) |` and extend the closing paragraph: "The privacy worker is nudged on `DisplaysReconfigured` / `SystemWoke`; `shutdown` kills its `log stream`."

- [ ] **Step 4: Build and test**

Run: `cargo test -p mbar-macos && cargo clippy --workspace --all-targets -- -D warnings`
Expected: tests pass (incl. `filters_indicator_windows` and the convert mapping test); no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/mbar-macos
git commit -m "feat(macos): privacy indicator worker (log stream, window check, wiring)"
```

---

### Task 6: Docs and Lua types

**Files:**
- Modify: `docs/EXTENSIONS.md`, `docs/LUA.md`, `docs/ARCHITECTURE.md`, `lua/mbar.d.lua`
- Test: `crates/mbar-lua/tests/docs.rs` (new test for the marked example)

**Interfaces:**
- Consumes: the user-facing names from **Global Constraints**.

- [ ] **Step 1: Write the failing docs test** (append to `crates/mbar-lua/tests/docs.rs`)

```rust
/// The privacy indicator example of `docs/LUA.md`: inset on, the clock subscribes
/// and turns orange while an app uses the microphone.
#[test]
fn privacy_indicator_example() {
    let mut engine = LuaEngine::new().unwrap();
    let mut host = Mock::default();
    engine
        .load_string(example("privacy-indicator"), "=init.lua", &mut host)
        .unwrap();
    let msg = host.messages.concat();
    assert!(has_seq(&msg, &["--bar", "privacy_indicator_inset=on"]), "{msg:?}");
    assert!(has_seq(&msg, &["--subscribe", "clock", "privacy_indicator_change"]), "{msg:?}");
    let handler = script_id(&msg, "clock");
    host.messages.clear();
    engine
        .run_handler(
            handler,
            &env(&[
                ("NAME", "clock"),
                ("SENDER", "privacy_indicator_change"),
                ("MIC", "com.goodsnooze.MacWhisper"),
                ("INFO", r#"{"visible":"on","mic":["com.goodsnooze.MacWhisper"],"camera":[],"screen":[],"audio":[],"location":[],"attribution":"on"}"#),
            ]),
            &mut host,
        )
        .unwrap();
    let set = host.messages.concat();
    assert!(
        set.iter().any(|a| a.starts_with("label.color=")),
        "{set:?}"
    );
}
```

Run: `cargo test -p mbar-lua --test docs privacy_indicator_example`
Expected: FAIL with `example marker` panic.

- [ ] **Step 2: `docs/LUA.md`** — new subsection `### Privacy indicator` after the `### AeroSpace` subsection (before `## Differences from SbarLua`):

````markdown
### Privacy indicator

`privacy_indicator_change` fires when macOS shows or hides its privacy dot or the
apps behind it change. `env.info` holds `visible`, `frame` and one list of bundle
ids per sensor (`mic`, `camera`, `screen`, `audio`, `location`); the same lists are
in `env.MIC`, `env.CAMERA`, … as comma-separated strings. With
`privacy_indicator_inset = true` the right-hand items move out of the dot's way.
Details in [`EXTENSIONS.md`](EXTENSIONS.md#privacy-indicator).

<!-- example: privacy-indicator -->
```lua
mbar.bar({ privacy_indicator_inset = true })

local clock = mbar.add("item", "clock", "right", { provider = "clock" })
clock:subscribe("privacy_indicator_change", function(env)
  local info = env.info or {}
  local mic = info.mic and #info.mic > 0
  clock:set({ label = { color = mic and 0xffff9f0a or 0xffffffff } })
end)
```
````

- [ ] **Step 3: `lua/mbar.d.lua`**
  - `mbar.Event` alias: add `---| "privacy_indicator_change"` after `"aerospace_binding_triggered"`.
  - `mbar.Env`: add after the AeroSpace fields

```lua
---@field VISIBLE? "on"|"off" privacy_indicator_change
---@field MIC? string privacy_indicator_change: comma-separated bundle ids
---@field CAMERA? string privacy_indicator_change
---@field SCREEN? string privacy_indicator_change
---@field AUDIO? string privacy_indicator_change (system audio capture)
---@field LOCATION? string privacy_indicator_change
```

  - `mbar.BarProps`: `---@field privacy_indicator_inset? mbar.Bool` after `hide_menubar`.
  - New class before `mbar.AerospaceResult`:

```lua
---`env.info` of `privacy_indicator_change` and `mbar.query("privacy_indicator")`.
---@class mbar.PrivacyIndicatorInfo
---@field visible "on"|"off"
---@field frame? { x: integer, y: integer, w: integer, h: integer } Global points, top-left origin; absent while hidden
---@field mic string[] Bundle ids
---@field camera string[]
---@field screen string[]
---@field audio string[] System audio capture
---@field location string[]
---@field attribution "on"|"off" "off": the lists are unknown (log stream not running or its format changed)
---@field active? "on"|"off" Query only: detection started
---@field inset? "on"|"off" Query only: privacy_indicator_inset
```

- [ ] **Step 4: `docs/EXTENSIONS.md`**
  - Intro paragraph: add "The [privacy indicator](#privacy-indicator) is detected only when a config asks for it."
  - Command table: row `| \`--query privacy_indicator\` | JSON of the privacy indicator state. See [Privacy indicator](#privacy-indicator). |`
  - Bar properties table: row `| \`privacy_indicator_inset=on\|off\` | Right-hand items avoid macOS's privacy dot. See [Privacy indicator](#privacy-indicator). |`
  - New section `## Privacy indicator` before `## Lua`, containing: what the dot is and why it covers the bar (menu bar hidden); the event table (variables from Global Constraints) and an `INFO` example (the one from the spec); the inset rule (`min(w, dot.x - bar.x) - padding_right`, only overlapping horizontal bars, only `right` items, no animation, `--query bar` unchanged); `--query privacy_indicator` with `active`/`inset` and the item-name rule; lazy start; late subscribers get the state once; a manual `--trigger` reaches subscribers but does not change the state; `--add event privacy_indicator_change <notification>` ignores the notification; **How it works** (Control Center's log lines via `log stream`, the indicator window via `CGWindowList`, no Screen Recording permission needed, safety poll 10 s / 2 s); **Caveat**: the log line is not an API — on a format change `attribution` turns `off`, the inset keeps working. Link the spec.

- [ ] **Step 5: `docs/ARCHITECTURE.md`**
  - mbar-core table: row `| \`privacy\` | privacy indicator: Control Center log-line parser, state, \`privacy_indicator_change\` payload, \`--query privacy_indicator\`, and the \`Tracker\` (window-check timing, history order, format check, backoff) |`
  - mbar-macos table: row `| \`privacy\` | privacy indicator worker: \`log stream\`/\`log show\` children, \`CGWindowList\` indicator filter, drives \`mbar_core::privacy::Tracker\` |`
  - After `#### AeroSpace`, a `#### Privacy indicator` paragraph:

```
 first --subscribe privacy_indicator_change / privacy_indicator_inset=on (not --query)
                                   │
        Runtime ──► Effect::Platform(PlatformRequest::StartPrivacyIndicator)   (once)
                                   │
        Services::execute ──► sys::privacy::start(sink)
                                   │ worker thread: Tracker; `log stream` child + reader;
                                   │ `log show` once per spawn; CGWindowList checks
                                   ▼
        SysEvent::PrivacyIndicator(sample) ──► Input::PrivacyIndicator
                                   │
        Runtime: store (survives --reload), fire privacy_indicator_change on change,
                 bar_needs_update when the dot moved and the inset is on;
                 layout::horizontal_pass ends right items at the dot
```

- [ ] **Step 6: Run the docs tests**

Run: `cargo test -p mbar-lua --test docs`
Expected: all pass, including `privacy_indicator_example` and `definitions_compile`.

- [ ] **Step 7: Commit**

```bash
git add docs lua crates/mbar-lua/tests/docs.rs
git commit -m "docs: privacy indicator (extensions, Lua, architecture, types)"
```

---

### Task 7: Lint, full test run and manual verification

**Host:** macOS.

- [ ] **Step 1: Lint and test everything**

Run: `make lint && make test`
Expected: no warnings, all tests pass.

- [ ] **Step 2: Manual check against the real dot**

Build and run a second daemon next to the installed one, with a test config:

```bash
cargo build --release
mkdir -p /private/tmp/mbar-privacy && cat > /private/tmp/mbar-privacy/mbarrc <<'EOF'
#!/bin/sh
mbar --bar height=32 privacy_indicator_inset=on \
     --add item clock right --set clock label=CLOCK \
     --add item dot right --subscribe dot privacy_indicator_change \
     --set dot script='mbar --set $NAME label="V=$VISIBLE M=$MIC C=$CAMERA S=$SCREEN A=$AUDIO"'
EOF
chmod +x /private/tmp/mbar-privacy/mbarrc
```

Then stop the installed bar for the duration of the test (quit mbar.app's daemon or `launchctl bootout` per `docs/INSTALL.md`), run `target/release/mbar --config /private/tmp/mbar-privacy/mbarrc`, and check, with the native menu bar hidden:

1. With an always-on source (SoundSource/ARK): the clock ends left of the dot; `mbar --query privacy_indicator` shows `visible: on`, `audio: ["com.rogueamoeba.arkaudiod"]`, `attribution: on`.
2. Photo Booth open/close: the `dot` label shows `C=com.apple.PhotoBooth`, then empty again within ~2 s.
3. Quit every source: the dot disappears and the clock moves back to the edge within ~2 s.
4. `pgrep -fl "log stream --style ndjson"` shows exactly one child of the test daemon; it is gone after the daemon exits.
5. With mbar idle for a minute, `ps -o %cpu -p <daemon pid>` stays near 0.

6. **As actually launched:** restart the installed bar (login item / `launchctl kickstart -k gui/$(id -u)/dev.rubeen.mbar` after installing the new build, see `docs/INSTALL.md`) with a config that sets `privacy_indicator_inset=on`. Check that `pgrep -P <daemon pid> -fl "log stream"` finds exactly one child and that `mbar --query privacy_indicator` shows `visible: on` and `audio: ["com.rogueamoeba.arkaudiod"]` — this proves `log stream` and the window owner name work without the terminal's permissions.

Record the outcome (pass/fail per point) in the PR description.

- [ ] **Step 3: Commit any fixes** from the manual run (each with its own test where possible).
