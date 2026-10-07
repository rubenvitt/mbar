//! Client mode (`cli.md` §2.3): send argv to the daemon, print the response.
//!
//! * daemon not reachable / no reply → nothing printed, exit 0 (SketchyBar behaviour);
//! * response starting with `[!]` (second byte `!`) → stderr, exit 1;
//! * otherwise → stdout without an added newline, exit 0.
//!
//! `--monitor` (extension) keeps the connection open and prints every frame the daemon
//! streams until it closes the connection.

use std::io::Write;
use std::os::unix::net::UnixStream;

/// Runs client mode; returns the process exit code.
pub fn run(bar_name: &str, args: &[String]) -> i32 {
    // `sketchybar -m` with nothing to send.
    if args.is_empty() {
        return 0;
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
    let Ok(mut stream) = UnixStream::connect(&path) else {
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
