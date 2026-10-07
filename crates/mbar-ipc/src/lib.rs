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

/// `basename(argv[0])`: SketchyBar's `g_name` as the user sees it (`cli.md` §1.1). This is
/// the value exported as `BAR_NAME` (step 3), so a daemon started through a
/// `sketchybar -> mbar` symlink gives its scripts `BAR_NAME=sketchybar`
/// (`examples.md` §0). The IPC identity comes from [`bar_name_from_argv0`] instead.
pub fn program_name(argv0: &str) -> String {
    std::path::Path::new(argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BAR_NAME)
        .to_string()
}

/// Derives the bar name (IPC identity: socket, lock file, mach service, config
/// directory) from `argv[0]` (basename). `sketchybar` maps to `mbar`; any other name
/// (e.g. a `bottom_bar -> mbar` symlink) runs an independent instance.
pub fn bar_name_from_argv0(argv0: &str) -> String {
    match program_name(argv0).as_str() {
        "sketchybar" | "mbar" => DEFAULT_BAR_NAME.to_string(),
        other => other.to_string(),
    }
}

/// `dev.rubeen.<bar_name>`.
pub fn mach_service_name(bar_name: &str) -> String {
    format!("{MACH_SERVICE_PREFIX}{bar_name}")
}

/// Effective uid of this process.
pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// A directory only this user can create entries in: owned by the effective uid and
/// neither group- nor world-writable.
fn is_private_dir(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.is_dir() && meta.uid() == euid() && meta.mode() & 0o022 == 0
}

/// `base` itself when it is private to this user, otherwise mbar's own per-user
/// `mbar-<uid>` subdirectory of it (`true`).
fn private_dir_in(base: PathBuf) -> (PathBuf, bool) {
    if std::fs::metadata(&base).is_ok_and(|m| is_private_dir(&m)) {
        (base, false)
    } else {
        (base.join(format!("mbar-{}", euid())), true)
    }
}

/// Where the socket lives, and whether that is mbar's own per-user subdirectory of a
/// shared directory.
fn socket_dir_choice() -> (PathBuf, bool) {
    let base = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    private_dir_in(base)
}

/// Creates `dir` with mode 0700 when it is mbar's own subdirectory (`subdir`) and
/// verifies that it belongs to this user and is not writable by anybody else. A
/// directory pre-created (or symlinked) by another user is refused.
fn ensure_private_dir(dir: &std::path::Path, subdir: bool) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::unix::fs::DirBuilderExt;
    let meta = if subdir {
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        // Never follow a symlink planted in the shared parent directory.
        std::fs::symlink_metadata(dir)?
    } else {
        std::fs::metadata(dir)?
    };
    if !is_private_dir(&meta) {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            format!(
                "{} is not a directory private to uid {}",
                dir.display(),
                euid()
            ),
        ));
    }
    Ok(())
}

/// Directory of the socket: `$TMPDIR` (falling back to `/tmp`) when it is private to
/// this user (the per-user macOS `$TMPDIR`, a test sandbox), otherwise a `mbar-<uid>`
/// subdirectory with mode 0700 (e.g. `/tmp/mbar-1000` on Linux). A fixed name in a
/// shared, world-writable directory could be bound first by another local user, who
/// would then receive every client request and answer every `--query`.
pub fn socket_dir() -> PathBuf {
    socket_dir_choice().0
}

/// Unix socket path: `<socket_dir>/mbar_<user>_<bar_name>.socket` (see [`socket_dir`]).
pub fn socket_path(bar_name: &str) -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    socket_dir().join(format!("mbar_{user}_{bar_name}.socket"))
}

/// Daemon side of [`socket_path`]: creates the per-user directory if needed (mode 0700)
/// and verifies that the socket directory belongs to this user and is not writable by
/// anybody else. A directory pre-created (or symlinked) by another user is refused.
pub fn prepare_socket_path(bar_name: &str) -> std::io::Result<PathBuf> {
    let (dir, subdir) = socket_dir_choice();
    ensure_private_dir(&dir, subdir)?;
    Ok(socket_path(bar_name))
}

/// Test hook: base directory of the daemon's lock file instead of `/tmp`, so a test
/// suite can run independent daemons side by side. Not meant for normal use: every
/// daemon of one user and bar name must see the same lock file.
pub const LOCK_DIR_ENV: &str = "MBAR_LOCK_DIR";

/// Directory of the single-instance lock file. SketchyBar uses the fixed
/// `/tmp/<g_name>_<USER>.lock` (`cli.md` §1.1), independent of `$TMPDIR`, so a daemon
/// started by a service manager and one started from a shell with a different
/// `$TMPDIR` still exclude each other. mbar keeps that base (`/tmp`, or
/// [`LOCK_DIR_ENV`] when set) but, like the socket, uses the private `mbar-<uid>`
/// subdirectory of it when the base is shared, so another local user cannot squat the
/// lock file.
pub fn lock_dir() -> PathBuf {
    lock_dir_choice().0
}

fn lock_dir_choice() -> (PathBuf, bool) {
    let base = std::env::var_os(LOCK_DIR_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    private_dir_in(base)
}

/// Daemon lock file `<lock_dir>/mbar_<user>_<bar_name>.lock` (see [`lock_dir`]). Creates
/// the per-user directory if needed and refuses one that is not private to this user.
pub fn prepare_lock_path(user: &str, bar_name: &str) -> std::io::Result<PathBuf> {
    let (dir, subdir) = lock_dir_choice();
    ensure_private_dir(&dir, subdir)?;
    Ok(dir.join(format!("mbar_{user}_{bar_name}.lock")))
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
        // `BAR_NAME` keeps the invoked name (`cli.md` §1.1 step 3).
        assert_eq!(program_name("/usr/local/bin/sketchybar"), "sketchybar");
        assert_eq!(program_name("mbar"), "mbar");
        assert_eq!(program_name("/x/bottom_bar"), "bottom_bar");
        assert_eq!(program_name(""), "mbar");
        assert_eq!(bar_name_from_argv0(""), "mbar");
    }

    #[test]
    fn errors() {
        assert!(is_error_response("[!] Item not found\n"));
        assert!(!is_error_response("[?] hm"));
        assert!(!is_error_response("{}"));
    }
}
