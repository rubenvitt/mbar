//! Termination signals: a headless daemon that receives `SIGTERM`, `SIGINT` or `SIGHUP`
//! shuts down like `--exit` (exit code 0, socket removed) and terminates the process
//! groups of the scripts it started, including their background children.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_mbar");
const USER: &str = "mbarsig";
const TIMEOUT: Duration = Duration::from_secs(15);

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        // Short: Unix socket paths are limited to ~100 bytes.
        let root = PathBuf::from("/tmp").join(format!(
            "mbsig-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let (home, tmp) = (root.join("home"), root.join("tmp"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
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
            .env("MBAR_ALLOW_ROOT", "1");
        c
    }

    fn socket(&self) -> PathBuf {
        self.tmp.join(format!("mbar_{USER}_mbar.socket"))
    }

    fn query_ok(&self) -> bool {
        self.command()
            .args(["--query", "bar"])
            .stdin(Stdio::null())
            .output()
            .map(|o| o.status.success() && o.stdout.starts_with(b"{"))
            .unwrap_or(false)
    }

    /// Starts `mbar --headless` and waits until it answers.
    fn daemon(&self) -> Child {
        let mut child = self
            .command()
            .arg("--headless")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("daemon exited early: {status}");
            }
            if self.socket().exists() && self.query_ok() {
                return child;
            }
            if start.elapsed() > TIMEOUT {
                let _ = child.kill();
                panic!("daemon did not become ready");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            panic!("daemon did not exit after the signal");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_file(path: &Path) -> String {
    let start = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if s.ends_with('\n') {
                return s;
            }
        }
        assert!(start.elapsed() < TIMEOUT, "{} not written", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Running (not a zombie waiting for a reaper).
fn alive(pid: i32) -> bool {
    let o = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&o.stdout).trim().to_string();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn signal(child: &Child, sig: libc::c_int) {
    // SAFETY: plain syscall on our own child.
    assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, sig) }, 0);
}

fn check_signal(sig: libc::c_int) {
    let sb = Sandbox::new();
    let mut child = sb.daemon();
    signal(&child, sig);
    let status = wait_exit(&mut child);
    assert_eq!(status.code(), Some(0), "signal {sig}: {status}");
    assert!(
        !sb.socket().exists(),
        "socket left behind after signal {sig}"
    );
    // The lock is released: a new daemon starts.
    let mut again = sb.daemon();
    signal(&again, libc::SIGTERM);
    assert_eq!(wait_exit(&mut again).code(), Some(0));
}

#[test]
fn sigterm_shuts_down_cleanly() {
    check_signal(libc::SIGTERM);
}

#[test]
fn sigint_shuts_down_cleanly() {
    check_signal(libc::SIGINT);
}

#[test]
fn sighup_shuts_down_cleanly() {
    check_signal(libc::SIGHUP);
}

/// Scripts still running and the background children of scripts that already exited
/// (the config's `cmd &`) are terminated with their process groups.
#[test]
fn sigterm_terminates_script_process_groups() {
    let sb = Sandbox::new();
    let dir = sb.home.join(".config/mbar");
    std::fs::create_dir_all(&dir).unwrap();
    let bg = sb.root.join("bg.pid");
    let fg = sb.root.join("fg.pid");
    // The config leaves a background `sleep` behind and exits; the item script keeps
    // running in the foreground of its shell.
    let rc = format!(
        "#!/bin/sh\n\
         sleep 300 &\n\
         echo $! > '{bg}'\n\
         mbar --add item a left --set a script='sleep 301 & echo $! > \"{fg}\"; wait' \
         --update\n",
        bg = bg.display(),
        fg = fg.display(),
    );
    let rc_path = dir.join("mbarrc");
    std::fs::write(&rc_path, rc).unwrap();
    // `mbar` must resolve to the binary under test inside the config.
    let bin = sb.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(EXE, bin.join("mbar")).unwrap();

    let mut child = sb
        .command()
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .arg("--headless")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let bg_pid: i32 = wait_file(&bg).trim().parse().unwrap();
    let fg_pid: i32 = wait_file(&fg).trim().parse().unwrap();
    assert!(alive(bg_pid) && alive(fg_pid));

    signal(&child, libc::SIGTERM);
    assert_eq!(wait_exit(&mut child).code(), Some(0));
    assert!(!sb.socket().exists());

    let start = Instant::now();
    while (alive(bg_pid) || alive(fg_pid)) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let left = (alive(bg_pid), alive(fg_pid));
    for pid in [bg_pid, fg_pid] {
        // SAFETY: cleanup of the test's own stray processes.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    assert_eq!(
        left,
        (false, false),
        "script processes survived the shutdown"
    );
}
