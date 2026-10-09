//! Transport to a running [AeroSpace](https://github.com/nikitabobko/AeroSpace)
//! (`docs/superpowers/specs/2026-10-09-aerospace-design.md`, "Transport").
//!
//! * **Socket** (AeroSpace `docs/guide.adoc`, "Socket protocol", see [`protocol`]):
//!   `/tmp/bobko.aerospace-$USER.sock`, a `u32 LE` version handshake, then
//!   length-prefixed JSON frames. [`run`] sends one `ClientRequest` per connection;
//!   [`subscribe`] sends `subscribe --all` and reads the event stream.
//! * **CLI fallback** for servers that predate the handshake (answered with another
//!   version, closed, or not answered within 1 s): commands run as `aerospace <args>`,
//!   events come from one `aerospace subscribe --all` child (one JSON object per line).
//! * **Reconnect**: the subscription retries forever with backoff (1 s, doubling, at
//!   most 30 s; reset after a successful connection) and reports every status change.
//!
//! Environment overrides (read by [`Config::from_env`], used by [`run`] and [`subscribe`]):
//! `MBAR_AEROSPACE_SOCKET` (socket path), `MBAR_AEROSPACE_CLI` (the `aerospace` binary),
//! `MBAR_AEROSPACE_BACKOFF_MS` (initial reconnect delay; for tests). Tests running in
//! parallel should build a [`Config`] and call [`run_with`] / [`subscribe_with`]
//! instead, since environment variables are process-global.
//!
//! Platform-independent (std threads, no async runtime): the tests run on Linux against
//! a fake server.

mod cli;
pub mod protocol;
mod subscribe;

use std::ffi::CStr;
use std::fmt;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use subscribe::{subscribe, subscribe_with, Subscription};

/// AeroSpace's `SOCKET_PROTOCOL_VERSION` that mbar speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// Overrides [`socket_path`].
pub const SOCKET_ENV: &str = "MBAR_AEROSPACE_SOCKET";
/// Overrides the `aerospace` CLI lookup ([`find_cli`]).
pub const CLI_ENV: &str = "MBAR_AEROSPACE_CLI";
/// Overrides the initial reconnect delay in milliseconds ([`Config::from_env`]).
pub const BACKOFF_ENV: &str = "MBAR_AEROSPACE_BACKOFF_MS";

/// Where the `aerospace` CLI is looked for after `PATH` (Homebrew on Apple Silicon and
/// Intel, and the binary shipped inside the app bundle).
pub const CLI_DIRS: [&str; 3] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/Applications/AeroSpace.app/Contents/Resources/bin",
];

/// Initial reconnect delay.
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
/// Longest reconnect delay.
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// How long the server may take to answer the handshake before mbar assumes a server
/// without the socket protocol.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long one command may take (AeroSpace's accessibility calls can block for seconds).
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// AeroSpace's socket: `$MBAR_AEROSPACE_SOCKET`, else [`default_socket_path`].
pub fn socket_path() -> PathBuf {
    match std::env::var_os(SOCKET_ENV) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => default_socket_path(),
    }
}

/// `/tmp/bobko.aerospace-<user>.sock` (AeroSpace `commonUtil.swift`; the user is
/// `NSUserName()` there): `$USER`, else the passwd entry of the real uid.
pub fn default_socket_path() -> PathBuf {
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(passwd_user_name)
        .unwrap_or_default();
    PathBuf::from(format!("/tmp/bobko.aerospace-{user}.sock"))
}

/// The login name of the real uid from the passwd database.
fn passwd_user_name() -> Option<String> {
    let mut buf: Vec<libc::c_char> = vec![0; 1024];
    loop {
        // SAFETY: `passwd` is a plain C struct of integers and pointers; all-zero is a
        // valid (if meaningless) value that getpwuid_r overwrites.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: all pointers are valid for the duration of the call and `buf.len()` is
        // the real size of `buf`; getpwuid_r writes only into `pwd` and `buf`.
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_name.is_null() {
            return None;
        }
        // SAFETY: on success `pw_name` points to a NUL-terminated string inside `buf`,
        // which is still alive here.
        let name = unsafe { CStr::from_ptr(pwd.pw_name) };
        return Some(name.to_string_lossy().into_owned()).filter(|n| !n.is_empty());
    }
}

/// The `aerospace` CLI: `$MBAR_AEROSPACE_CLI`, else the first executable `aerospace` on
/// `PATH`, else in [`CLI_DIRS`].
pub fn find_cli() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(CLI_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let path_dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    path_dirs
        .into_iter()
        .chain(CLI_DIRS.iter().map(PathBuf::from))
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join("aerospace"))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The result of one command, from the socket's `ServerAnswer` or the CLI's output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Answer {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// `serverVersionAndHash` (socket only; `None` for the CLI fallback).
    pub server_version: Option<String>,
}

/// Why a command could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// AeroSpace is not running (no socket / connection refused, and the CLI fallback
    /// is missing or could not connect either).
    NotRunning(String),
    /// The server does not speak the socket protocol and there is no usable CLI
    /// fallback, or it sent something unparsable.
    Protocol(String),
    /// I/O failure or timeout talking to AeroSpace.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotRunning(m) => write!(f, "AeroSpace is not running: {m}"),
            Error::Protocol(m) => write!(f, "AeroSpace protocol error: {m}"),
            Error::Io(m) => write!(f, "AeroSpace I/O error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Where and how to reach AeroSpace. [`Config::from_env`] is what [`run`] and
/// [`subscribe`] use; tests build their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// AeroSpace's socket.
    pub socket: PathBuf,
    /// The `aerospace` CLI for the fallback (`None`: no fallback).
    pub cli: Option<PathBuf>,
    /// First reconnect delay (doubles up to `max_backoff`).
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// Handshake answer timeout; exceeding it means "server without socket protocol".
    pub handshake_timeout: Duration,
    /// Timeout for one command's answer (socket and CLI).
    pub answer_timeout: Duration,
}

impl Config {
    /// `socket` and `cli` with the default timings ([`INITIAL_BACKOFF`],
    /// [`MAX_BACKOFF`], [`HANDSHAKE_TIMEOUT`], [`ANSWER_TIMEOUT`]).
    pub fn new(socket: PathBuf, cli: Option<PathBuf>) -> Config {
        Config {
            socket,
            cli,
            initial_backoff: INITIAL_BACKOFF,
            max_backoff: MAX_BACKOFF,
            handshake_timeout: HANDSHAKE_TIMEOUT,
            answer_timeout: ANSWER_TIMEOUT,
        }
    }

    /// [`socket_path`], [`find_cli`], and `MBAR_AEROSPACE_BACKOFF_MS` (milliseconds) as
    /// the initial backoff when set.
    pub fn from_env() -> Config {
        let mut config = Config::new(socket_path(), find_cli());
        if let Some(ms) = std::env::var(BACKOFF_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
        {
            config.initial_backoff = Duration::from_millis(ms.max(1));
        }
        config
    }
}

/// One command (socket, else CLI). Blocking: call it off the main thread.
pub fn run(args: &[String]) -> Result<Answer, Error> {
    run_with(&Config::from_env(), args)
}

/// [`run`] with an explicit [`Config`].
///
/// The socket is tried first. When the server does not speak the protocol (handshake
/// failure) or cannot be reached at all, the CLI runs the command instead. Once the
/// request went out over the socket there is no fallback (the command may have run).
pub fn run_with(config: &Config, args: &[String]) -> Result<Answer, Error> {
    let fallback_error = match socket_run(config, args) {
        Ok(answer) => return Ok(answer),
        Err(Failure::Fatal(e)) => return Err(e),
        Err(Failure::Unreachable(e)) => e,
        Err(Failure::Handshake(msg)) => Error::Protocol(msg),
    };
    let Some(cli) = &config.cli else {
        return Err(with_note(fallback_error, "no aerospace CLI found"));
    };
    match cli::run(cli, args, config.answer_timeout) {
        Ok(a) if a.exit_code != 0 && a.stderr.contains("Can't connect to AeroSpace server") => {
            Err(Error::NotRunning(a.stderr))
        }
        Ok(a) => Ok(a),
        Err(cli::CliError::Spawn(e)) => Err(with_note(
            fallback_error,
            &format!("cannot run {}: {e}", cli.display()),
        )),
        Err(cli::CliError::Timeout) => Err(Error::Io(format!(
            "{} did not finish within {:?}",
            cli.display(),
            config.answer_timeout
        ))),
    }
}

fn with_note(e: Error, note: &str) -> Error {
    match e {
        Error::NotRunning(m) => Error::NotRunning(format!("{m}; {note}")),
        Error::Protocol(m) => Error::Protocol(format!("{m}; {note}")),
        Error::Io(m) => Error::Io(format!("{m}; {note}")),
    }
}

/// Why the socket path did not produce an answer.
pub(crate) enum Failure {
    /// Connecting failed ([`Error::NotRunning`] or [`Error::Io`]); the CLI may still work.
    Unreachable(Error),
    /// The handshake failed: a server without the socket protocol; use the CLI.
    Handshake(String),
    /// Failed after the handshake; no fallback.
    Fatal(Error),
}

/// Connects to `config.socket` (calling `register` with the fresh stream, so a
/// subscription can shut it down from another thread) and performs the handshake.
pub(crate) fn connect(
    config: &Config,
    register: &dyn Fn(&UnixStream),
) -> Result<UnixStream, Failure> {
    let mut stream = UnixStream::connect(&config.socket).map_err(|e| {
        Failure::Unreachable(match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => Error::NotRunning(
                format!("cannot connect to {}: {e}", config.socket.display()),
            ),
            _ => Error::Io(format!(
                "cannot connect to {}: {e}",
                config.socket.display()
            )),
        })
    })?;
    register(&stream);
    let timeout = Some(nonzero(config.handshake_timeout));
    stream
        .set_read_timeout(timeout)
        .and_then(|()| stream.set_write_timeout(timeout))
        .map_err(|e| Failure::Unreachable(Error::Io(e.to_string())))?;
    protocol::handshake(&mut stream).map_err(Failure::Handshake)?;
    Ok(stream)
}

/// One request/answer exchange on a handshaken stream, with `timeout` for both.
pub(crate) fn exchange(
    stream: &mut UnixStream,
    args: &[String],
    timeout: Duration,
) -> Result<Answer, Error> {
    let timeout = nonzero(timeout);
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|e| Error::Io(e.to_string()))?;
    protocol::write_frame(stream, protocol::client_request(args, "").as_bytes())
        .map_err(|e| Error::Io(format!("sending the request failed: {e}")))?;
    let frame = protocol::read_frame(stream).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData => Error::Protocol(e.to_string()),
        io::ErrorKind::UnexpectedEof => {
            Error::Io("AeroSpace closed the connection without answering".into())
        }
        _ if protocol::is_timeout(&e) => {
            Error::Io(format!("AeroSpace did not answer within {timeout:?}"))
        }
        _ => Error::Io(format!("reading the answer failed: {e}")),
    })?;
    protocol::parse_server_answer(&frame).map_err(Error::Protocol)
}

fn socket_run(config: &Config, args: &[String]) -> Result<Answer, Failure> {
    let mut stream = connect(config, &|_| {})?;
    exchange(&mut stream, args, config.answer_timeout).map_err(Failure::Fatal)
}

/// Socket timeouts must not be zero (`set_read_timeout(Some(0))` is an error).
pub(crate) fn nonzero(d: Duration) -> Duration {
    d.max(Duration::from_millis(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_path_has_the_aerospace_shape() {
        let p = default_socket_path();
        let s = p.to_str().unwrap();
        assert!(s.starts_with("/tmp/bobko.aerospace-"), "{s}");
        assert!(s.ends_with(".sock"), "{s}");
    }

    #[test]
    fn passwd_lookup_does_not_fail_for_the_current_user() {
        // Containers may lack a passwd entry; the call must just not crash.
        let _ = passwd_user_name();
    }

    #[test]
    fn config_defaults() {
        let c = Config::new("/tmp/x.sock".into(), None);
        assert_eq!(c.initial_backoff, Duration::from_secs(1));
        assert_eq!(c.max_backoff, Duration::from_secs(30));
        assert_eq!(c.handshake_timeout, Duration::from_secs(1));
        assert_eq!(c.answer_timeout, Duration::from_secs(10));
    }
}
