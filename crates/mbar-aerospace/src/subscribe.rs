//! The event subscription (design doc "Transport"): one background thread that keeps a
//! `subscribe --all` stream open (socket, else an `aerospace subscribe --all` child),
//! reconnects with backoff, and reports events and status changes through callbacks.
//!
//! Each attempt: connect + handshake for a one-shot request with empty args (the server
//! answers it with an error, but the answer carries `serverVersionAndHash`, like
//! `aerospace --version` does), then a second connection: handshake, the `subscribe`
//! request, then `ServerEvent` frames until EOF. AeroSpace sends the current workspace,
//! focus, monitor and mode right after `subscribe` (`subscriptions.swift`), so the
//! consumer is up to date without a separate query.

use std::io::{self, BufRead, BufReader, Read};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mbar_core::aerospace::{AerospaceEvent, AerospaceStatus, AerospaceTransport};

use crate::protocol::{self, MAX_FRAME_LEN};
use crate::{cli, Config, Failure};

/// How often a blocked reader wakes up to check the stop flag (the socket is also shut
/// down on drop, which wakes it immediately).
const POLL: Duration = Duration::from_millis(200);

/// The arguments of the subscribe request.
const SUBSCRIBE_ARGS: [&str; 2] = ["subscribe", "--all"];

type EventFn = Box<dyn Fn(AerospaceEvent) + Send>;
type StatusFn = Box<dyn Fn(AerospaceStatus) + Send>;

/// A running subscription. Dropping it stops the background thread (and kills the CLI
/// child); the callbacks are not called after the drop returns.
///
/// The callbacks run on the subscription thread. Dropping the subscription from inside
/// a callback is allowed (the thread then finishes on its own instead of being joined).
#[must_use = "dropping the Subscription stops it"]
pub struct Subscription {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// Background subscription with reconnect, configured from the environment
/// ([`Config::from_env`]); dropping it stops the thread and the CLI child.
pub fn subscribe(
    on_event: impl Fn(AerospaceEvent) + Send + 'static,
    on_status: impl Fn(AerospaceStatus) + Send + 'static,
) -> Subscription {
    subscribe_with(Config::from_env(), on_event, on_status)
}

/// [`subscribe`] with an explicit [`Config`].
pub fn subscribe_with(
    config: Config,
    on_event: impl Fn(AerospaceEvent) + Send + 'static,
    on_status: impl Fn(AerospaceStatus) + Send + 'static,
) -> Subscription {
    let shared = Arc::new(Shared::default());
    let worker = Worker {
        config,
        shared: shared.clone(),
        on_event: Box::new(on_event),
        on_status: Box::new(on_status),
        last_status: AerospaceStatus::default(),
    };
    let thread = thread::Builder::new()
        .name("mbar-aerospace".into())
        .spawn(move || worker.run());
    let thread = match thread {
        Ok(t) => Some(t),
        Err(e) => {
            log::error!("aerospace: cannot start the subscription thread: {e}");
            None
        }
    };
    Subscription { shared, thread }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(t) = self.thread.take() {
            if t.thread().id() != thread::current().id() {
                let _ = t.join();
            }
        }
    }
}

/// State shared between the [`Subscription`] handle and its thread.
#[derive(Default)]
struct Shared {
    stopped: AtomicBool,
    /// Guards the stop/sleep handshake of `wakeup`.
    sleep_lock: Mutex<()>,
    wakeup: Condvar,
    /// A clone of the socket the thread is blocked on (shut down on stop).
    socket: Mutex<Option<UnixStream>>,
    /// The CLI child (killed and reaped on stop).
    child: Mutex<Option<Child>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Sets the stop flag, wakes the sleeping thread, and unblocks its reads by shutting
    /// down the socket / killing the child.
    fn stop(&self) {
        {
            let _g = lock(&self.sleep_lock);
            self.stopped.store(true, Ordering::SeqCst);
            self.wakeup.notify_all();
        }
        if let Some(s) = lock(&self.socket).take() {
            let _ = s.shutdown(Shutdown::Both);
        }
        if let Some(mut c) = lock(&self.child).take() {
            cli::kill_and_reap(&mut c);
        }
    }

    /// Sleeps `d` unless stopped first; false when stopped.
    fn sleep(&self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        let mut g = lock(&self.sleep_lock);
        while !self.stopped() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            g = self
                .wakeup
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        false
    }

    /// Makes `s` the socket that [`Shared::stop`] shuts down. The flag is checked after
    /// storing, so a stop racing with this never leaves the thread blocked.
    fn register_socket(&self, s: &UnixStream) {
        *lock(&self.socket) = s.try_clone().ok();
        if self.stopped() {
            if let Some(s) = lock(&self.socket).take() {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }

    fn clear_socket(&self) {
        lock(&self.socket).take();
    }

    /// Hands `child` to [`Shared::stop`]; false (child killed) when already stopped.
    fn register_child(&self, child: Child) -> bool {
        *lock(&self.child) = Some(child);
        if self.stopped() {
            if let Some(mut c) = lock(&self.child).take() {
                cli::kill_and_reap(&mut c);
            }
            return false;
        }
        true
    }
}

/// How one connection attempt ended.
struct Outcome {
    /// The attempt reached the streaming state (resets the backoff).
    connected: bool,
    /// Why it ended (shown in the disconnected status).
    error: String,
}

impl Outcome {
    fn failed(error: impl Into<String>) -> Outcome {
        Outcome {
            connected: false,
            error: error.into(),
        }
    }
}

struct Worker {
    config: Config,
    shared: Arc<Shared>,
    on_event: EventFn,
    on_status: StatusFn,
    last_status: AerospaceStatus,
}

impl Worker {
    fn run(mut self) {
        let mut backoff = self.config.initial_backoff;
        while !self.shared.stopped() {
            let outcome = self.attempt();
            if self.shared.stopped() {
                break;
            }
            if outcome.connected {
                backoff = self.config.initial_backoff;
            }
            log::debug!(
                "aerospace: disconnected ({}); retrying in {backoff:?}",
                outcome.error
            );
            self.report(AerospaceStatus {
                connected: false,
                transport: AerospaceTransport::None,
                server_version: None,
                error: Some(outcome.error),
            });
            if !self.shared.sleep(backoff) {
                break;
            }
            backoff = (backoff * 2).min(self.config.max_backoff.max(self.config.initial_backoff));
        }
    }

    /// Reports `status` unless it equals the last one.
    fn report(&mut self, status: AerospaceStatus) {
        if status != self.last_status {
            log::info!(
                "aerospace: {} via {}{}",
                if status.connected {
                    "connected"
                } else {
                    "disconnected"
                },
                status.transport.as_str(),
                status
                    .error
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default()
            );
            self.last_status = status.clone();
            (self.on_status)(status);
        }
    }

    fn attempt(&mut self) -> Outcome {
        let shared = self.shared.clone();
        let register = |s: &UnixStream| shared.register_socket(s);

        // Server version (best effort) on its own connection; this also tells whether
        // the server speaks the socket protocol at all.
        let server_version = match crate::connect(&self.config, &register) {
            Ok(mut s) => {
                let version = crate::exchange(&mut s, &[], self.config.handshake_timeout)
                    .ok()
                    .and_then(|a| a.server_version);
                self.shared.clear_socket();
                version
            }
            Err(Failure::Handshake(msg)) => {
                self.shared.clear_socket();
                return self.cli_session(&msg);
            }
            Err(Failure::Unreachable(e)) | Err(Failure::Fatal(e)) => {
                self.shared.clear_socket();
                return Outcome::failed(e.to_string());
            }
        };
        if self.shared.stopped() {
            return Outcome::failed("stopped");
        }

        let outcome = match crate::connect(&self.config, &register) {
            Ok(stream) => self.socket_session(stream, server_version),
            Err(Failure::Unreachable(e)) | Err(Failure::Fatal(e)) => Outcome::failed(e.to_string()),
            Err(Failure::Handshake(msg)) => {
                self.shared.clear_socket();
                return self.cli_session(&msg);
            }
        };
        self.shared.clear_socket();
        outcome
    }

    fn socket_session(
        &mut self,
        mut stream: UnixStream,
        server_version: Option<String>,
    ) -> Outcome {
        let setup = stream
            .set_write_timeout(Some(crate::nonzero(self.config.answer_timeout)))
            .and_then(|()| stream.set_read_timeout(Some(POLL)));
        if let Err(e) = setup {
            return Outcome::failed(e.to_string());
        }
        let args: Vec<String> = SUBSCRIBE_ARGS.iter().map(|s| s.to_string()).collect();
        let request = protocol::client_request(&args, "");
        if let Err(e) = protocol::write_frame(&mut stream, request.as_bytes()) {
            return Outcome::failed(format!("sending the subscribe request failed: {e}"));
        }
        self.report(AerospaceStatus {
            connected: true,
            transport: AerospaceTransport::Socket,
            server_version,
            error: None,
        });
        let shared = self.shared.clone();
        let keep_waiting = move || !shared.stopped();
        loop {
            let frame = match protocol::read_frame_with(&mut stream, &keep_waiting) {
                Ok(frame) => frame,
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Outcome {
                        connected: true,
                        error: "AeroSpace closed the connection".into(),
                    }
                }
                Err(e) => {
                    return Outcome {
                        connected: true,
                        error: format!("reading AeroSpace events failed: {e}"),
                    }
                }
            };
            if protocol::is_server_answer(&frame) {
                // `subscribe` was answered like a command: rejected / unknown. Not a
                // successful connection, so the backoff keeps growing.
                let detail = protocol::parse_server_answer(&frame)
                    .map(|a| {
                        let text = if a.stderr.is_empty() {
                            a.stdout
                        } else {
                            a.stderr
                        };
                        format!("exit {}: {}", a.exit_code, text.trim())
                    })
                    .unwrap_or_default();
                return Outcome::failed(format!("AeroSpace rejected `subscribe --all` ({detail})"));
            }
            self.deliver(&frame);
        }
    }

    fn deliver(&self, payload: &[u8]) {
        let Ok(text) = std::str::from_utf8(payload) else {
            log::debug!("aerospace: skipping a non-UTF-8 event");
            return;
        };
        match AerospaceEvent::from_json(text) {
            Some(ev) => (self.on_event)(ev),
            None => log::debug!("aerospace: skipping unknown event {text}"),
        }
    }

    /// The CLI fallback for one attempt: `aerospace subscribe --all`, one event per line.
    fn cli_session(&mut self, reason: &str) -> Outcome {
        log::debug!("aerospace: socket protocol unavailable ({reason}); trying the CLI");
        let Some(cli_path) = self.config.cli.clone() else {
            return Outcome::failed(format!("{reason}; no aerospace CLI found for the fallback"));
        };
        let args: Vec<String> = SUBSCRIBE_ARGS.iter().map(|s| s.to_string()).collect();
        let mut cmd = cli::command(&cli_path, &args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cli::spawn(&mut cmd) {
            Ok(c) => c,
            Err(e) => {
                return Outcome::failed(format!("{reason}; cannot run {}: {e}", cli_path.display()))
            }
        };
        let stdout = child.stdout.take();
        let stderr_rx = child.stderr.take().map(cli::read_pipe);
        if !self.shared.register_child(child) {
            return Outcome::failed("stopped");
        }
        let mut connected = false;
        let mut read_error = None;
        if let Some(stdout) = stdout {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match (&mut reader)
                    .take(MAX_FRAME_LEN as u64 + 1)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => break,
                    Ok(_) if line.len() > MAX_FRAME_LEN => {
                        read_error = Some("an event line exceeds the size limit".to_string());
                        break;
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        read_error = Some(e.to_string());
                        break;
                    }
                }
                if !connected {
                    connected = true;
                    self.report(AerospaceStatus {
                        connected: true,
                        transport: AerospaceTransport::Cli,
                        server_version: None,
                        error: None,
                    });
                }
                if line.iter().any(|b| !b.is_ascii_whitespace()) {
                    self.deliver(&line);
                }
            }
        }
        if self.shared.stopped() {
            return Outcome::failed("stopped");
        }
        // Stdout is closed: the child is exiting. Give it a moment, then make sure.
        let exit = lock(&self.shared.child).take().map(|mut c| {
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                match c.try_wait() {
                    Ok(Some(status)) => break cli::exit_code(status),
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    _ => {
                        cli::kill_and_reap(&mut c);
                        break -1;
                    }
                }
            }
        });
        let stderr = stderr_rx
            .and_then(|rx| rx.recv_timeout(Duration::from_millis(500)).ok())
            .unwrap_or_default();
        let mut error = format!(
            "`{} subscribe --all` {} (exit {})",
            cli_path.display(),
            if connected { "exited" } else { "failed" },
            exit.unwrap_or(-1)
        );
        if let Some(e) = read_error {
            error.push_str(&format!(": {e}"));
        }
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            error.push_str(&format!(": {stderr}"));
        }
        if !connected {
            error = format!("{reason}; {error}");
        }
        Outcome { connected, error }
    }
}
