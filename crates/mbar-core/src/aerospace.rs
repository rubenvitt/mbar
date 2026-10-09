//! AeroSpace integration (`docs/superpowers/specs/2026-10-09-aerospace-design.md`): the
//! events AeroSpace's `subscribe` stream delivers, their mbar event names and script
//! environment, and the connection status shown by `--query aerospace`.
//!
//! The connection itself lives outside the core (`crates/mbar-aerospace`, driven by the
//! binary); it reports [`crate::platform::Input::Aerospace`] and
//! [`crate::platform::Input::AerospaceStatus`].

use serde_json::Value;

/// One `ServerEvent` of AeroSpace's `subscribe` stream (AeroSpace `ServerEvent.swift`,
/// JSON with sorted keys and the type in `_event`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AerospaceEvent {
    /// `focused-workspace-changed`.
    WorkspaceChanged {
        workspace: String,
        prev_workspace: String,
    },
    /// `focus-changed` (`window_id` is `None` on an empty workspace).
    FocusChanged {
        window_id: Option<u32>,
        workspace: String,
    },
    /// `focused-monitor-changed` (`monitor_id` is 1-based).
    MonitorChanged { workspace: String, monitor_id: i64 },
    /// `mode-changed`.
    ModeChanged { mode: Option<String> },
    /// `window-detected`.
    WindowDetected {
        window_id: u32,
        workspace: Option<String>,
        app_bundle_id: Option<String>,
        app_name: Option<String>,
    },
    /// `binding-triggered`.
    BindingTriggered { mode: String, binding: String },
}

/// The mbar events, in [`AerospaceEvent`] order.
pub const EVENT_NAMES: [&str; 6] = [
    "aerospace_workspace_change",
    "aerospace_focus_change",
    "aerospace_monitor_change",
    "aerospace_mode_change",
    "aerospace_window_detected",
    "aerospace_binding_triggered",
];

/// Whether `name` is one of the built-in AeroSpace events.
pub fn is_event_name(name: &str) -> bool {
    EVENT_NAMES.contains(&name)
}

impl AerospaceEvent {
    /// Parses one event frame / line. Unknown event types and malformed JSON are `None`
    /// (newer AeroSpace versions may add events).
    pub fn from_json(text: &str) -> Option<AerospaceEvent> {
        let v: Value = serde_json::from_str(text.trim()).ok()?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let wid = |k: &str| {
            v.get(k)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        Some(match v.get("_event")?.as_str()? {
            "focused-workspace-changed" => AerospaceEvent::WorkspaceChanged {
                workspace: s("workspace")?,
                prev_workspace: s("prevWorkspace").unwrap_or_default(),
            },
            "focus-changed" => AerospaceEvent::FocusChanged {
                window_id: wid("windowId"),
                workspace: s("workspace")?,
            },
            "focused-monitor-changed" => AerospaceEvent::MonitorChanged {
                workspace: s("workspace")?,
                monitor_id: v.get("monitorId").and_then(Value::as_i64).unwrap_or(0),
            },
            "mode-changed" => AerospaceEvent::ModeChanged { mode: s("mode") },
            "window-detected" => AerospaceEvent::WindowDetected {
                window_id: wid("windowId")?,
                workspace: s("workspace"),
                app_bundle_id: s("appBundleId"),
                app_name: s("appName"),
            },
            "binding-triggered" => AerospaceEvent::BindingTriggered {
                mode: s("mode")?,
                binding: s("binding")?,
            },
            _ => return None,
        })
    }

    /// The mbar event this triggers (one of [`EVENT_NAMES`]).
    pub fn event_name(&self) -> &'static str {
        match self {
            AerospaceEvent::WorkspaceChanged { .. } => EVENT_NAMES[0],
            AerospaceEvent::FocusChanged { .. } => EVENT_NAMES[1],
            AerospaceEvent::MonitorChanged { .. } => EVENT_NAMES[2],
            AerospaceEvent::ModeChanged { .. } => EVENT_NAMES[3],
            AerospaceEvent::WindowDetected { .. } => EVENT_NAMES[4],
            AerospaceEvent::BindingTriggered { .. } => EVENT_NAMES[5],
        }
    }

    /// Script variables of the event (besides `NAME`/`SENDER`/`INFO`). The names follow
    /// AeroSpace's documented SketchyBar trigger (`FOCUSED_WORKSPACE`, AeroSpace
    /// `docs/goodies.adoc`) so existing plugin scripts keep working. Recipes that pass
    /// AeroSpace's own variable names through (`AEROSPACE_FOCUSED_WORKSPACE=…`,
    /// `AEROSPACE_PREV_WORKSPACE=…`) work too: both are set as aliases wherever
    /// `FOCUSED_WORKSPACE` / `PREV_WORKSPACE` are.
    pub fn env(&self) -> Vec<(String, String)> {
        self.vars(true)
    }

    /// The variables of [`AerospaceEvent::env`]; `aliases` adds the `AEROSPACE_*` aliases.
    fn vars(&self, aliases: bool) -> Vec<(String, String)> {
        let mut env = Vec::new();
        let mut put = |k: &str, v: &str| env.push((k.to_string(), v.to_string()));
        match self {
            AerospaceEvent::WorkspaceChanged {
                workspace,
                prev_workspace,
            } => {
                put("FOCUSED_WORKSPACE", workspace);
                put("PREV_WORKSPACE", prev_workspace);
                if aliases {
                    put(ALIAS_FOCUSED_WORKSPACE, workspace);
                    put(ALIAS_PREV_WORKSPACE, prev_workspace);
                }
            }
            AerospaceEvent::FocusChanged {
                window_id,
                workspace,
            } => {
                put("FOCUSED_WORKSPACE", workspace);
                put(
                    "WINDOW_ID",
                    &window_id.map(|w| w.to_string()).unwrap_or_default(),
                );
                if aliases {
                    put(ALIAS_FOCUSED_WORKSPACE, workspace);
                }
            }
            AerospaceEvent::MonitorChanged {
                workspace,
                monitor_id,
            } => {
                put("FOCUSED_WORKSPACE", workspace);
                put("MONITOR_ID", &monitor_id.to_string());
                if aliases {
                    put(ALIAS_FOCUSED_WORKSPACE, workspace);
                }
            }
            AerospaceEvent::ModeChanged { mode } => put("MODE", mode.as_deref().unwrap_or("")),
            AerospaceEvent::WindowDetected {
                window_id,
                workspace,
                app_bundle_id,
                app_name,
            } => {
                put("WINDOW_ID", &window_id.to_string());
                put("WORKSPACE", workspace.as_deref().unwrap_or(""));
                put("APP_BUNDLE_ID", app_bundle_id.as_deref().unwrap_or(""));
                put("APP_NAME", app_name.as_deref().unwrap_or(""));
            }
            AerospaceEvent::BindingTriggered { mode, binding } => {
                put("MODE", mode);
                put("BINDING", binding);
            }
        }
        env
    }

    /// `INFO`: the event as JSON with mbar's snake_case keys (without the `AEROSPACE_*`
    /// aliases of [`AerospaceEvent::env`]).
    pub fn info_json(&self) -> String {
        let obj: serde_json::Map<String, Value> = self
            .vars(false)
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), Value::String(v)))
            .collect();
        Value::Object(obj).to_string()
    }
}

/// Alias of `FOCUSED_WORKSPACE` (AeroSpace's own variable name in
/// `exec-on-workspace-change`).
pub const ALIAS_FOCUSED_WORKSPACE: &str = "AEROSPACE_FOCUSED_WORKSPACE";
/// Alias of `PREV_WORKSPACE`.
pub const ALIAS_PREV_WORKSPACE: &str = "AEROSPACE_PREV_WORKSPACE";

/// How the connection to AeroSpace currently works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AerospaceTransport {
    /// Not connected.
    #[default]
    None,
    /// AeroSpace's socket (`/tmp/bobko.aerospace-$USER.sock`, protocol version 1).
    Socket,
    /// Fallback for servers without the socket protocol: a long-running
    /// `aerospace subscribe --all` child.
    Cli,
}

impl AerospaceTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            AerospaceTransport::None => "none",
            AerospaceTransport::Socket => "socket",
            AerospaceTransport::Cli => "cli",
        }
    }
}

/// Connection status for `--query aerospace` and mbar.app.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AerospaceStatus {
    pub connected: bool,
    pub transport: AerospaceTransport,
    /// `serverVersionAndHash` when known.
    pub server_version: Option<String>,
    /// Why the last attempt failed (shown while disconnected).
    pub error: Option<String>,
}

/// What the runtime knows about AeroSpace (`Model::aerospace`): the state carried by the
/// event stream plus the connection status. Survives `--reload` (the connection does too,
/// so AeroSpace does not resend its initial state).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AerospaceState {
    /// At least one state-carrying event (workspace, focus, monitor, mode) arrived.
    pub known: bool,
    pub focused_workspace: String,
    pub prev_workspace: String,
    /// Current binding mode (`main` …); empty until reported.
    pub mode: String,
    /// Focused monitor id (1-based); `0` until reported.
    pub monitor: i64,
    /// A `focus-changed` event arrived (`window_id` is meaningful).
    pub focus_known: bool,
    /// Window of the last `focus-changed` (`None` on an empty workspace).
    pub window_id: Option<u32>,
    /// The connection was requested (`PlatformRequest::StartAerospace` emitted; `active`
    /// in `--query aerospace`).
    pub active: bool,
    pub status: AerospaceStatus,
}

impl AerospaceState {
    fn focus_workspace(&mut self, workspace: &str, prev: Option<&str>) {
        match prev {
            // AeroSpace's initial event (right after `subscribe`) reports
            // `prevWorkspace == workspace`: no information about the previous one.
            Some(p) if !p.is_empty() && p != workspace => self.prev_workspace = p.to_string(),
            _ if self.focused_workspace != workspace && !self.focused_workspace.is_empty() => {
                self.prev_workspace = std::mem::take(&mut self.focused_workspace);
            }
            _ => {}
        }
        self.focused_workspace = workspace.to_string();
    }

    /// Updates the state from one event. Returns whether the event carries state
    /// (workspace, focus, monitor or mode); `window-detected` and `binding-triggered`
    /// do not.
    pub fn apply(&mut self, ev: &AerospaceEvent) -> bool {
        match ev {
            AerospaceEvent::WorkspaceChanged {
                workspace,
                prev_workspace,
            } => self.focus_workspace(workspace, Some(prev_workspace)),
            AerospaceEvent::FocusChanged {
                window_id,
                workspace,
            } => {
                self.focus_known = true;
                self.window_id = *window_id;
                self.focus_workspace(workspace, None);
            }
            AerospaceEvent::MonitorChanged {
                workspace,
                monitor_id,
            } => {
                self.monitor = *monitor_id;
                self.focus_workspace(workspace, None);
            }
            AerospaceEvent::ModeChanged { mode } => {
                self.mode = mode.clone().unwrap_or_default();
            }
            AerospaceEvent::WindowDetected { .. } | AerospaceEvent::BindingTriggered { .. } => {
                return false
            }
        }
        self.known = true;
        true
    }

    /// A manual `--trigger aerospace_workspace_change FOCUSED_WORKSPACE=…` (the
    /// `exec-on-workspace-change` fallback for AeroSpace versions without `subscribe`):
    /// `focused` / `prev` are the passed `FOCUSED_WORKSPACE` / `PREV_WORKSPACE` (or their
    /// `AEROSPACE_*` aliases). Applied only while not connected: with a connection the
    /// native stream is authoritative, and a forked trigger arriving later could
    /// overwrite a newer event. Returns whether the state changed.
    pub fn apply_manual_trigger(&mut self, focused: Option<&str>, prev: Option<&str>) -> bool {
        if self.status.connected {
            return false;
        }
        let Some(focused) = focused.filter(|w| !w.is_empty()) else {
            return false;
        };
        let before = (self.focused_workspace.clone(), self.prev_workspace.clone());
        self.focus_workspace(focused, Some(prev.unwrap_or("")));
        self.known = true;
        before.0 != self.focused_workspace || before.1 != self.prev_workspace
    }

    /// The event `name` (one of [`EVENT_NAMES`]) rebuilt from the stored state, for
    /// subscribers that arrive after AeroSpace sent it (AeroSpace sends its initial state
    /// once per connection). `None` when the state for that event is unknown or the event
    /// carries no state (`window_detected`, `binding_triggered`).
    pub fn synthetic_event(&self, name: &str) -> Option<AerospaceEvent> {
        let workspace =
            (!self.focused_workspace.is_empty()).then(|| self.focused_workspace.clone());
        match name {
            "aerospace_workspace_change" => Some(AerospaceEvent::WorkspaceChanged {
                workspace: workspace?,
                prev_workspace: self.prev_workspace.clone(),
            }),
            "aerospace_focus_change" if self.focus_known => Some(AerospaceEvent::FocusChanged {
                window_id: self.window_id,
                workspace: workspace?,
            }),
            "aerospace_monitor_change" if self.monitor != 0 => {
                Some(AerospaceEvent::MonitorChanged {
                    workspace: workspace?,
                    monitor_id: self.monitor,
                })
            }
            "aerospace_mode_change" if !self.mode.is_empty() => Some(AerospaceEvent::ModeChanged {
                mode: Some(self.mode.clone()),
            }),
            _ => None,
        }
    }

    /// `--query aerospace` (design §`--query aerospace`): TAB indented, strings
    /// JSON-escaped, `None` strings empty, trailing newline. `active` says whether the
    /// connection was started at all (it starts on first use, never by the query).
    pub fn to_json(&self) -> String {
        use crate::value::{format_bool, json_escape};
        let s = |v: &str| json_escape(v);
        let o = |v: &Option<String>| json_escape(v.as_deref().unwrap_or(""));
        format!(
            "{{\n\t\"connected\": \"{}\",\n\t\"active\": \"{}\",\n\t\"transport\": \"{}\",\n\t\"server_version\": \"{}\",\n\t\"error\": \"{}\",\n\t\"focused_workspace\": \"{}\",\n\t\"prev_workspace\": \"{}\",\n\t\"mode\": \"{}\",\n\t\"monitor\": {}\n}}\n",
            format_bool(self.status.connected),
            format_bool(self.active),
            self.status.transport.as_str(),
            o(&self.status.server_version),
            o(&self.status.error),
            s(&self.focused_workspace),
            s(&self.prev_workspace),
            s(&self.mode),
            self.monitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_follows_events() {
        let mut st = AerospaceState::default();
        assert!(!st.apply(&AerospaceEvent::BindingTriggered {
            mode: "main".into(),
            binding: "alt-1".into()
        }));
        assert!(!st.known);
        st.apply(&AerospaceEvent::FocusChanged {
            window_id: None,
            workspace: "1".into(),
        });
        assert_eq!(
            (st.focused_workspace.as_str(), st.prev_workspace.as_str()),
            ("1", "")
        );
        st.apply(&AerospaceEvent::MonitorChanged {
            workspace: "3".into(),
            monitor_id: 2,
        });
        assert_eq!(
            (st.focused_workspace.as_str(), st.prev_workspace.as_str()),
            ("3", "1")
        );
        assert_eq!(st.monitor, 2);
        st.apply(&AerospaceEvent::WorkspaceChanged {
            workspace: "4".into(),
            prev_workspace: "2".into(),
        });
        assert_eq!(
            (st.focused_workspace.as_str(), st.prev_workspace.as_str()),
            ("4", "2")
        );
        st.apply(&AerospaceEvent::ModeChanged { mode: None });
        assert_eq!(st.mode, "");
        assert!(st.known);
    }

    /// AeroSpace's initial workspace event has `prevWorkspace == workspace`: a known
    /// previous workspace is kept.
    #[test]
    fn initial_event_keeps_the_previous_workspace() {
        let mut st = AerospaceState::default();
        let ws = |cur: &str, prev: &str| AerospaceEvent::WorkspaceChanged {
            workspace: cur.into(),
            prev_workspace: prev.into(),
        };
        st.apply(&ws("1", "1"));
        assert_eq!(
            (st.focused_workspace.as_str(), st.prev_workspace.as_str()),
            ("1", "")
        );
        st.apply(&ws("2", "1"));
        // Reconnect: AeroSpace resends the current workspace as its own previous one.
        st.apply(&ws("2", "2"));
        assert_eq!(
            (st.focused_workspace.as_str(), st.prev_workspace.as_str()),
            ("2", "1")
        );
        // The workspace changed while disconnected: the stored one becomes the previous.
        st.apply(&ws("5", "5"));
        assert_eq!(
            (st.focused_workspace.as_str(), st.prev_workspace.as_str()),
            ("5", "2")
        );
    }

    #[test]
    fn manual_trigger_only_while_disconnected() {
        let mut st = AerospaceState::default();
        assert!(!st.apply_manual_trigger(None, Some("1")));
        assert!(!st.known);
        assert!(st.apply_manual_trigger(Some("2"), None));
        assert!(st.known);
        assert!(st.apply_manual_trigger(Some("3"), None));
        assert_eq!(
            (st.focused_workspace.as_str(), st.prev_workspace.as_str()),
            ("3", "2")
        );
        assert!(st.apply_manual_trigger(Some("4"), Some("1")));
        assert_eq!(
            (st.focused_workspace.as_str(), st.prev_workspace.as_str()),
            ("4", "1")
        );
        assert!(!st.apply_manual_trigger(Some("4"), Some("1")), "unchanged");
        st.status.connected = true;
        assert!(!st.apply_manual_trigger(Some("9"), None));
        assert_eq!(st.focused_workspace, "4");
    }

    #[test]
    fn synthetic_events_from_the_state() {
        let mut st = AerospaceState::default();
        for name in EVENT_NAMES {
            assert_eq!(st.synthetic_event(name), None, "{name}");
        }
        st.apply(&AerospaceEvent::WorkspaceChanged {
            workspace: "2".into(),
            prev_workspace: "1".into(),
        });
        st.apply(&AerospaceEvent::ModeChanged {
            mode: Some("main".into()),
        });
        assert_eq!(
            st.synthetic_event("aerospace_workspace_change"),
            Some(AerospaceEvent::WorkspaceChanged {
                workspace: "2".into(),
                prev_workspace: "1".into()
            })
        );
        assert_eq!(
            st.synthetic_event("aerospace_mode_change"),
            Some(AerospaceEvent::ModeChanged {
                mode: Some("main".into())
            })
        );
        assert_eq!(st.synthetic_event("aerospace_focus_change"), None);
        assert_eq!(st.synthetic_event("aerospace_monitor_change"), None);
        st.apply(&AerospaceEvent::FocusChanged {
            window_id: None,
            workspace: "2".into(),
        });
        st.apply(&AerospaceEvent::MonitorChanged {
            workspace: "2".into(),
            monitor_id: 1,
        });
        assert_eq!(
            st.synthetic_event("aerospace_focus_change"),
            Some(AerospaceEvent::FocusChanged {
                window_id: None,
                workspace: "2".into()
            })
        );
        assert_eq!(
            st.synthetic_event("aerospace_monitor_change"),
            Some(AerospaceEvent::MonitorChanged {
                workspace: "2".into(),
                monitor_id: 1
            })
        );
        assert_eq!(st.synthetic_event("aerospace_window_detected"), None);
        assert_eq!(st.synthetic_event("aerospace_binding_triggered"), None);
    }

    #[test]
    fn parses_every_event_type() {
        let ev = AerospaceEvent::from_json(
            r#"{"_event":"focused-workspace-changed","prevWorkspace":"1","workspace":"2"}"#,
        )
        .unwrap();
        assert_eq!(
            ev,
            AerospaceEvent::WorkspaceChanged {
                workspace: "2".into(),
                prev_workspace: "1".into()
            }
        );
        assert_eq!(ev.event_name(), "aerospace_workspace_change");
        assert_eq!(
            ev.env(),
            vec![
                ("FOCUSED_WORKSPACE".to_string(), "2".to_string()),
                ("PREV_WORKSPACE".to_string(), "1".to_string()),
                ("AEROSPACE_FOCUSED_WORKSPACE".to_string(), "2".to_string()),
                ("AEROSPACE_PREV_WORKSPACE".to_string(), "1".to_string()),
            ]
        );
        let focus = AerospaceEvent::FocusChanged {
            window_id: Some(3),
            workspace: "A".into(),
        };
        assert!(focus
            .env()
            .contains(&("AEROSPACE_FOCUSED_WORKSPACE".to_string(), "A".to_string())));
        assert_eq!(
            focus.info_json(),
            r#"{"focused_workspace":"A","window_id":"3"}"#
        );
        let monitor = AerospaceEvent::MonitorChanged {
            workspace: "B".into(),
            monitor_id: 1,
        };
        assert!(monitor
            .env()
            .contains(&("AEROSPACE_FOCUSED_WORKSPACE".to_string(), "B".to_string())));
        assert!(!monitor.info_json().contains("aerospace_"));
        assert_eq!(
            ev.info_json(),
            r#"{"focused_workspace":"2","prev_workspace":"1"}"#
        );

        assert_eq!(
            AerospaceEvent::from_json(r#"{"_event":"focus-changed","workspace":"3"}"#),
            Some(AerospaceEvent::FocusChanged {
                window_id: None,
                workspace: "3".into()
            })
        );
        assert_eq!(
            AerospaceEvent::from_json(
                r#"{"_event":"focused-monitor-changed","monitorId":2,"workspace":"A"}"#
            ),
            Some(AerospaceEvent::MonitorChanged {
                workspace: "A".into(),
                monitor_id: 2
            })
        );
        assert_eq!(
            AerospaceEvent::from_json(r#"{"_event":"mode-changed","mode":"service"}"#),
            Some(AerospaceEvent::ModeChanged {
                mode: Some("service".into())
            })
        );
        assert_eq!(
            AerospaceEvent::from_json(
                r#"{"_event":"window-detected","appBundleId":"com.x","appName":"X","windowId":7,"workspace":"1"}"#
            ),
            Some(AerospaceEvent::WindowDetected {
                window_id: 7,
                workspace: Some("1".into()),
                app_bundle_id: Some("com.x".into()),
                app_name: Some("X".into())
            })
        );
        assert_eq!(
            AerospaceEvent::from_json(
                r#"{"_event":"binding-triggered","binding":"alt-1","mode":"main"}"#
            ),
            Some(AerospaceEvent::BindingTriggered {
                mode: "main".into(),
                binding: "alt-1".into()
            })
        );
    }

    #[test]
    fn unknown_or_malformed_events_are_ignored() {
        assert_eq!(AerospaceEvent::from_json(r#"{"_event":"new-thing"}"#), None);
        assert_eq!(AerospaceEvent::from_json("not json"), None);
        assert_eq!(
            AerospaceEvent::from_json(r#"{"_event":"focus-changed"}"#),
            None
        );
        assert!(is_event_name("aerospace_mode_change"));
        assert!(!is_event_name("space_change"));
    }
}
