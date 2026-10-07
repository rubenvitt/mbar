//! Child processes: item scripts, click scripts, the shell config and Lua `mbar.exec`.
//!
//! `fork_exec` semantics (`cli.md` §10.2, §12.1, §14.8, D1):
//! `/usr/bin/env sh -c <command>` with a **fresh** environment (the daemon's startup
//! environment + `BAR_NAME`/`CONFIG_DIR` + the vars of this run), cwd = config directory,
//! stdin `/dev/null`, stdout/stderr inherited (they end up in the daemon log) unless the
//! output is captured.
//!
//! Timeout: like SketchyBar's `alarm(60)` in the `vfork` child, the timer is armed in the
//! child itself (`setitimer(ITIMER_REAL)` between `fork` and `exec`, `SIGALRM` reset to its
//! default action). A pending timer survives `exec` and is not inherited by the shell's own
//! children, so after 60 s only the direct `sh` gets `SIGALRM`: it terminates unless it
//! handles or ignores the signal (`item.md` §8.5), and nothing escalates to `SIGKILL`.
//!
//! A reaper thread blocks in `waitpid` (no polling) and reports the exit. Captured output
//! (Lua `mbar.exec` with a callback) is read by a second thread and ends with the shell:
//! once the shell has been reaped, whatever is still buffered in the pipe is drained and the
//! pipe is closed, so a background process that keeps the write end open (`cmd &`) neither
//! delays the callback nor feeds the daemon. At most [`MAX_CAPTURE`] bytes are kept; the
//! pipe is closed when the limit is reached (the writer gets `SIGPIPE`/`EPIPE`, like
//! `cmd | head -c`).
//!
//! Process groups: every child leads its own process group, so the daemon can end a script
//! together with everything it started in the background (`cmd &`) when it shuts down
//! ([`kill_process_groups`], after `--exit` or a termination signal), whether or not the
//! script itself has already exited. A group is tracked from the spawn until it has no
//! members left (checked when its leader is reaped and on later spawns); the 60 s
//! `SIGALRM` above still reaches only the shell.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

/// `alarm(60)` of every spawned shell.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum number of captured stdout bytes of one `mbar.exec` command.
pub const MAX_CAPTURE: usize = 4 << 20;

/// Process groups (= leader pids) of spawned children that may still have members.
static GROUPS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Tracked groups beyond which a spawn first drops the empty ones.
const GROUP_PRUNE_AT: usize = 64;

/// `killpg(pgid, 0)`: the group still has a member (a zombie leader counts) we may signal.
fn group_alive(pgid: u32) -> bool {
    // SAFETY: plain syscall; signal 0 only checks existence and permission.
    unsafe { libc::killpg(pgid as libc::pid_t, 0) == 0 }
}

fn groups() -> std::sync::MutexGuard<'static, Vec<u32>> {
    GROUPS.lock().unwrap_or_else(|e| e.into_inner())
}

fn track_group(pgid: u32) {
    let mut g = groups();
    if g.len() >= GROUP_PRUNE_AT {
        g.retain(|p| group_alive(*p));
    }
    g.push(pgid);
}

/// The leader of `pgid` was reaped: stop tracking the group unless background members
/// remain (then the id cannot be reused yet).
fn forget_group_if_empty(pgid: u32) {
    let mut g = groups();
    if !group_alive(pgid) {
        g.retain(|p| *p != pgid);
    }
}

/// Sends `sig` to the process group of every spawned child that still has members (daemon
/// shutdown). Returns the number of groups signalled.
pub fn kill_process_groups(sig: libc::c_int) -> usize {
    let mut g = groups();
    let mut n = 0;
    for pgid in g.drain(..) {
        // SAFETY: plain syscall on a group this process created; an empty group fails with
        // ESRCH and is skipped.
        if unsafe { libc::killpg(pgid as libc::pid_t, sig) } == 0 {
            n += 1;
        }
    }
    n
}

/// One child to spawn.
#[derive(Debug, Clone)]
pub struct Spawn {
    /// Shell command line (`sh -c`).
    pub command: String,
    /// Vars added to the base environment (later entries win).
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    /// Capture stdout (Lua `mbar.exec`).
    pub capture: bool,
}

/// Spawns `spec`; `on_exit(pid, output)` runs on the reaper thread when the child has
/// exited (`output` is `Some` only when captured).
pub fn spawn(
    spec: Spawn,
    base_env: &[(OsString, OsString)],
    timeout: Duration,
    on_exit: impl FnOnce(u32, Option<String>) + Send + 'static,
) -> std::io::Result<u32> {
    let env_bin = std::path::Path::new("/usr/bin/env");
    let mut cmd = if env_bin.exists() {
        let mut c = Command::new(env_bin);
        c.arg("sh");
        c
    } else {
        Command::new("sh")
    };
    cmd.arg("-c").arg(&spec.command);
    cmd.env_clear();
    cmd.envs(base_env.iter().map(|(k, v)| (k, v)));
    cmd.envs(spec.env.iter().map(|(k, v)| (k, v)));
    if let Some(dir) = spec.cwd.as_ref().filter(|d| d.is_dir()) {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null());
    if spec.capture {
        cmd.stdout(Stdio::piped());
    }
    cmd.process_group(0);
    let timer = itimer(timeout);
    // SAFETY: the closure runs in the forked child before `exec` and only makes
    // async-signal-safe system calls (`sigaction`, `sigprocmask`, `setitimer`).
    unsafe {
        cmd.pre_exec(move || arm_alarm(&timer));
    }
    // Wakes the capture thread once the shell has been reaped.
    let wake = if spec.capture {
        Some(UnixStream::pair()?)
    } else {
        None
    };
    let mut child = cmd.spawn()?;
    let pid = child.id();
    track_group(pid);
    let (reader, mut notify) = match (child.stdout.take(), wake) {
        (Some(out), Some((tx, rx))) => {
            let reader = std::thread::Builder::new()
                .name("mbar-capture".into())
                .spawn(move || capture(out, &rx, MAX_CAPTURE));
            match reader {
                Ok(r) => (Some(r), Some(tx)),
                Err(e) => {
                    // The pipe was moved into the failed closure and is closed by now.
                    log::warn!("cannot start capture thread: {e}");
                    (None, None)
                }
            }
        }
        _ => (None, None),
    };
    std::thread::Builder::new()
        .name("mbar-reaper".into())
        .spawn(move || {
            let _ = child.wait();
            forget_group_if_empty(pid);
            if let Some(tx) = notify.as_mut() {
                let _ = tx.write_all(&[1]);
            }
            let output = reader.map(|r| r.join().unwrap_or_default());
            on_exit(pid, output);
        })?;
    Ok(pid)
}

fn itimer(timeout: Duration) -> libc::itimerval {
    // A zero `it_value` would disarm the timer: fire after 1 µs instead.
    let timeout = timeout.max(Duration::from_micros(1));
    libc::itimerval {
        it_interval: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        it_value: libc::timeval {
            tv_sec: timeout.as_secs().min(i32::MAX as u64) as libc::time_t,
            tv_usec: timeout.subsec_micros() as libc::suseconds_t,
        },
    }
}

/// Runs in the child between `fork` and `exec`: the `alarm(60)` of SketchyBar's `fork_exec`
/// (with sub-second precision for tests). `SIGALRM` gets its default action and is unblocked
/// so the timer terminates the shell unless the script itself traps or ignores it.
fn arm_alarm(timer: &libc::itimerval) -> std::io::Result<()> {
    // SAFETY: plain system calls on valid, initialised arguments.
    unsafe {
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGALRM);
        libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        if libc::setitimer(libc::ITIMER_REAL, timer, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Reads `out` until EOF, until `limit` bytes are kept, or until `wake` becomes readable
/// (the shell was reaped); in the last case the bytes still buffered in the pipe are drained
/// first. Blocks in `poll` (no timeouts) between reads.
fn capture(mut out: ChildStdout, wake: &UnixStream, limit: usize) -> String {
    let fd = out.as_raw_fd();
    // SAFETY: `fcntl` on a pipe end we own.
    let nonblocking = unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
    };
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut exited = false;
    'outer: loop {
        // Drain what is available right now.
        loop {
            let want = chunk.len().min(limit - buf.len());
            match out.read(&mut chunk[..want]) {
                Ok(0) => break 'outer,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() >= limit {
                        log::warn!("exec output truncated to {limit} bytes");
                        break 'outer;
                    }
                    if !nonblocking {
                        // A blocking read would never notice the exit: read until EOF.
                        continue;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break 'outer,
            }
        }
        if exited {
            break;
        }
        let mut fds = [
            libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: `fds` is a valid array of two pollfds.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if r < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if fds[1].revents != 0 {
            exited = true;
        }
    }
    drop(out);
    match String::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Quotes `s` for `sh` (single quotes).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    fn base() -> Vec<(OsString, OsString)> {
        vec![
            ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
            ("BASE".into(), "1".into()),
        ]
    }

    #[test]
    fn captures_output_with_clean_env() {
        let (tx, rx) = mpsc::channel();
        std::env::set_var("MBAR_TEST_LEAK", "leaked");
        spawn(
            Spawn {
                command: "printf '%s|%s|%s|%s' \"$BASE\" \"$X\" \"$MBAR_TEST_LEAK\" \"$(pwd)\""
                    .into(),
                env: vec![("X".into(), "y".into())],
                cwd: Some("/".into()),
                capture: true,
            },
            &base(),
            SCRIPT_TIMEOUT,
            move |pid, out| tx.send((pid, out)).unwrap(),
        )
        .unwrap();
        let (_, out) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(out.as_deref(), Some("1|y||/"));
    }

    #[test]
    fn timeout_kills() {
        let (tx, rx) = mpsc::channel();
        let start = Instant::now();
        spawn(
            Spawn {
                command: "sleep 30".into(),
                env: vec![],
                cwd: None,
                capture: false,
            },
            &base(),
            Duration::from_millis(200),
            move |_, _| tx.send(()).unwrap(),
        )
        .unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    /// Runs `command` with captured stdout; returns the output and the time to `on_exit`.
    fn run_captured(command: &str, timeout: Duration) -> (String, Duration) {
        let (tx, rx) = mpsc::channel();
        let start = Instant::now();
        spawn(
            Spawn {
                command: command.into(),
                env: vec![],
                cwd: None,
                capture: true,
            },
            &base(),
            timeout,
            move |_, out| tx.send(out).unwrap(),
        )
        .unwrap();
        let out = rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|e| panic!("{command}: no callback: {e}"));
        (out.expect("captured"), start.elapsed())
    }

    /// RT-5: like `alarm(60)` in SketchyBar, a script that ignores `SIGALRM` runs to the end;
    /// there is no `SIGKILL` escalation.
    #[test]
    fn review_rt5_ignored_alarm_is_not_escalated_to_sigkill() {
        // Longer than the old 5 s SIGKILL grace after the timeout.
        let (out, took) = run_captured(
            "trap '' ALRM; sleep 6; echo done",
            Duration::from_millis(100),
        );
        assert_eq!(out, "done\n");
        assert!(took >= Duration::from_secs(6), "{took:?}");
        // Same for a handler: it runs, the script continues.
        let (out, _) = run_captured(
            "trap 'echo alarm' ALRM; sleep 1; echo done",
            Duration::from_millis(100),
        );
        assert!(out.ends_with("done\n"), "{out:?}");
    }

    /// The default action still terminates the shell at the timeout.
    #[test]
    fn review_rt5_default_alarm_terminates_at_timeout() {
        let (out, took) =
            run_captured("echo start; sleep 30; echo end", Duration::from_millis(200));
        assert_eq!(out, "start\n");
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    /// R6 / F5: captured output is capped and the writer is cut off (no 60 s of `cat`).
    #[test]
    fn review_r6_capture_is_capped() {
        let (out, took) = run_captured("head -c 100000000 /dev/zero", SCRIPT_TIMEOUT);
        assert_eq!(out.len(), MAX_CAPTURE);
        assert!(took < Duration::from_secs(10), "{took:?}");
    }

    /// Output larger than the pipe buffer below the cap arrives complete.
    #[test]
    fn review_r6_large_output_below_cap_is_complete() {
        let (out, _) = run_captured(
            "head -c 300000 /dev/zero | tr '\\0' a; echo end",
            SCRIPT_TIMEOUT,
        );
        assert_eq!(out.len(), 300_004);
        assert!(out.ends_with("aend\n"));
    }

    /// Output written just before the shell exits is never lost to the exit race.
    #[test]
    fn review_f5_output_before_exit_is_drained() {
        for i in 0..30 {
            let (out, _) = run_captured(&format!("printf {i}"), SCRIPT_TIMEOUT);
            assert_eq!(out, i.to_string());
        }
    }

    /// F5: a background process holding the pipe does not delay the callback.
    #[test]
    fn review_f5_background_child_does_not_hold_callback() {
        let (out, took) = run_captured("sleep 5 & echo done", SCRIPT_TIMEOUT);
        assert_eq!(out, "done\n");
        assert!(took < Duration::from_secs(3), "{took:?}");
    }

    /// R6 / F5: a background writer (`yes &`) neither blocks the callback nor grows the
    /// capture; it is cut off by the closed pipe.
    #[test]
    fn review_r6_background_writer_is_cut_off() {
        let dir = std::env::temp_dir().join(format!("mbar-review-r6-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("pid");
        let (out, took) = run_captured(
            &format!(
                "yes mbar-review-r6 & echo $! > {}",
                shell_quote(&pidfile.to_string_lossy())
            ),
            SCRIPT_TIMEOUT,
        );
        assert!(took < Duration::from_secs(5), "{took:?}");
        assert!(out.len() <= MAX_CAPTURE);
        let pid: libc::pid_t = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let alive = || {
            let o = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            let stat = String::from_utf8_lossy(&o.stdout).trim().to_string();
            !stat.is_empty() && !stat.starts_with('Z')
        };
        let start = Instant::now();
        while alive() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let still = alive();
        // SAFETY: cleanup of the test's own stray process.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        assert!(!still, "background writer still running");
    }

    /// PERF-10: the exit is noticed by a blocking wait, not by 50 ms polls.
    #[test]
    fn review_perf10_exit_reported_without_poll_latency() {
        // The old 1 → 50 ms polling added up to 50 ms on top of the command's own runtime.
        // Process start-up cost varies a lot between machines (macOS CI runners need
        // 30–50 ms for `env sh -c`), so compare against a baseline: the same command
        // started and waited for directly with `std::process`.
        let cmd = "sleep 0.07";
        let baseline = (0..5)
            .map(|_| {
                let t = Instant::now();
                Command::new("/usr/bin/env")
                    .args(["sh", "-c", cmd])
                    .status()
                    .unwrap();
                t.elapsed()
            })
            .min()
            .unwrap();
        let best = (0..5)
            .map(|_| run_captured(cmd, SCRIPT_TIMEOUT).1)
            .min()
            .unwrap();
        assert!(
            best < baseline + Duration::from_millis(25),
            "reported after {best:?}, direct wait took {baseline:?}"
        );
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
    }
}
