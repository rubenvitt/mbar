//! mbar IPC.
//!
//! A request is the client's argv (without argv[0]) encoded exactly like SketchyBar's
//! mach payload: every argument followed by `\0`, plus one terminating `\0`.
//! A response is UTF-8 text (possibly empty). Responses whose second byte is `!`
//! (`"[!] ..."`) are errors: the client prints them to stderr and exits with 1.
//!
//! Transports:
//! * Unix domain socket (all platforms), see [`socket`].
//! * mach bootstrap service `dev.rubeen.<bar_name>` (macOS), see [`mach`]. The server
//!   side lives in `mbar-macos` because it is driven by the main CFRunLoop.

pub mod socket;

#[cfg(target_os = "macos")]
pub mod mach;

use std::path::PathBuf;

/// Default bar name. A binary invoked as `sketchybar` also maps to this name, so
/// SketchyBar plugins reach mbar through a `sketchybar -> mbar` symlink.
pub const DEFAULT_BAR_NAME: &str = "mbar";

/// Prefix of the mach bootstrap service name.
pub const MACH_SERVICE_PREFIX: &str = "dev.rubeen.";

/// Derives the bar name from `argv[0]` (basename). `sketchybar` maps to `mbar`;
/// any other name (e.g. a `bottom_bar -> mbar` symlink) runs an independent instance.
pub fn bar_name_from_argv0(argv0: &str) -> String {
    let base = std::path::Path::new(argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(DEFAULT_BAR_NAME);
    match base {
        "" | "sketchybar" | "mbar" => DEFAULT_BAR_NAME.to_string(),
        other => other.to_string(),
    }
}

/// `dev.rubeen.<bar_name>`.
pub fn mach_service_name(bar_name: &str) -> String {
    format!("{MACH_SERVICE_PREFIX}{bar_name}")
}

/// Unix socket path: `$TMPDIR/mbar_<user>_<bar_name>.socket` (falls back to `/tmp`).
pub fn socket_path(bar_name: &str) -> PathBuf {
    let dir = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    dir.join(format!("mbar_{user}_{bar_name}.socket"))
}

/// Encodes arguments as `arg\0arg\0...\0` + `\0`.
pub fn encode_args<S: AsRef<str>>(args: &[S]) -> Vec<u8> {
    let mut out = Vec::with_capacity(args.iter().map(|a| a.as_ref().len() + 1).sum::<usize>() + 1);
    for a in args {
        out.extend_from_slice(a.as_ref().as_bytes());
        out.push(0);
    }
    out.push(0);
    out
}

/// Decodes a payload produced by [`encode_args`]. Tolerates a missing final
/// terminator. Empty arguments in the middle are preserved (`--set foo label=`
/// yields `"label="`, never an empty token, but `""` arguments are legal).
pub fn decode_args(payload: &[u8]) -> Vec<String> {
    let mut body = payload;
    // Strip the terminating empty argument.
    if body.last() == Some(&0) {
        body = &body[..body.len() - 1];
    }
    if body.is_empty() {
        return Vec::new();
    }
    if body.last() == Some(&0) {
        body = &body[..body.len() - 1];
    }
    body.split(|b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Returns true if a response denotes an error (SketchyBar convention: `"[!] ..."`).
pub fn is_error_response(rsp: &str) -> bool {
    rsp.len() > 2 && rsp.as_bytes()[1] == b'!'
}

/// Sends a request to a running daemon using the best available transport and
/// returns the response text.
pub fn send(bar_name: &str, args: &[String]) -> std::io::Result<String> {
    let payload = encode_args(args);
    #[cfg(target_os = "macos")]
    {
        if let Some(rsp) = mach::send(&mach_service_name(bar_name), &payload) {
            return Ok(rsp);
        }
    }
    socket::send(&socket_path(bar_name), &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let args = vec![
            "--set".to_string(),
            "clock".into(),
            "label=".into(),
            "".into(),
            "x y".into(),
        ];
        let enc = encode_args(&args);
        assert_eq!(enc, b"--set\0clock\0label=\0\0x y\0\0");
        assert_eq!(decode_args(&enc), args);
        assert!(decode_args(&encode_args::<&str>(&[])).is_empty());
    }

    #[test]
    fn names() {
        assert_eq!(bar_name_from_argv0("/usr/local/bin/sketchybar"), "mbar");
        assert_eq!(bar_name_from_argv0("mbar"), "mbar");
        assert_eq!(bar_name_from_argv0("/x/bottom_bar"), "bottom_bar");
        assert_eq!(mach_service_name("mbar"), "dev.rubeen.mbar");
    }

    #[test]
    fn errors() {
        assert!(is_error_response("[!] Item not found\n"));
        assert!(!is_error_response("[?] hm"));
        assert!(!is_error_response("{}"));
    }
}
