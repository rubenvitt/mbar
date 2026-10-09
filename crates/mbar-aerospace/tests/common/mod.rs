//! A fake AeroSpace for the transport tests: a Unix socket server speaking the socket
//! protocol (handshake, one-shot answers, `subscribe` streams) and fake `aerospace`
//! CLI scripts. The framing is written independently of the crate's own helpers.
#![allow(dead_code)]

use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mbar_aerospace::Config;

pub const SERVER_VERSION: &str = "0.0.0-Test 1234abcd";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A short, unique path under /tmp (macOS limits `sun_path` to 104 bytes).
pub fn temp_path(ext: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    PathBuf::from(format!("/tmp/mbas-{}-{n}{ext}", std::process::id()))
}

/// A config for `socket` with small timings for fast tests.
pub fn config(socket: &Path, cli: Option<PathBuf>) -> Config {
    let mut c = Config::new(socket.to_path_buf(), cli);
    c.initial_backoff = Duration::from_millis(20);
    c.max_backoff = Duration::from_millis(100);
    c.handshake_timeout = Duration::from_millis(300);
    c.answer_timeout = Duration::from_secs(5);
    c
}

/// How the fake server answers the handshake.
#[derive(Clone, Copy)]
pub enum Handshake {
    /// Answers with this version (1 = speaks the protocol).
    Version(u32),
    /// Closes the connection after reading the client's version.
    Close,
    /// Never answers.
    Silent,
}

pub type AnswerFn = Arc<dyn Fn(&[String]) -> (i32, String, String) + Send + Sync>;

#[derive(Clone)]
pub struct Spec {
    pub handshake: Handshake,
    /// One-shot command answers: (exit code, stdout, stderr).
    pub answer: AnswerFn,
    /// Frames sent right after a `subscribe` request (AeroSpace's initial state).
    pub initial_events: Vec<String>,
    /// Answer `subscribe` with this answer frame instead of streaming.
    pub reject_subscribe: Option<String>,
    /// Never answer one-shot requests.
    pub hang_requests: bool,
}

impl Default for Spec {
    fn default() -> Spec {
        Spec {
            handshake: Handshake::Version(1),
            answer: Arc::new(|args: &[String]| {
                (0, format!("ran {}", args.join(" ")), String::new())
            }),
            initial_events: Vec::new(),
            reject_subscribe: None,
            hang_requests: false,
        }
    }
}

pub struct FakeServer {
    pub path: PathBuf,
    stop: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<UnixStream>>>,
    subscribers: Arc<Mutex<Vec<UnixStream>>>,
    /// Raw JSON of every request received.
    pub requests: Arc<Mutex<Vec<String>>>,
    subscribed: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    accept: Option<JoinHandle<()>>,
}

impl FakeServer {
    pub fn start(path: &Path, spec: Spec) -> FakeServer {
        let _ = fs::remove_file(path);
        let listener = UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let conns: Arc<Mutex<Vec<UnixStream>>> = Arc::default();
        let subscribers: Arc<Mutex<Vec<UnixStream>>> = Arc::default();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let subscribed: Arc<Mutex<Option<mpsc::Sender<()>>>> = Arc::default();
        let accept = {
            let (stop, conns, subscribers, requests, subscribed) = (
                stop.clone(),
                conns.clone(),
                subscribers.clone(),
                requests.clone(),
                subscribed.clone(),
            );
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            conns.lock().unwrap().push(stream.try_clone().unwrap());
                            let ctx = Conn {
                                spec: spec.clone(),
                                subscribers: subscribers.clone(),
                                requests: requests.clone(),
                                subscribed: subscribed.clone(),
                            };
                            thread::spawn(move || ctx.serve(stream));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
        };
        FakeServer {
            path: path.to_path_buf(),
            stop,
            conns,
            subscribers,
            requests,
            subscribed,
            accept: Some(accept),
        }
    }

    /// A channel that receives `()` whenever a client subscribes.
    pub fn on_subscribe(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        *self.subscribed.lock().unwrap() = Some(tx);
        rx
    }

    /// Sends an event frame to every subscriber.
    pub fn push(&self, event: &str) {
        for s in self.subscribers.lock().unwrap().iter_mut() {
            let _ = write_frame(s, event.as_bytes());
        }
    }

    /// Stops listening, closes every connection and removes the socket file (AeroSpace
    /// quitting).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.accept.take() {
            t.join().unwrap();
        }
        for c in self.conns.lock().unwrap().drain(..) {
            let _ = c.shutdown(Shutdown::Both);
        }
        self.subscribers.lock().unwrap().clear();
        let _ = fs::remove_file(&self.path);
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Conn {
    spec: Spec,
    subscribers: Arc<Mutex<Vec<UnixStream>>>,
    requests: Arc<Mutex<Vec<String>>>,
    subscribed: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

impl Conn {
    fn serve(self, mut s: UnixStream) {
        let mut v = [0u8; 4];
        if s.read_exact(&mut v).is_err() {
            return;
        }
        match self.spec.handshake {
            Handshake::Version(n) => {
                if s.write_all(&n.to_le_bytes()).is_err() || n != 1 {
                    return;
                }
            }
            Handshake::Close => return,
            Handshake::Silent => {
                let _ = s.read_to_end(&mut Vec::new());
                return;
            }
        }
        assert_eq!(u32::from_le_bytes(v), 1, "client protocol version");
        loop {
            let Some(req) = read_frame(&mut s) else {
                return;
            };
            let text = String::from_utf8(req).unwrap();
            self.requests.lock().unwrap().push(text.clone());
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            let args: Vec<String> = json["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap().to_string())
                .collect();
            if args.first().map(String::as_str) == Some("subscribe") {
                if let Some(answer) = &self.spec.reject_subscribe {
                    let _ = write_frame(&mut s, answer.as_bytes());
                    continue;
                }
                for ev in &self.spec.initial_events {
                    if write_frame(&mut s, ev.as_bytes()).is_err() {
                        return;
                    }
                }
                self.subscribers
                    .lock()
                    .unwrap()
                    .push(s.try_clone().unwrap());
                if let Some(tx) = self.subscribed.lock().unwrap().as_ref() {
                    let _ = tx.send(());
                }
                // Like AeroSpace: keep the connection until the client goes away.
                let _ = s.read_to_end(&mut Vec::new());
                return;
            }
            if self.spec.hang_requests {
                let _ = s.read_to_end(&mut Vec::new());
                return;
            }
            let (code, stdout, stderr) = (self.spec.answer)(&args);
            let answer = serde_json::json!({
                "exitCode": code,
                "stdout": stdout,
                "stderr": stderr,
                "serverVersionAndHash": SERVER_VERSION,
            });
            if write_frame(&mut s, answer.to_string().as_bytes()).is_err() {
                return;
            }
        }
    }
}

fn write_frame(s: &mut UnixStream, payload: &[u8]) -> std::io::Result<()> {
    let mut buf = (payload.len() as u32).to_le_bytes().to_vec();
    buf.extend_from_slice(payload);
    s.write_all(&buf)
}

fn read_frame(s: &mut UnixStream) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).ok()?;
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    s.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// A fake `aerospace` script in its own temp directory (removed on drop).
pub struct FakeCli {
    dir: PathBuf,
    path: PathBuf,
}

impl FakeCli {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

impl Drop for FakeCli {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Writes an executable fake `aerospace` script with `body` (after `#!/bin/sh`).
pub fn fake_cli(body: &str) -> FakeCli {
    let dir = temp_path("");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("aerospace");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    FakeCli { dir, path }
}
