//! Script spawning (`docs/spec/events.md` §4.5, `docs/DEVIATIONS.md` D1/D16).
//!
//! Each run is `/usr/bin/env sh -c <script>` (a leading `~` replaced by `$HOME`) with a
//! **fresh** environment: the daemon's startup environment + the given variables (D1), the
//! config directory as working directory, stdin `/dev/null`, stderr inherited and stdout
//! inherited or captured (Lua `mbar.exec`). Every script runs in its own process group; a
//! watchdog kills the whole group (`SIGKILL`) after the timeout (60 s by default, like
//! SketchyBar's `alarm(60)`). A waiter thread per child reaps it and posts
//! [`SysEvent::ScriptFinished`].
//!
//! Requirement: the process must **not** set `SIGCHLD` to `SIG_IGN` (SketchyBar does);
//! children are reaped explicitly here.

use super::{Sink, SysEvent};
use std::ffi::OsString;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// SketchyBar's `FORK_TIMEOUT`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Captured output is truncated beyond this many bytes.
pub const MAX_CAPTURE: usize = 16 * 1024 * 1024;

/// Per-run options.
#[derive(Debug, Clone)]
pub struct SpawnOptions {
    /// Caller tag echoed in [`SysEvent::ScriptFinished`].
    pub id: u64,
    /// Capture stdout (otherwise inherited from the daemon).
    pub capture_stdout: bool,
    /// Kill the process group after this long.
    pub timeout: Duration,
    /// Post [`SysEvent::ScriptFinished`] when the script ends.
    pub notify: bool,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        SpawnOptions {
            id: 0,
            capture_stdout: false,
            timeout: DEFAULT_TIMEOUT,
            notify: true,
        }
    }
}

/// `resolve_path`: a leading `~` becomes `home`.
pub fn resolve_tilde(script: &str, home: Option<&str>) -> String {
    match (script.strip_prefix('~'), home) {
        (Some(rest), Some(h)) => format!("{h}{rest}"),
        _ => script.to_string(),
    }
}

/// Fresh environment: `base` with `vars` applied in order (later keys replace earlier
/// ones, keeping the replacing key's position at the end like `env_vars_set`).
pub fn build_env(base: &[(OsString, OsString)], vars: &[(String, String)]) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = base.to_vec();
    for (k, v) in vars {
        let k = OsString::from(k);
        env.retain(|(ek, _)| *ek != k);
        env.push((k, OsString::from(v)));
    }
    env
}

struct WatchEntry {
    pid: u32,
    deadline: Instant,
    timed_out: Arc<AtomicBool>,
}

struct Watchdog {
    entries: Mutex<Vec<WatchEntry>>,
    cv: Condvar,
}

fn watchdog() -> &'static Arc<Watchdog> {
    static W: OnceLock<Arc<Watchdog>> = OnceLock::new();
    W.get_or_init(|| {
        let w = Arc::new(Watchdog {
            entries: Mutex::new(Vec::new()),
            cv: Condvar::new(),
        });
        let w2 = w.clone();
        let _ = std::thread::Builder::new()
            .name("mbar-script-watchdog".into())
            .stack_size(64 * 1024)
            .spawn(move || watchdog_loop(&w2));
        w
    })
}

fn kill_group(pid: u32) {
    // SAFETY: plain syscall; the group id equals the child's pid (process_group(0)), and
    // the child is not reaped yet while its entry exists, so the id cannot be reused.
    unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
}

fn watchdog_loop(w: &Watchdog) {
    let mut entries = w.entries.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let now = Instant::now();
        entries.retain(|e| {
            if e.deadline <= now {
                e.timed_out.store(true, Ordering::SeqCst);
                kill_group(e.pid);
                false
            } else {
                true
            }
        });
        let next = entries.iter().map(|e| e.deadline).min();
        entries = match next {
            Some(t) => w
                .cv
                .wait_timeout(entries, t.saturating_duration_since(Instant::now()))
                .map(|r| r.0)
                .unwrap_or_else(|e| e.into_inner().0),
            None => w.cv.wait(entries).unwrap_or_else(|e| e.into_inner()),
        };
    }
}

/// Waits for `pid` to exit **without reaping it** (`waitid(WNOWAIT)`).
fn wait_exit_noreap(pid: u32) {
    loop {
        // SAFETY: zeroed siginfo is a valid out buffer.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: valid id type/pid, out buffer and flags.
        let r = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if r == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return;
        }
    }
}

/// Spawns scripts. Cheap to clone.
#[derive(Clone)]
pub struct ScriptRunner {
    sink: Sink,
    cwd: Option<PathBuf>,
    base_env: Arc<Vec<(OsString, OsString)>>,
    running: Arc<AtomicUsize>,
    pids: Arc<Mutex<Vec<u32>>>,
}

impl ScriptRunner {
    /// Uses the current process environment as the base (call at daemon startup, before
    /// anything mutates the environment).
    pub fn new(sink: Sink, cwd: Option<PathBuf>) -> ScriptRunner {
        Self::with_env(sink, cwd, std::env::vars_os().collect())
    }

    /// Explicit base environment (e.g. startup env + `BAR_NAME`, `CONFIG_DIR`).
    pub fn with_env(sink: Sink, cwd: Option<PathBuf>, base_env: Vec<(OsString, OsString)>) -> ScriptRunner {
        ScriptRunner {
            sink,
            cwd,
            base_env: Arc::new(base_env),
            running: Arc::new(AtomicUsize::new(0)),
            pids: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Changes the working directory for later runs (config reload with a new path).
    pub fn set_cwd(&mut self, cwd: Option<PathBuf>) {
        self.cwd = cwd;
    }

    /// Replaces/sets one base variable (e.g. `CONFIG_DIR` after `--reload <path>`).
    pub fn set_base_var(&mut self, key: &str, value: &str) {
        let mut env = (*self.base_env).clone();
        env.retain(|(k, _)| k != key);
        env.push((key.into(), value.into()));
        self.base_env = Arc::new(env);
    }

    /// Number of scripts currently running.
    pub fn running(&self) -> usize {
        self.running.load(Ordering::SeqCst)
    }

    /// Spawns `script` with `vars`; returns the child pid.
    pub fn spawn(&self, script: &str, vars: &[(String, String)], opts: SpawnOptions) -> io::Result<u32> {
        let home = std::env::var("HOME").ok();
        let script = resolve_tilde(script, home.as_deref());
        let mut cmd = Command::new("/usr/bin/env");
        cmd.arg("sh").arg("-c").arg(&script);
        cmd.env_clear();
        cmd.envs(build_env(&self.base_env, vars));
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(if opts.capture_stdout { Stdio::piped() } else { Stdio::inherit() });
        cmd.stderr(Stdio::inherit());
        cmd.process_group(0);
        let started = Instant::now();
        let mut child = cmd.spawn()?;
        let pid = child.id();

        let timed_out = Arc::new(AtomicBool::new(false));
        let wd = watchdog();
        {
            let mut e = wd.entries.lock().unwrap_or_else(|e| e.into_inner());
            e.push(WatchEntry {
                pid,
                deadline: started + opts.timeout,
                timed_out: timed_out.clone(),
            });
            wd.cv.notify_all();
        }
        self.running.fetch_add(1, Ordering::SeqCst);
        self.pids.lock().unwrap_or_else(|e| e.into_inner()).push(pid);

        let sink = self.sink.clone();
        let running = self.running.clone();
        let pids = self.pids.clone();
        let stdout = child.stdout.take();
        let spawn_result = std::thread::Builder::new()
            .name("mbar-script".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let output = stdout.map(|mut out| {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        match out.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => {
                                if buf.len() < MAX_CAPTURE {
                                    let take = n.min(MAX_CAPTURE - buf.len());
                                    buf.extend_from_slice(&chunk[..take]);
                                }
                            }
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                            Err(_) => break,
                        }
                    }
                    String::from_utf8_lossy(&buf).into_owned()
                });
                // Leave the watchdog before reaping so the group id cannot be reused while
                // it may still be killed.
                wait_exit_noreap(pid);
                {
                    let wd = watchdog();
                    wd.entries.lock().unwrap_or_else(|e| e.into_inner()).retain(|e| e.pid != pid);
                }
                let status = child.wait().ok();
                running.fetch_sub(1, Ordering::SeqCst);
                pids.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| *p != pid);
                if opts.notify {
                    sink(SysEvent::ScriptFinished {
                        id: opts.id,
                        pid,
                        status: status.and_then(|s| s.code()),
                        output,
                        duration: started.elapsed(),
                        timed_out: timed_out.load(Ordering::SeqCst),
                    });
                }
            });
        if let Err(e) = spawn_result {
            log::warn!("script reaper thread failed: {e}");
        }
        Ok(pid)
    }

    /// Kills every running script's process group (daemon exit).
    pub fn kill_all(&self) {
        let pids = self.pids.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for pid in pids {
            kill_group(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde() {
        assert_eq!(resolve_tilde("~/plugins/x.sh", Some("/Users/a")), "/Users/a/plugins/x.sh");
        assert_eq!(resolve_tilde("echo ~", Some("/Users/a")), "echo ~");
        assert_eq!(resolve_tilde("~/x", None), "~/x");
    }

    #[test]
    fn env_building() {
        let base = vec![("PATH".into(), "/bin".into()), ("INFO".into(), "old".into())];
        let env = build_env(&base, &[("INFO".into(), "new".into()), ("NAME".into(), "clock".into())]);
        assert_eq!(
            env,
            vec![
                (OsString::from("PATH"), OsString::from("/bin")),
                (OsString::from("INFO"), OsString::from("new")),
                (OsString::from("NAME"), OsString::from("clock")),
            ]
        );
    }
}
