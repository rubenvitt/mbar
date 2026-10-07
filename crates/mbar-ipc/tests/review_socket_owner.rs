//! Regression tests for F2: the IPC socket must not be usable by another local user.
//!
//! * `socket_dir` / `prepare_socket_path` never place the socket directly in a shared
//!   (world-writable) directory such as `/tmp`, and refuse a per-user directory that is
//!   not private to the user (wrong owner, group/world writable, planted symlink).
//! * The client refuses a listener run by another user (a squatted socket), and the
//!   server drops connections from another user.
//!
//! The cross-user cases re-run this test binary as uid/gid 65534 and therefore only run
//! as root; otherwise they are skipped.

use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HELPER_ROLE: &str = "MBAR_REVIEW_F2_ROLE";
const HELPER_PATH: &str = "MBAR_REVIEW_F2_PATH";
const NOBODY: u32 = 65534;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// A fresh directory under `/tmp` with the given mode (removed on drop).
struct Dir(PathBuf);

impl Dir {
    fn new(mode: u32) -> Dir {
        let p = PathBuf::from("/tmp").join(format!(
            "mbf2-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The only test that touches `TMPDIR` (tests in one binary share the environment).
#[test]
fn socket_dir_is_private_to_the_user() {
    let saved = std::env::var_os("TMPDIR");
    std::env::set_var("USER", "f2user");

    // A shared, sticky, world-writable directory (like `/tmp`): private subdirectory.
    let shared = Dir::new(0o1777);
    std::env::set_var("TMPDIR", &shared.0);
    let sub = shared.0.join(format!("mbar-{}", euid()));
    assert_eq!(mbar_ipc::socket_dir(), sub);
    assert_eq!(
        mbar_ipc::socket_path("bar"),
        sub.join("mbar_f2user_bar.socket")
    );
    assert!(!sub.exists(), "socket_path must not create anything");
    let p = mbar_ipc::prepare_socket_path("bar").unwrap();
    assert_eq!(p, sub.join("mbar_f2user_bar.socket"));
    let meta = std::fs::symlink_metadata(&sub).unwrap();
    assert!(meta.is_dir());
    assert_eq!(meta.mode() & 0o777, 0o700);
    assert_eq!(meta.uid(), euid());
    // Idempotent.
    assert_eq!(mbar_ipc::prepare_socket_path("bar").unwrap(), p);

    // A pre-existing per-user directory that others can write to is refused.
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o777)).unwrap();
    let err = mbar_ipc::prepare_socket_path("bar").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    std::fs::remove_dir(&sub).unwrap();

    // A symlink planted in the shared directory is never followed.
    let target = Dir::new(0o700);
    std::os::unix::fs::symlink(&target.0, &sub).unwrap();
    let err = mbar_ipc::prepare_socket_path("bar").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    std::fs::remove_file(&sub).unwrap();

    // A directory created in advance by another user is refused.
    if euid() == 0 {
        std::fs::create_dir(&sub).unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::chown(&sub, Some(NOBODY), Some(NOBODY)).unwrap();
        let err = mbar_ipc::prepare_socket_path("bar").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }

    // A directory private to the user (macOS per-user `$TMPDIR`, test sandboxes) is
    // used as-is.
    let private = Dir::new(0o700);
    std::env::set_var("TMPDIR", &private.0);
    assert_eq!(mbar_ipc::socket_dir(), private.0);
    assert_eq!(
        mbar_ipc::prepare_socket_path("bar").unwrap(),
        private.0.join("mbar_f2user_bar.socket")
    );
    // ... but not when it is group- or world-writable.
    std::fs::set_permissions(&private.0, std::fs::Permissions::from_mode(0o770)).unwrap();
    assert_eq!(
        mbar_ipc::socket_dir(),
        private.0.join(format!("mbar-{}", euid()))
    );

    match saved {
        Some(v) => std::env::set_var("TMPDIR", v),
        None => std::env::remove_var("TMPDIR"),
    }
}

#[test]
fn peer_of_own_connection_is_trusted() {
    let (a, b) = UnixStream::pair().unwrap();
    assert_eq!(mbar_ipc::socket::peer_uid(&a).unwrap(), euid());
    assert!(mbar_ipc::socket::peer_trusted(&b));
}

/// Copies this test binary somewhere uid 65534 can execute it and returns a command
/// that runs `helper` there as that user. `None` when not running as root.
fn helper_command(dir: &Path, role: &str, path: &Path) -> Option<Command> {
    if euid() != 0 {
        eprintln!("skipped: needs root to run a second user");
        return None;
    }
    let exe = dir.join("helper");
    std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut c = Command::new(exe);
    c.args(["helper", "--exact", "--nocapture", "--test-threads=1"])
        .env(HELPER_ROLE, role)
        .env(HELPER_PATH, path)
        .uid(NOBODY)
        .gid(NOBODY)
        .stdin(Stdio::null());
    Some(c)
}

/// Runs as uid 65534 when spawned by the tests below; a no-op otherwise.
#[test]
fn helper() {
    let (Some(role), Some(path)) = (std::env::var_os(HELPER_ROLE), std::env::var_os(HELPER_PATH))
    else {
        return;
    };
    let path = PathBuf::from(path);
    match role.to_str().unwrap() {
        // Squat the socket path and answer every connection with fake JSON.
        "squat" => {
            let l = UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
            for conn in l.incoming() {
                let Ok(mut s) = conn else { continue };
                let _ = mbar_ipc::socket::write_frame(&mut s, b"{\"evil\":true}");
            }
        }
        // Talk to a server owned by another user and print what comes back.
        "client" => {
            let mut s = UnixStream::connect(&path).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            mbar_ipc::socket::write_frame(&mut s, &mbar_ipc::encode_args(&["--query", "bar"]))
                .unwrap();
            let mut rest = Vec::new();
            let _ = s.read_to_end(&mut rest);
            println!("REPLY:{}", String::from_utf8_lossy(&rest));
        }
        other => panic!("unknown role {other}"),
    }
}

#[test]
fn client_refuses_socket_of_another_user() {
    let dir = Dir::new(0o777);
    let sock = dir.0.join("s.socket");
    let Some(mut cmd) = helper_command(&dir.0, "squat", &sock) else {
        return;
    };
    let mut child = cmd.stdout(Stdio::null()).spawn().unwrap();
    let start = Instant::now();
    while UnixStream::connect(&sock).is_err() {
        assert!(start.elapsed() < Duration::from_secs(15), "squatter not up");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::metadata(&sock).unwrap().uid(), NOBODY);

    let res = mbar_ipc::socket::send(&sock, &mbar_ipc::encode_args(&["--query", "bar"]));
    let _ = child.kill();
    let _ = child.wait();
    let err = res.expect_err("client accepted a reply from another user's socket");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(mbar_ipc::socket::connect(&sock).is_err());
}

#[test]
fn server_drops_connections_of_another_user() {
    let dir = Dir::new(0o777);
    let sock = dir.0.join("s.socket");
    let Some(mut cmd) = helper_command(&dir.0, "client", &sock) else {
        return;
    };
    let server = mbar_ipc::socket::Server::bind(&sock).unwrap();
    // Bound sockets are 0600; open it up so only the peer check stands in the way.
    assert_eq!(std::fs::metadata(&sock).unwrap().mode() & 0o777, 0o600);
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o777)).unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits2 = hits.clone();
    server
        .spawn(move |req| {
            hits2.fetch_add(1, Ordering::SeqCst);
            req.respond("{\"secret\":true}");
        })
        .unwrap();
    let out = cmd.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("REPLY:"), "{text}");
    assert!(!text.contains("secret"), "{text}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    // The owner still gets through.
    let rsp = mbar_ipc::socket::send(&sock, &mbar_ipc::encode_args(&["x"])).unwrap();
    assert_eq!(rsp, "{\"secret\":true}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
