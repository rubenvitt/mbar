//! Privacy indicator awareness (`docs/superpowers/specs/2026-10-10-privacy-indicator-design.md`):
//! Control Center's log lines, the state behind `privacy_indicator_change`,
//! `--query privacy_indicator` and the bar's `privacy_indicator_inset`, and the
//! [`Tracker`] that times the platform's window checks.

use crate::geometry::Rect;
use crate::value::format_bool;
use serde::Serialize;
use serde_json::{json, Map, Value};

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
}
