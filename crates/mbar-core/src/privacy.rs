//! Privacy indicator awareness (`docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`):
//! Control Center's log lines, the state behind `privacy_indicator_change`,
//! `--query privacy_indicator` and the bar's `privacy_indicator_inset`, and the
//! [`Tracker`] that times the platform's window checks.

use crate::geometry::Rect;
use crate::value::format_bool;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant};

/// The built-in event.
pub const EVENT_NAME: &str = "privacy_indicator_change";

/// `log stream` / `log show` predicate for Control Center's attribution lines
/// (subsystem `com.apple.controlcenter`, category `sensor-indicators`). The image path
/// rejects lines other processes log under Control Center's subsystem (os_log strings are
/// not authenticated).
pub const LOG_PREDICATE: &str = "subsystem == \"com.apple.controlcenter\" AND category == \"sensor-indicators\" AND processImagePath == \"/System/Library/CoreServices/ControlCenter.app/Contents/MacOS/ControlCenter\" AND (eventMessage BEGINSWITH \"Active activity attributions changed to \" OR eventMessage BEGINSWITH \"Sorted active attributions from SystemStatus update: \")";

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

const CHANGED_PREFIX: &str = "Active activity attributions changed to ";
const SORTED_PREFIX: &str = "Sorted active attributions from SystemStatus update: ";

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
        let entry = if i == 0 {
            part.strip_prefix('[')?
        } else {
            part
        };
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
        let any = geometry || self.attribution != attribution || self.attributions != attributions;
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

// ----------------------------------------------------------------------------------
// Tracker (design §mbar-core: `Tracker`)
// ----------------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

    fn a(
        mic: &[&str],
        camera: &[&str],
        screen: &[&str],
        audio: &[&str],
        location: &[&str],
    ) -> Attributions {
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
            parse_log_message(
                r#"Active activity attributions changed to ["aud:com.rogueamoeba.arkaudiod", "cam:com.apple.PhotoBooth", "mic:com.goodsnooze.MacWhisper"]"#
            ),
            Some(a(
                &["com.goodsnooze.MacWhisper"],
                &["com.apple.PhotoBooth"],
                &[],
                &["com.rogueamoeba.arkaudiod"],
                &[]
            ))
        );
        // Duplicated entry (seen for CleanShot), sorted output.
        assert_eq!(
            parse_log_message(
                r#"Active activity attributions changed to ["scr:pl.maketheweb.cleanshotx", "aud:com.rogueamoeba.arkaudiod", "scr:pl.maketheweb.cleanshotx", "scr:app.cotypist.Cotypist"]"#
            ),
            Some(a(
                &[],
                &[],
                &["app.cotypist.Cotypist", "pl.maketheweb.cleanshotx"],
                &["com.rogueamoeba.arkaudiod"],
                &[]
            ))
        );
        assert_eq!(
            parse_log_message("Active activity attributions changed to []"),
            Some(Attributions::default())
        );
        assert_eq!(
            parse_log_message(
                r#"Active activity attributions changed to ["loc:com.apple.weather", "xyz:com.example.new"]"#
            ),
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
        assert_eq!(
            parse_log_message("Active activity attributions changed to [broken"),
            None
        );
        assert_eq!(
            parse_log_message("Sorted active attributions from SystemStatus update: [aud] x (y)"),
            None
        );
        assert_eq!(
            parse_log_message(
                "Sorted active attributions from SystemStatus update: [[aud] no bundle]"
            ),
            None
        );
        assert_eq!(
            parse_log_message("Dependent controller changed: sensor indicators"),
            None
        );
    }

    #[test]
    fn classifies_stream_lines() {
        let line = r#"{"eventMessage":"Active activity attributions changed to [\"cam:com.apple.PhotoBooth\"]","subsystem":"com.apple.controlcenter"}"#;
        assert_eq!(
            classify_stream_line(line),
            StreamLine::Attributions(a(&[], &["com.apple.PhotoBooth"], &[], &[], &[]))
        );
        assert_eq!(
            classify_stream_line(
                r#"Filtering the log data using "subsystem == "com.apple.controlcenter" AND category == "sensor-indicators"""#
            ),
            StreamLine::Other
        );
        let renamed = r#"{"eventMessage":"Active activity attributions changed to {\"mic\":[]}"}"#;
        assert_eq!(classify_stream_line(renamed), StreamLine::Unparsed);
        assert_eq!(
            classify_stream_line(r#"{"eventMessage":"unrelated"}"#),
            StreamLine::Other
        );
        assert_eq!(classify_stream_line(""), StreamLine::Other);
    }

    fn sample(visible: bool, frames: &[Rect], at: Option<Attributions>) -> PrivacySample {
        PrivacySample {
            visible,
            frames: frames.to_vec(),
            attributions: at,
        }
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
    fn predicate_pins_control_center() {
        assert!(LOG_PREDICATE.contains(
            "processImagePath == \"/System/Library/CoreServices/ControlCenter.app/Contents/MacOS/ControlCenter\""
        ));
        assert!(LOG_PREDICATE.contains(&format!("eventMessage BEGINSWITH \"{CHANGED_PREFIX}\"")));
        assert!(LOG_PREDICATE.contains(&format!("eventMessage BEGINSWITH \"{SORTED_PREFIX}\"")));
    }

    #[test]
    fn env_info_and_query() {
        let mut st = PrivacyState::default();
        st.apply(sample(
            true,
            &[
                Rect::new(2025.0, 3.0, 28.0, 28.0),
                Rect::new(2025.0, 3.0, 28.0, 28.0),
            ],
            Some(a(&["b", "a"], &[], &[], &["ark"], &[])),
        ));
        let env: std::collections::HashMap<_, _> = st.env().into_iter().collect();
        assert_eq!(env["VISIBLE"], "on");
        assert_eq!(env["MIC"], "b,a");
        assert_eq!(env["CAMERA"], "");
        assert_eq!(env["AUDIO"], "ark");
        let info: serde_json::Value = serde_json::from_str(&st.info_json()).unwrap();
        assert_eq!(info["visible"], "on");
        assert_eq!(
            info["frame"],
            serde_json::json!({"x": 2025, "y": 3, "w": 28, "h": 28})
        );
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
        assert_eq!(
            tr.next_check(),
            Some(ms(t, 15_300)),
            "changed again: re-check in 2 s"
        );
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
        assert_eq!(
            tr.next_check(),
            Some(ms(t, 12_000)),
            "frames changed: re-check"
        );
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), ms(t, 12_000));
        assert_eq!(
            tr.next_check(),
            Some(ms(t, 13_000)),
            "format check 3 s after"
        );
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
        tr.handle(
            TrackerInput::Line(a(&[], &["cam"], &[], &[], &[])),
            ms(t, 9_500),
        );
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
        assert_eq!(
            tr.next_check(),
            Some(ms(t, 1_000)),
            "and the window is checked"
        );
    }

    #[test]
    fn stream_exits_back_off_and_poll_fast() {
        let t = Instant::now();
        let mut tr = Tracker::new(t);
        tr.handle(TrackerInput::StreamExited, t);
        tr.handle(TrackerInput::Windows(vec![f(2025.0)]), t);
        assert_eq!(tr.sample().attributions, None);
        assert_eq!(
            tr.next_check(),
            Some(ms(t, 2_000)),
            "2 s while the stream is down"
        );
        let mut expected = [1u64, 2, 4, 8, 16, 30, 30].into_iter();
        let mut now = t;
        assert_eq!(
            tr.restart_at(),
            Some(ms(now, 1_000 * expected.next().unwrap()))
        );
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
        tr.handle(
            TrackerInput::Windows(vec![f(2025.0), f(2025.0), Rect::ZERO]),
            t,
        );
        assert_eq!(tr.sample().frames, vec![f(2025.0)]);
        assert!(tr.ready() && tr.sample().visible);
        tr.handle(TrackerInput::Nudge, ms(t, 4_000));
        assert_eq!(tr.next_check(), Some(ms(t, 4_000)));
    }
}
