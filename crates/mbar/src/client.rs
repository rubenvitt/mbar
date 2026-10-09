//! Client mode (`cli.md` §2.3): send argv to the daemon, print the response.
//!
//! * daemon not reachable / no reply → nothing printed, exit 0 (SketchyBar behaviour);
//! * response starting with `[!]` (second byte `!`) → stderr, exit 1;
//! * otherwise → stdout without an added newline, exit 0.
//!
//! `--monitor` (extension) keeps the connection open and prints every frame the daemon
//! streams until it closes the connection.
//!
//! [`run_borders`] is the client of the `borders` argv0 mode (borders design §2,
//! `docs/spec/borders.md` BR-CLI-03): it waits for the daemon instead of giving up, and
//! reports a missing daemon instead of exiting silently.

use std::ffi::OsStr;
use std::io::Write;
use std::time::{Duration, Instant};

/// Runs client mode; returns the process exit code.
pub fn run(bar_name: &str, args: &[String]) -> i32 {
    // `sketchybar -m` with nothing to send.
    if args.is_empty() {
        return 0;
    }
    if lua_sync_marked(
        bar_name,
        std::env::var_os(mbar_lua::SYNC_SHELL_ENV).as_deref(),
    ) {
        eprintln!(
            "{bar_name}: cannot message the bar from a blocking io.popen/os.execute of its own \
             Lua config or handler (the daemon cannot answer before Lua returns); use \
             mbar.query/mbar.exec instead"
        );
        return 1;
    }
    if std::env::var_os("USER").map_or(true, |u| u.is_empty()) {
        eprintln!("sketchybar-msg: 'env USER' not set! abort..");
        return 1;
    }
    if args.iter().any(|a| a == "--monitor") {
        return monitor(bar_name, args);
    }
    match mbar_ipc::send(bar_name, args) {
        Ok(rsp) => print_response(&rsp),
        Err(e) => {
            log::debug!("client: {e}");
            0
        }
    }
}

/// JankyBorders' message when a primary runs and no argument was valid (BR-CLI-03 step 4;
/// the typo "where" is in the original).
pub const BORDERS_ALREADY_RUNNING: &str = "A borders instance is already running and no valid \
arguments where provided. To modify properties of the running instance provide them as \
arguments.\n";

/// The `borders` client found no mbar daemon (borders design §2).
pub const BORDERS_NOT_RUNNING: &str =
    "borders: mbar is not running. mbar draws the window borders; start mbar.app.\n";

/// How long the `borders` client waits for the daemon to come up: window-manager
/// startup commands (AeroSpace `after-startup-command`, a `yabairc`) can run before the
/// mbar login item is up.
const BORDERS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Pause between two connection attempts of the `borders` client.
const BORDERS_RETRY_INTERVAL: Duration = Duration::from_millis(200);

/// Test hook: overrides [`BORDERS_CONNECT_TIMEOUT`] (milliseconds), so tests of a
/// missing daemon do not wait 5 s. Not meant for normal use.
pub const BORDERS_CONNECT_TIMEOUT_ENV: &str = "MBAR_BORDERS_CONNECT_TIMEOUT_MS";

/// [`BORDERS_CONNECT_TIMEOUT`], or the value of [`BORDERS_CONNECT_TIMEOUT_ENV`] when it
/// is a number of milliseconds.
fn borders_connect_timeout(env: Option<&OsStr>) -> Duration {
    env.and_then(OsStr::to_str)
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(BORDERS_CONNECT_TIMEOUT, Duration::from_millis)
}

/// The `borders` client (borders design §2): `valid` are the arguments that passed
/// `mbar_core::borders::validate_args` (the caller already printed the error lines of
/// the others). Returns the process exit code.
///
/// * no valid argument → stderr [`BORDERS_ALREADY_RUNNING`] when the daemon runs, else
///   [`BORDERS_NOT_RUNNING`]; exit 1. A daemon is never started.
/// * otherwise `--borders <valid…>` goes to `bar_name`, retried until the daemon accepts
///   a connection or the connect timeout ran out (then [`BORDERS_NOT_RUNNING`], exit 1).
///   JankyBorders' client exits 0 once sent, so the reply is ignored except an `[!]`
///   error, which goes to stderr (still exit 0).
pub fn run_borders(bar_name: &str, valid: &[String]) -> i32 {
    if std::env::var_os("USER").map_or(true, |u| u.is_empty()) {
        eprintln!("borders: 'env USER' not set! abort..");
        return 1;
    }
    if valid.is_empty() {
        eprint!(
            "{}",
            if mbar_ipc::is_running(bar_name) {
                BORDERS_ALREADY_RUNNING
            } else {
                BORDERS_NOT_RUNNING
            }
        );
        return 1;
    }
    if lua_sync_marked(
        bar_name,
        std::env::var_os(mbar_lua::SYNC_SHELL_ENV).as_deref(),
    ) {
        eprintln!(
            "borders: cannot message the bar from a blocking io.popen/os.execute of its own \
             Lua config or handler; use mbar.borders or mbar.exec instead"
        );
        return 1;
    }
    let mut args = Vec::with_capacity(valid.len() + 1);
    args.push("--borders".to_string());
    args.extend_from_slice(valid);
    let deadline = Instant::now()
        + borders_connect_timeout(std::env::var_os(BORDERS_CONNECT_TIMEOUT_ENV).as_deref());
    loop {
        match mbar_ipc::try_send(bar_name, &args) {
            Ok(rsp) => {
                if mbar_ipc::is_error_response(&rsp) {
                    let mut err = std::io::stderr().lock();
                    let _ = err.write_all(rsp.as_bytes());
                    let _ = err.flush();
                }
                return 0;
            }
            Err(mbar_ipc::SendError::Failed(e)) => {
                // Possibly delivered: never send twice.
                log::debug!("borders: {e}");
                return 0;
            }
            Err(mbar_ipc::SendError::NotRunning(e)) => {
                let now = Instant::now();
                if now >= deadline {
                    log::debug!("borders: {e}");
                    eprint!("{BORDERS_NOT_RUNNING}");
                    return 1;
                }
                std::thread::sleep(BORDERS_RETRY_INTERVAL.min(deadline - now));
            }
        }
    }
}

/// Whether this process was started (directly or indirectly) by a blocking
/// `io.popen` / `os.execute` of the Lua engine of the daemon `bar_name`
/// (`mbar_lua::SYNC_SHELL_ENV`). Messaging that daemon would only wait for the
/// client timeout: its main thread, the only one that answers, is busy running
/// the very Lua call that waits for this process.
fn lua_sync_marked(bar_name: &str, marker: Option<&std::ffi::OsStr>) -> bool {
    marker.is_some_and(|m| m == bar_name)
}

fn print_response(rsp: &str) -> i32 {
    if mbar_ipc::is_error_response(rsp) {
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(rsp.as_bytes());
        let _ = err.flush();
        1
    } else {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(rsp.as_bytes());
        let _ = out.flush();
        0
    }
}

/// `--monitor`: one request, then length-prefixed frames (each one or more JSON lines)
/// until EOF. An error response as first frame exits with 1.
fn monitor(bar_name: &str, args: &[String]) -> i32 {
    let path = mbar_ipc::socket_path(bar_name);
    let Ok(mut stream) = mbar_ipc::socket::connect(&path) else {
        return 0;
    };
    let _ = stream.set_write_timeout(Some(mbar_ipc::socket::CLIENT_TIMEOUT));
    if mbar_ipc::socket::write_frame(&mut stream, &mbar_ipc::encode_args(args)).is_err() {
        return 0;
    }
    let mut first = true;
    let mut out = std::io::stdout();
    while let Ok(frame) = mbar_ipc::socket::read_frame(&mut stream) {
        let text = String::from_utf8_lossy(&frame);
        if first && mbar_ipc::is_error_response(&text) {
            eprint!("{text}");
            return 1;
        }
        first = false;
        if out.write_all(text.as_bytes()).is_err() || out.flush().is_err() {
            // stdout closed (e.g. `mbar --monitor | head`).
            return 0;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borders_connect_timeout_override() {
        assert_eq!(borders_connect_timeout(None), BORDERS_CONNECT_TIMEOUT);
        assert_eq!(
            borders_connect_timeout(Some(OsStr::new("250"))),
            Duration::from_millis(250)
        );
        assert_eq!(
            borders_connect_timeout(Some(OsStr::new("0"))),
            Duration::ZERO
        );
        // Not a number of milliseconds: the default.
        assert_eq!(
            borders_connect_timeout(Some(OsStr::new("5s"))),
            BORDERS_CONNECT_TIMEOUT
        );
        assert_eq!(
            borders_connect_timeout(Some(OsStr::new(""))),
            BORDERS_CONNECT_TIMEOUT
        );
    }

    /// BR-CLI-03 step 4, typo included, on one line.
    #[test]
    fn borders_already_running_is_jankyborders_text() {
        assert_eq!(
            BORDERS_ALREADY_RUNNING,
            "A borders instance is already running and no valid arguments where provided. \
             To modify properties of the running instance provide them as arguments.\n"
        );
    }
}
