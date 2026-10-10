//! The `aerospace` CLI fallback (design doc "Transport"): for AeroSpace servers that
//! predate the socket protocol handshake, commands run as `aerospace <args>` and events
//! come from a long-running `aerospace subscribe --all` child.
//!
//! Children run in their own process group so that killing them also kills anything a
//! wrapper script started (a grandchild holding the stdout pipe would otherwise keep
//! readers blocked).

use std::io::{self, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::protocol::MAX_FRAME_LEN;
use crate::Answer;

/// Variables AeroSpace sets for its callbacks. mbar is not such a callback, so the CLI
/// must not inherit them (the socket transport sends `null` for both as well).
const CALLBACK_ENV: [&str; 2] = ["AEROSPACE_WINDOW_ID", "AEROSPACE_WORKSPACE"];

/// A [`Command`] for `cli args…`: stdin closed, callback variables removed, own process
/// group.
pub(crate) fn command(cli: &Path, args: &[String]) -> Command {
    let mut cmd = Command::new(cli);
    cmd.args(args).stdin(Stdio::null()).process_group(0);
    for k in CALLBACK_ENV {
        cmd.env_remove(k);
    }
    cmd
}

/// Spawns `cmd`, retrying a few times on `ETXTBSY` (the binary is being replaced, or —
/// in tests — another thread forked while a freshly written script was still open).
pub(crate) fn spawn(cmd: &mut Command) -> io::Result<Child> {
    let mut attempts = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && attempts < 10 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(10));
            }
            r => return r,
        }
    }
}

/// Kills the child's process group and the child, then reaps it. Must only be called
/// while the child has not been reaped yet (its pid, and so its group id, cannot have
/// been reused).
pub(crate) fn kill_and_reap(child: &mut Child) {
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill(2) has no memory-safety preconditions. The child is our unreaped
        // child and the leader of its own group (`process_group(0)`), so `-pgid` names
        // only that group.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The CLI's exit code: the code, or `128 + signal` like a shell reports it.
pub(crate) fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(-1)
}

/// Reads a pipe to the end on a helper thread; at most [`MAX_FRAME_LEN`] bytes are kept,
/// the rest is drained so the child never blocks on a full pipe.
pub(crate) fn read_pipe(pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("mbar-aerospace-pipe".into())
        .spawn(move || {
            let mut pipe = pipe;
            let mut buf = Vec::new();
            let _ = (&mut pipe).take(MAX_FRAME_LEN as u64).read_to_end(&mut buf);
            let _ = io::copy(&mut pipe, &mut io::sink());
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
        });
    if let Err(e) = spawned {
        log::warn!("aerospace: cannot start a pipe reader thread: {e}");
    }
    rx
}

/// Why a CLI command produced no [`Answer`].
#[derive(Debug)]
pub(crate) enum CliError {
    /// The binary could not be started (missing, not executable, …).
    Spawn(io::Error),
    /// No exit within the timeout; the child was killed.
    Timeout,
}

/// Runs `cli args…` and collects its output like the socket answer: one trailing newline
/// (added by the CLI's `print`/`eprint`) is removed from stdout and stderr.
pub(crate) fn run(cli: &Path, args: &[String], timeout: Duration) -> Result<Answer, CliError> {
    let mut cmd = command(cli, args);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn(&mut cmd).map_err(CliError::Spawn)?;
    let deadline = Instant::now() + timeout;
    let stdout: Option<ChildStdout> = child.stdout.take();
    let stderr: Option<ChildStderr> = child.stderr.take();
    let out_rx = stdout.map(read_pipe);
    let err_rx = stderr.map(read_pipe);
    let mut collect = |rx: Option<mpsc::Receiver<String>>| -> Result<String, CliError> {
        let Some(rx) = rx else {
            return Ok(String::new());
        };
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(s) => Ok(s),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(String::new()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                kill_and_reap(&mut child);
                Err(CliError::Timeout)
            }
        }
    };
    let stdout = collect(out_rx)?;
    let stderr = collect(err_rx)?;
    // Both pipes are closed, so the child is exiting; wait for it within the deadline.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(1)),
            Ok(None) => {
                kill_and_reap(&mut child);
                return Err(CliError::Timeout);
            }
            Err(e) => return Err(CliError::Spawn(e)),
        }
    };
    Ok(Answer {
        exit_code: exit_code(status),
        stdout: strip_newline(stdout),
        stderr: strip_newline(stderr),
        server_version: None,
    })
}

fn strip_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_exactly_one_newline() {
        assert_eq!(strip_newline("a\n\n".into()), "a\n");
        assert_eq!(strip_newline("a".into()), "a");
        assert_eq!(strip_newline(String::new()), "");
    }

    #[test]
    fn runs_a_command() {
        let a = run(
            Path::new("/bin/sh"),
            &["-c".into(), "echo out; echo err >&2; exit 4".into()],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            a,
            Answer {
                exit_code: 4,
                stdout: "out".into(),
                stderr: "err".into(),
                server_version: None
            }
        );
    }

    #[test]
    fn times_out_and_kills_the_group() {
        let start = Instant::now();
        let r = run(
            Path::new("/bin/sh"),
            &["-c".into(), "sleep 30; echo late".into()],
            Duration::from_millis(100),
        );
        assert!(matches!(r, Err(CliError::Timeout)));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn missing_binary_is_a_spawn_error() {
        let r = run(
            Path::new("/nonexistent/aerospace"),
            &[],
            Duration::from_secs(1),
        );
        assert!(matches!(r, Err(CliError::Spawn(_))));
    }
}
