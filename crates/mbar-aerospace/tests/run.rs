//! `run_with` against a fake AeroSpace server and fake CLI scripts.

mod common;

use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{config, fake_cli, temp_path, FakeServer, Handshake, Spec, SERVER_VERSION};
use mbar_aerospace::{run_with, Answer, Error};

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn runs_a_command_over_the_socket() {
    let path = temp_path(".sock");
    let server = FakeServer::start(&path, Spec::default());
    let answer = run_with(&config(&path, None), &args(&["list-workspaces", "--all"])).unwrap();
    assert_eq!(
        answer,
        Answer {
            exit_code: 0,
            stdout: "ran list-workspaces --all".into(),
            stderr: String::new(),
            server_version: Some(SERVER_VERSION.into()),
        }
    );
    assert_eq!(
        server.requests.lock().unwrap().as_slice(),
        [r#"{"args":["list-workspaces","--all"],"stdin":"","windowId":null,"workspace":null}"#]
    );
}

#[test]
fn reports_a_non_zero_exit() {
    let path = temp_path(".sock");
    let _server = FakeServer::start(
        &path,
        Spec {
            answer: Arc::new(|_: &[String]| {
                (2, String::new(), "Unrecognized subcommand 'x'".into())
            }),
            ..Spec::default()
        },
    );
    let answer = run_with(&config(&path, None), &args(&["x"])).unwrap();
    assert_eq!(answer.exit_code, 2);
    assert_eq!(answer.stderr, "Unrecognized subcommand 'x'");
    assert_eq!(answer.server_version.as_deref(), Some(SERVER_VERSION));
}

const ECHO_CLI: &str = r#"printf 'cli:%s\n' "$*"
echo warn >&2
exit 3"#;

fn assert_cli_answer(answer: Answer) {
    assert_eq!(
        answer,
        Answer {
            exit_code: 3,
            stdout: "cli:workspace 3".into(),
            stderr: "warn".into(),
            server_version: None,
        }
    );
}

#[test]
fn version_mismatch_falls_back_to_the_cli() {
    let path = temp_path(".sock");
    let server = FakeServer::start(
        &path,
        Spec {
            handshake: Handshake::Version(7),
            ..Spec::default()
        },
    );
    let cli = fake_cli(ECHO_CLI);
    assert_cli_answer(
        run_with(&config(&path, Some(cli.path())), &args(&["workspace", "3"])).unwrap(),
    );
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn closed_or_silent_handshake_falls_back_to_the_cli() {
    let cli = fake_cli(ECHO_CLI);
    for handshake in [Handshake::Close, Handshake::Silent] {
        let path = temp_path(".sock");
        let _server = FakeServer::start(
            &path,
            Spec {
                handshake,
                ..Spec::default()
            },
        );
        let start = Instant::now();
        let answer = run_with(&config(&path, Some(cli.path())), &args(&["workspace", "3"]));
        assert_cli_answer(answer.unwrap());
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}

#[test]
fn version_mismatch_without_cli_is_a_protocol_error() {
    let path = temp_path(".sock");
    let _server = FakeServer::start(
        &path,
        Spec {
            handshake: Handshake::Version(7),
            ..Spec::default()
        },
    );
    let err = run_with(&config(&path, None), &args(&["workspace", "3"])).unwrap_err();
    assert!(
        matches!(&err, Error::Protocol(m) if m.contains("version 7")),
        "{err:?}"
    );
    let missing = Some("/nonexistent/aerospace".into());
    let err = run_with(&config(&path, missing), &args(&["workspace", "3"])).unwrap_err();
    assert!(matches!(err, Error::Protocol(_)), "{err:?}");
}

#[test]
fn not_running_without_socket_or_cli() {
    let path = temp_path(".sock");
    let err = run_with(&config(&path, None), &args(&["workspace", "1"])).unwrap_err();
    assert!(matches!(err, Error::NotRunning(_)), "{err:?}");

    // A stale socket file (AeroSpace crashed): connection refused.
    let stale = temp_path(".sock");
    drop(UnixListener::bind(&stale).unwrap());
    let err = run_with(&config(&stale, None), &args(&["workspace", "1"])).unwrap_err();
    assert!(matches!(err, Error::NotRunning(_)), "{err:?}");
    let _ = std::fs::remove_file(&stale);

    // The CLI cannot connect either.
    let cli = fake_cli(
        "echo \"Can't connect to AeroSpace server. Is AeroSpace.app running?\" >&2\nexit 2",
    );
    let err = run_with(&config(&path, Some(cli.path())), &args(&["workspace", "1"])).unwrap_err();
    assert!(
        matches!(&err, Error::NotRunning(m) if m.contains("Can't connect")),
        "{err:?}"
    );
}

#[test]
fn missing_socket_uses_a_working_cli() {
    // The CLI may know a socket path mbar does not (e.g. NSUserName differs from $USER).
    let path = temp_path(".sock");
    let cli = fake_cli(ECHO_CLI);
    assert_cli_answer(
        run_with(&config(&path, Some(cli.path())), &args(&["workspace", "3"])).unwrap(),
    );
}

#[test]
fn answer_timeout_is_an_io_error() {
    let path = temp_path(".sock");
    let _server = FakeServer::start(
        &path,
        Spec {
            hang_requests: true,
            ..Spec::default()
        },
    );
    let mut c = config(&path, None);
    c.answer_timeout = Duration::from_millis(200);
    let start = Instant::now();
    let err = run_with(&c, &args(&["workspace", "1"])).unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err:?}");
    assert!(start.elapsed() < Duration::from_secs(3));
}
