//! Child processes: item scripts, click scripts, the shell config and Lua `mbar.exec`.
//!
//! `fork_exec` semantics (`cli.md` §10.2, §12.1, §14.8, D1):
//! `/usr/bin/env sh -c <command>` with a **fresh** environment (the daemon's startup
//! environment + `BAR_NAME`/`CONFIG_DIR` + the vars of this run), cwd = config directory,
//! stdin `/dev/null`, stdout/stderr inherited (they end up in the daemon log) unless the
//! output is captured. After 60 s the shell gets `SIGALRM` (the default action terminates
//! it, like SketchyBar's `alarm(60)`), and `SIGKILL` 5 s later if it is still alive. A
//! reaper thread waits for the child and reports its exit.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// `alarm(60)` of every spawned shell.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);
/// Grace period after `SIGALRM` before `SIGKILL`.
const KILL_GRACE: Duration = Duration::from_secs(5);

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
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let reader = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    std::thread::Builder::new()
        .name("mbar-reaper".into())
        .spawn(move || {
            reap(&mut child, timeout);
            let output = reader.and_then(|r| r.join().ok());
            on_exit(pid, output);
        })?;
    Ok(pid)
}

/// Waits for `child`, enforcing the timeout. Polls with a growing interval (1 ms → 50 ms):
/// most scripts finish within a few milliseconds. The child is never signalled after it
/// has been reaped, so its pid cannot have been reused.
fn reap(child: &mut Child, timeout: Duration) {
    let start = Instant::now();
    let mut interval = Duration::from_millis(1);
    let mut alarmed = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {}
        }
        let elapsed = start.elapsed();
        if !alarmed && elapsed >= timeout {
            // SAFETY: plain kill(2) on our own, not yet reaped child.
            unsafe {
                libc::kill(child.id() as libc::pid_t, libc::SIGALRM);
            }
            alarmed = true;
        } else if alarmed && elapsed >= timeout + KILL_GRACE {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(interval);
        interval = (interval * 2).min(Duration::from_millis(50));
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

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
    }
}
