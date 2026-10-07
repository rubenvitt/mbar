//! Regression tests for the daemon's socket listener:
//!
//! * F3: the socket is mode 0600 regardless of the daemon's umask (a daemon started with
//!   `umask 000` used to create a world-connectable `srwxrwxrwx` socket).
//! * PERF-7: requests are read on the listener's poll loop, not on a thread per
//!   connection; clients that connect and stall neither spawn threads nor delay others.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");
const USER: &str = "mbarlisten";
const TIMEOUT: Duration = Duration::from_secs(15);

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
}

impl Sandbox {
    fn new(tmp_mode: u32) -> Sandbox {
        let root = PathBuf::from("/tmp").join(format!(
            "mblis-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let (home, tmp) = (root.join("home"), root.join("tmp"));
        for d in [&home, &tmp] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(tmp_mode)).unwrap();
        Sandbox { root, home, tmp }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(EXE);
        c.env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env("MBAR_LOCK_DIR", &self.tmp)
            .env("USER", USER)
            .env("MBAR_ALLOW_ROOT", "1")
            .stdin(Stdio::null());
        c
    }

    /// Starts a headless daemon under `umask` and waits for its socket to answer.
    fn daemon(&self, umask: libc::mode_t) -> (Daemon, PathBuf) {
        let mut cmd = self.command();
        cmd.arg("--headless")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: umask is async-signal-safe.
        unsafe {
            cmd.pre_exec(move || {
                libc::umask(umask);
                Ok(())
            });
        }
        let mut d = Daemon(cmd.spawn().unwrap());
        let start = Instant::now();
        loop {
            assert!(d.0.try_wait().unwrap().is_none(), "daemon exited early");
            if let Some(sock) = self.socket() {
                if query(&sock, &["--query", "bar"]).is_some_and(|r| r.starts_with('{')) {
                    return (d, sock);
                }
            }
            assert!(start.elapsed() < TIMEOUT, "daemon did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn socket(&self) -> Option<PathBuf> {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let name = format!("mbar_{USER}_mbar.socket");
        [
            self.tmp.join(&name),
            self.tmp.join(format!("mbar-{uid}")).join(&name),
        ]
        .into_iter()
        .find(|p| p.exists())
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn query(sock: &Path, args: &[&str]) -> Option<String> {
    mbar_ipc::socket::send(sock, &mbar_ipc::encode_args(args)).ok()
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn socket_is_private_under_permissive_umask_in_private_tmpdir() {
    let sb = Sandbox::new(0o700);
    let (_d, sock) = sb.daemon(0o000);
    assert_eq!(sock.parent().unwrap(), sb.tmp);
    assert_eq!(mode(&sock), 0o600, "socket mode under umask 000");
}

#[test]
fn socket_is_private_under_permissive_umask_in_shared_tmpdir() {
    let sb = Sandbox::new(0o1777);
    let (_d, sock) = sb.daemon(0o000);
    assert_ne!(sock.parent().unwrap(), sb.tmp);
    assert_eq!(mode(sock.parent().unwrap()), 0o700);
    assert_eq!(mode(&sock), 0o600, "socket mode under umask 000");
}

/// Names of the daemon's threads.
#[cfg(target_os = "linux")]
fn thread_names(pid: u32) -> Vec<String> {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("comm")).ok())
        .map(|s| s.trim().to_string())
        .collect()
}

#[test]
fn stalled_connections_neither_spawn_threads_nor_block_requests() {
    let sb = Sandbox::new(0o700);
    let (d, sock) = sb.daemon(0o022);
    let mut stalled = Vec::new();
    for i in 0..32 {
        let mut s = UnixStream::connect(&sock).unwrap();
        if i % 2 == 0 {
            // Half a header.
            s.write_all(&[20, 0]).unwrap();
        }
        stalled.push(s);
    }
    let start = Instant::now();
    let rsp = query(&sock, &["--add", "item", "a", "left"]).unwrap();
    assert_eq!(rsp, "");
    for i in 0..300 {
        let label = format!("label={i}");
        assert_eq!(query(&sock, &["--set", "a", &label]).unwrap(), "");
    }
    let item = query(&sock, &["--query", "a"]).unwrap();
    assert!(item.contains("\"299\""), "{item}");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "stalled clients slowed requests down ({:?})",
        start.elapsed()
    );
    #[cfg(target_os = "linux")]
    {
        let names = thread_names(d.0.id());
        assert!(
            !names.iter().any(|n| n.starts_with("mbar-ipc-conn")),
            "{names:?}"
        );
        assert!(names.len() < 20, "{} threads: {names:?}", names.len());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = &d;
    drop(stalled);
    assert!(query(&sock, &["--query", "bar"]).is_some());
}
