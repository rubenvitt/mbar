//! `subscribe_with` against a fake AeroSpace server and fake CLI scripts: event order,
//! status transitions, reconnect, CLI fallback, prompt shutdown.

mod common;

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::{config, fake_cli, temp_path, FakeServer, Handshake, Spec, SERVER_VERSION};
use mbar_aerospace::{subscribe_with, Config, Subscription};
use mbar_core::aerospace::{AerospaceEvent, AerospaceStatus, AerospaceTransport};

const WAIT: Duration = Duration::from_secs(5);

const WS_1_2: &str =
    r#"{"_event":"focused-workspace-changed","prevWorkspace":"1","workspace":"2"}"#;
const MODE_MAIN: &str = r#"{"_event":"mode-changed","mode":"main"}"#;
const FOCUS_7: &str = r#"{"_event":"focus-changed","windowId":7,"workspace":"2"}"#;

fn ws(prev: &str, cur: &str) -> AerospaceEvent {
    AerospaceEvent::WorkspaceChanged {
        workspace: cur.into(),
        prev_workspace: prev.into(),
    }
}

fn mode_main() -> AerospaceEvent {
    AerospaceEvent::ModeChanged {
        mode: Some("main".into()),
    }
}

fn focus_7() -> AerospaceEvent {
    AerospaceEvent::FocusChanged {
        window_id: Some(7),
        workspace: "2".into(),
    }
}

struct Harness {
    sub: Option<Subscription>,
    events: Receiver<AerospaceEvent>,
    status: Receiver<AerospaceStatus>,
}

fn start(config: Config) -> Harness {
    let (etx, events) = mpsc::channel();
    let (stx, status) = mpsc::channel();
    let sub = subscribe_with(
        config,
        move |e| {
            let _ = etx.send(e);
        },
        move |s| {
            let _ = stx.send(s);
        },
    );
    Harness {
        sub: Some(sub),
        events,
        status,
    }
}

impl Harness {
    fn event(&self) -> AerospaceEvent {
        self.events.recv_timeout(WAIT).expect("event")
    }

    fn status(&self) -> AerospaceStatus {
        self.status.recv_timeout(WAIT).expect("status")
    }

    fn no_more_events(&self) {
        assert_eq!(
            self.events.recv_timeout(Duration::from_millis(50)).ok(),
            None
        );
    }

    /// Drops the subscription and returns how long that took.
    fn stop(&mut self) -> Duration {
        let start = Instant::now();
        drop(self.sub.take());
        start.elapsed()
    }
}

fn connected_socket() -> AerospaceStatus {
    AerospaceStatus {
        connected: true,
        transport: AerospaceTransport::Socket,
        server_version: Some(SERVER_VERSION.into()),
        error: None,
    }
}

#[test]
fn socket_events_arrive_in_order_and_reconnect_after_a_restart() {
    let path = temp_path(".sock");
    let spec = Spec {
        initial_events: vec![
            WS_1_2.into(),
            MODE_MAIN.into(),
            r#"{"_event":"some-future-event","x":1}"#.into(),
            "not json".into(),
            FOCUS_7.into(),
        ],
        ..Spec::default()
    };
    let mut server = FakeServer::start(&path, spec.clone());
    let subscribed = server.on_subscribe();
    let mut h = start(config(&path, None));

    assert_eq!(h.status(), connected_socket());
    assert_eq!(h.event(), ws("1", "2"));
    assert_eq!(h.event(), mode_main());
    assert_eq!(h.event(), focus_7()); // unknown and malformed frames are skipped
    subscribed.recv_timeout(WAIT).unwrap();
    server.push(r#"{"_event":"focused-workspace-changed","prevWorkspace":"2","workspace":"3"}"#);
    server.push(r#"{"_event":"binding-triggered","binding":"alt-3","mode":"main"}"#);
    assert_eq!(h.event(), ws("2", "3"));
    assert_eq!(
        h.event(),
        AerospaceEvent::BindingTriggered {
            mode: "main".into(),
            binding: "alt-3".into()
        }
    );
    {
        let requests = server.requests.lock().unwrap();
        assert_eq!(
            requests.last().unwrap(),
            r#"{"args":["subscribe","--all"],"stdin":"","windowId":null,"workspace":null}"#
        );
    }

    // AeroSpace quits.
    server.stop();
    let down = h.status();
    assert!(!down.connected);
    assert_eq!(down.transport, AerospaceTransport::None);
    assert!(down.error.is_some());
    // While it stays away, the status does not repeat.
    std::thread::sleep(Duration::from_millis(150));
    let repeated: Vec<_> = h.status.try_iter().collect();
    assert!(
        repeated.iter().all(|s| !s.connected),
        "unexpected statuses {repeated:?}"
    );

    // AeroSpace comes back.
    let _server = FakeServer::start(&path, spec);
    let mut up = h.status();
    while !up.connected {
        up = h.status();
    }
    assert_eq!(up, connected_socket());
    assert_eq!(h.event(), ws("1", "2"));
    assert_eq!(h.event(), mode_main());
    assert_eq!(h.event(), focus_7());

    assert!(h.stop() < Duration::from_secs(1));
    h.no_more_events();
}

#[test]
fn not_running_reports_once_and_drop_is_prompt_during_backoff() {
    let path = temp_path(".sock");
    let mut c = config(&path, None);
    c.initial_backoff = Duration::from_secs(10);
    let mut h = start(c);
    let s = h.status();
    assert!(!s.connected);
    assert!(s.error.unwrap().contains("cannot connect"));
    assert!(h.stop() < Duration::from_secs(1));
}

#[test]
fn drop_is_prompt_while_streaming() {
    let path = temp_path(".sock");
    let _server = FakeServer::start(&path, Spec::default());
    let mut c = config(&path, None);
    c.initial_backoff = Duration::from_secs(10);
    let mut h = start(c);
    assert_eq!(h.status(), connected_socket());
    assert!(h.stop() < Duration::from_secs(1));
}

#[test]
fn cli_fallback_streams_lines_and_drop_kills_the_child() {
    let path = temp_path(".sock");
    let _server = FakeServer::start(
        &path,
        Spec {
            handshake: Handshake::Version(7),
            ..Spec::default()
        },
    );
    // Not `exec sleep`: the grandchild keeps stdout open, so only killing the whole
    // process group lets the reader finish.
    let marker = temp_path(".args");
    let cli = fake_cli(&format!(
        "echo \"$*\" > {marker}\n\
         echo '{WS_1_2}'\n\
         echo 'garbage'\n\
         echo ''\n\
         echo '{MODE_MAIN}'\n\
         sleep 30",
        marker = marker.display()
    ));
    let mut c = config(&path, Some(cli.path()));
    c.initial_backoff = Duration::from_secs(10);
    let mut h = start(c);
    assert_eq!(
        h.status(),
        AerospaceStatus {
            connected: true,
            transport: AerospaceTransport::Cli,
            server_version: None,
            error: None,
        }
    );
    assert_eq!(h.event(), ws("1", "2"));
    assert_eq!(h.event(), mode_main());
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().trim(),
        "subscribe --all"
    );
    assert!(h.stop() < Duration::from_secs(1));
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn cli_without_subscribe_reports_an_error() {
    let path = temp_path(".sock");
    let _server = FakeServer::start(
        &path,
        Spec {
            handshake: Handshake::Close,
            ..Spec::default()
        },
    );
    let cli = fake_cli("echo \"Unrecognized subcommand 'subscribe'\" >&2\nexit 2");
    let mut h = start(config(&path, Some(cli.path())));
    let s = h.status();
    assert!(!s.connected);
    let error = s.error.unwrap();
    assert!(
        error.contains("Unrecognized subcommand 'subscribe'"),
        "{error}"
    );
    assert!(error.contains("exit 2"), "{error}");
    assert!(h.stop() < Duration::from_secs(1));
}

#[test]
fn protocol_mismatch_without_cli_reports_an_error() {
    let path = temp_path(".sock");
    let _server = FakeServer::start(
        &path,
        Spec {
            handshake: Handshake::Version(7),
            ..Spec::default()
        },
    );
    let mut h = start(config(&path, None));
    let s = h.status();
    assert!(!s.connected);
    assert!(s.error.unwrap().contains("no aerospace CLI"));
    h.stop();
}

#[test]
fn rejected_subscribe_reports_the_server_error() {
    let path = temp_path(".sock");
    let _server = FakeServer::start(
        &path,
        Spec {
            reject_subscribe: Some(
                r#"{"exitCode":2,"stdout":"","stderr":"Unrecognized subcommand 'subscribe'","serverVersionAndHash":"x"}"#
                    .into(),
            ),
            ..Spec::default()
        },
    );
    let mut h = start(config(&path, None));
    assert_eq!(h.status(), connected_socket());
    let s = h.status();
    assert!(!s.connected);
    assert!(
        s.error.unwrap().contains("Unrecognized subcommand"),
        "status"
    );
    h.no_more_events();
    h.stop();
}

#[test]
fn dropping_from_a_callback_does_not_deadlock() {
    let path = temp_path(".sock");
    let server = FakeServer::start(&path, Spec::default());
    let subscribed = server.on_subscribe();
    let slot: std::sync::Arc<std::sync::Mutex<Option<Subscription>>> = Default::default();
    let (tx, rx) = mpsc::channel();
    let sub = {
        let slot = slot.clone();
        subscribe_with(
            config(&path, None),
            move |_| {
                if let Some(sub) = slot.lock().unwrap().take() {
                    drop(sub);
                    let _ = tx.send(());
                }
            },
            |_| {},
        )
    };
    *slot.lock().unwrap() = Some(sub);
    subscribed.recv_timeout(WAIT).unwrap();
    server.push(MODE_MAIN);
    rx.recv_timeout(WAIT).unwrap();
}
