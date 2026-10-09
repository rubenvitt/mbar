//! Blocking IPC client used by the UI. Every function here blocks on a socket, so the
//! GUI only calls them from GPUI's background executor or from the monitor thread,
//! never from the UI thread.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::model::{
    borders_drawing_args, parse_monitor_line, set_args, trigger_args, BarInfo, BordersInfo,
    ItemInfo, MonitorMessage, Snapshot, Stats, Target,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpcError {
    /// No daemon listens for this bar name.
    NotRunning,
    /// The daemon answered with an error message (`[!] ...`).
    Daemon(String),
    /// Transport error or unparsable response.
    Io(String),
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpcError::NotRunning => write!(f, "mbar is not running"),
            IpcError::Daemon(m) => write!(f, "{m}"),
            IpcError::Io(m) => write!(f, "IPC error: {m}"),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<io::Error> for IpcError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => IpcError::NotRunning,
            _ => IpcError::Io(e.to_string()),
        }
    }
}

/// Maps a raw daemon response to `Ok(text)` or `Err(Daemon(message))`.
pub fn classify_response(rsp: String) -> Result<String, IpcError> {
    if mbar_ipc::is_error_response(&rsp) {
        let msg = rsp.trim();
        let msg = msg.strip_prefix("[!]").unwrap_or(msg).trim();
        Err(IpcError::Daemon(msg.to_string()))
    } else {
        Ok(rsp)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaemonStatus {
    Unknown,
    Connected,
    NotRunning,
    Error(String),
}

/// Client for one bar instance (`--bar-name`).
#[derive(Clone, Debug)]
pub struct Client {
    bar_name: String,
}

impl Client {
    pub fn new(bar_name: impl Into<String>) -> Client {
        Client {
            bar_name: bar_name.into(),
        }
    }

    pub fn bar_name(&self) -> &str {
        &self.bar_name
    }

    pub fn send(&self, args: &[String]) -> Result<String, IpcError> {
        let rsp = mbar_ipc::send(&self.bar_name, args)?;
        classify_response(rsp)
    }

    pub fn send_strs(&self, args: &[&str]) -> Result<String, IpcError> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.send(&args)
    }

    pub fn status(&self) -> DaemonStatus {
        match self.send_strs(&["--query", "bar"]) {
            Ok(_) => DaemonStatus::Connected,
            Err(IpcError::NotRunning) => DaemonStatus::NotRunning,
            Err(e) => DaemonStatus::Error(e.to_string()),
        }
    }

    pub fn query_bar(&self) -> Result<BarInfo, IpcError> {
        let rsp = self.send_strs(&["--query", "bar"])?;
        BarInfo::parse(&rsp).map_err(IpcError::Io)
    }

    pub fn query_item(&self, name: &str) -> Result<ItemInfo, IpcError> {
        // `item <name>` also works for items named like a query keyword.
        let rsp = self.send_strs(&["--query", "item", name])?;
        ItemInfo::parse(name, &rsp).map_err(IpcError::Io)
    }

    pub fn query_stats(&self) -> Result<Stats, IpcError> {
        let rsp = self.send_strs(&["--query", "stats"])?;
        Stats::parse(&rsp).map_err(IpcError::Io)
    }

    /// `--query bar` plus `--query item <name>` for every item.
    pub fn fetch_snapshot(&self) -> Result<Snapshot, IpcError> {
        let bar = self.query_bar()?;
        let mut items = BTreeMap::new();
        let mut errors = Vec::new();
        for name in &bar.items {
            match self.query_item(name) {
                Ok(info) => {
                    items.insert(name.clone(), info);
                }
                Err(IpcError::NotRunning) => return Err(IpcError::NotRunning),
                Err(e) => errors.push((name.clone(), e.to_string())),
            }
        }
        Ok(Snapshot { bar, items, errors })
    }

    /// `--set <item> k=v ...` or `--bar k=v ...`. Returns the daemon's (non-error) text.
    pub fn set(&self, target: &Target, pairs: &[(String, String)]) -> Result<String, IpcError> {
        self.send(&set_args(target, pairs))
    }

    pub fn trigger(&self, event: &str, vars: &str) -> Result<String, IpcError> {
        let args = trigger_args(event, vars).map_err(IpcError::Io)?;
        self.send(&args)
    }

    pub fn reload(&self) -> Result<String, IpcError> {
        self.send_strs(&["--reload"])
    }

    /// `--menubar hide|show|toggle`.
    pub fn menubar(&self, mode: &str) -> Result<String, IpcError> {
        self.send_strs(&["--menubar", mode])
    }

    /// `--query borders`: the window-border configuration.
    pub fn query_borders(&self) -> Result<BordersInfo, IpcError> {
        let rsp = self.send_strs(&["--query", "borders"])?;
        BordersInfo::parse(&rsp).map_err(IpcError::Io)
    }

    /// `--borders drawing=on|off`.
    pub fn set_borders_drawing(&self, on: bool) -> Result<String, IpcError> {
        self.send(&borders_drawing_args(on))
    }
}

// ---------------------------------------------------------------------------
// `--monitor` stream
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamMode {
    Unknown,
    /// `u32 LE length` + payload frames (mbar-ipc socket framing).
    Framed,
    /// Plain newline-delimited text.
    Lines,
}

/// Turns the bytes of a `--monitor` connection into text lines. The daemon writes
/// length-prefixed frames (each holding one or more lines); a stream that starts with
/// JSON text instead is read as newline-delimited lines.
#[derive(Debug)]
pub struct StreamDecoder {
    buf: Vec<u8>,
    mode: StreamMode,
}

impl Default for StreamDecoder {
    fn default() -> Self {
        StreamDecoder::new()
    }
}

impl StreamDecoder {
    pub fn new() -> StreamDecoder {
        StreamDecoder {
            buf: Vec::new(),
            mode: StreamMode::Unknown,
        }
    }

    pub fn push(&mut self, data: &[u8]) -> Result<Vec<String>, String> {
        self.buf.extend_from_slice(data);
        if self.mode == StreamMode::Unknown {
            if self.buf.len() < 4 {
                return Ok(Vec::new());
            }
            let len =
                u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
            let textual = matches!(self.buf[0], b'{' | b'[') && self.buf[1] != 0;
            self.mode = if textual || len > mbar_ipc::socket::MAX_FRAME {
                StreamMode::Lines
            } else {
                StreamMode::Framed
            };
        }
        let mut lines = Vec::new();
        match self.mode {
            StreamMode::Framed => loop {
                if self.buf.len() < 4 {
                    break;
                }
                let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]])
                    as usize;
                if len > mbar_ipc::socket::MAX_FRAME {
                    return Err("frame too large".into());
                }
                if self.buf.len() < 4 + len {
                    break;
                }
                let frame: Vec<u8> = self.buf.drain(..4 + len).skip(4).collect();
                let text = String::from_utf8_lossy(&frame);
                lines.extend(
                    text.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string),
                );
            },
            StreamMode::Lines => {
                while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = self.buf.drain(..=pos).collect();
                    let text = String::from_utf8_lossy(&line);
                    let text = text.trim();
                    if !text.is_empty() {
                        lines.push(text.to_string());
                    }
                }
            }
            StreamMode::Unknown => unreachable!(),
        }
        Ok(lines)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MonitorUpdate {
    Connected,
    /// Connection lost or not possible; the thread retries.
    Disconnected(String),
    Message(MonitorMessage),
}

/// Stops the monitor thread when dropped.
pub struct MonitorHandle {
    stop: Arc<AtomicBool>,
    _thread: Option<JoinHandle<()>>,
}

impl MonitorHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

const READ_POLL: Duration = Duration::from_millis(250);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const ERROR_RETRY_DELAY: Duration = Duration::from_secs(10);

fn sleep_unless_stopped(stop: &AtomicBool, total: Duration) {
    let mut left = total;
    while !stop.load(Ordering::Relaxed) && !left.is_zero() {
        let step = left.min(READ_POLL);
        std::thread::sleep(step);
        left -= step;
    }
}

/// Opens `--monitor <what>` (`events`, `stats` or `all`) on a dedicated thread and
/// reconnects while the handle lives. `sink` runs on that thread; returning `false`
/// ends the thread (e.g. when the receiving UI is gone).
pub fn spawn_monitor(
    bar_name: &str,
    what: &str,
    mut sink: impl FnMut(MonitorUpdate) -> bool + Send + 'static,
) -> io::Result<MonitorHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let path = mbar_ipc::socket_path(bar_name);
    let payload = mbar_ipc::encode_args(&["--monitor", what]);
    let thread = std::thread::Builder::new()
        .name("mbar-ui-monitor".into())
        .spawn(move || {
            let stop = stop_thread;
            while !stop.load(Ordering::Relaxed) {
                let delay = match run_monitor_connection(&path, &payload, &stop, &mut sink) {
                    ConnectionEnd::SinkClosed => return,
                    ConnectionEnd::Stopped => return,
                    ConnectionEnd::Lost(msg) => {
                        if !sink(MonitorUpdate::Disconnected(msg)) {
                            return;
                        }
                        RETRY_DELAY
                    }
                    ConnectionEnd::Rejected(msg) => {
                        if !sink(MonitorUpdate::Disconnected(msg)) {
                            return;
                        }
                        ERROR_RETRY_DELAY
                    }
                };
                sleep_unless_stopped(&stop, delay);
            }
        })?;
    Ok(MonitorHandle {
        stop,
        _thread: Some(thread),
    })
}

enum ConnectionEnd {
    Stopped,
    SinkClosed,
    Lost(String),
    /// The daemon answered with an error (e.g. `--monitor` unsupported).
    Rejected(String),
}

fn run_monitor_connection(
    path: &std::path::Path,
    payload: &[u8],
    stop: &AtomicBool,
    sink: &mut impl FnMut(MonitorUpdate) -> bool,
) -> ConnectionEnd {
    let mut stream = match UnixStream::connect(path) {
        Ok(s) => s,
        Err(e) => return ConnectionEnd::Lost(IpcError::from(e).to_string()),
    };
    let _ = stream.set_write_timeout(Some(mbar_ipc::socket::CLIENT_TIMEOUT));
    if let Err(e) = mbar_ipc::socket::write_frame(&mut stream, payload) {
        return ConnectionEnd::Lost(e.to_string());
    }
    if let Err(e) = stream.set_read_timeout(Some(READ_POLL)) {
        return ConnectionEnd::Lost(e.to_string());
    }
    if !sink(MonitorUpdate::Connected) {
        return ConnectionEnd::SinkClosed;
    }
    let mut decoder = StreamDecoder::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        if stop.load(Ordering::Relaxed) {
            return ConnectionEnd::Stopped;
        }
        match stream.read(&mut buf) {
            Ok(0) => return ConnectionEnd::Lost("monitor connection closed".into()),
            Ok(n) => {
                let lines = match decoder.push(&buf[..n]) {
                    Ok(l) => l,
                    Err(e) => return ConnectionEnd::Lost(e),
                };
                for line in lines {
                    if mbar_ipc::is_error_response(&line) {
                        return ConnectionEnd::Rejected(
                            line.trim_start_matches("[!]").trim().to_string(),
                        );
                    }
                    if let Some(msg) = parse_monitor_line(&line) {
                        if !sink(MonitorUpdate::Message(msg)) {
                            return ConnectionEnd::SinkClosed;
                        }
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return ConnectionEnd::Lost(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn frame(s: &str) -> Vec<u8> {
        let mut v = (s.len() as u32).to_le_bytes().to_vec();
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[test]
    fn decoder_frames_split_anywhere() {
        let mut bytes = frame("{\"type\":\"event\",\"name\":\"a\"}\n");
        bytes.extend(frame("{\"name\":\"b\"}\n{\"name\":\"c\"}"));
        bytes.extend(frame(""));
        // Feed byte by byte: frames may arrive in arbitrary chunks.
        let mut d = StreamDecoder::new();
        let mut lines = Vec::new();
        for b in &bytes {
            lines.extend(d.push(&[*b]).unwrap());
        }
        assert_eq!(
            lines,
            vec![
                "{\"type\":\"event\",\"name\":\"a\"}",
                "{\"name\":\"b\"}",
                "{\"name\":\"c\"}"
            ]
        );
    }

    #[test]
    fn decoder_frame_of_length_123_is_not_text() {
        // Length 123 = 0x7b = '{' in the first byte, followed by zero bytes.
        let payload = "x".repeat(123);
        let mut d = StreamDecoder::new();
        assert_eq!(d.push(&frame(&payload)).unwrap(), vec![payload]);
    }

    #[test]
    fn decoder_plain_lines() {
        let mut d = StreamDecoder::new();
        assert!(d.push(b"{\"a\":1}\n{\"b\"").unwrap() == vec!["{\"a\":1}"]);
        assert_eq!(d.push(b":2}\n\n").unwrap(), vec!["{\"b\":2}"]);
    }

    #[test]
    fn errors_classified() {
        assert_eq!(
            classify_response("[!] Query: Item 'x' not found\n".into()),
            Err(IpcError::Daemon("Query: Item 'x' not found".into()))
        );
        assert_eq!(classify_response("{}".into()), Ok("{}".into()));
        assert_eq!(classify_response(String::new()), Ok(String::new()));
        let e: IpcError = io::Error::from(io::ErrorKind::NotFound).into();
        assert_eq!(e, IpcError::NotRunning);
        let e: IpcError = io::Error::from(io::ErrorKind::ConnectionRefused).into();
        assert_eq!(e, IpcError::NotRunning);
    }

    /// Spins up a fake daemon on a private socket and drives the real client paths
    /// through `mbar_ipc::socket` (the transport `Client` uses on every platform).
    fn with_fake_daemon<F: FnOnce(&std::path::Path)>(
        handler: impl Fn(Vec<String>) -> String + Send + Sync + 'static,
        f: F,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "mbar-ui-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("d.socket");
        let server = mbar_ipc::socket::Server::bind(&path).unwrap();
        server
            .spawn(move |req| {
                let rsp = handler(req.args());
                req.respond(&rsp);
            })
            .unwrap();
        f(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_over_socket() {
        with_fake_daemon(
            |args| {
                match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["--query", "bar"] => "{\n\t\"position\": \"top\",\n\t\"items\": [\n\t\t \"a\",\n\t\t \"b\"\n\t]\n}\n".into(),
                ["--query", "item", "a"] => "{\"name\":\"a\",\"type\":\"item\",\"geometry\":{\"position\":\"left\",\"drawing\":\"on\"}}".into(),
                ["--query", "item", n] => format!("[!] Query: Item '{n}' not found\n"),
                _ => String::new(),
            }
            },
            |path| {
                let send = |args: &[&str]| {
                    mbar_ipc::socket::send(path, &mbar_ipc::encode_args(args))
                        .map_err(IpcError::from)
                        .and_then(classify_response)
                };
                let bar = BarInfo::parse(&send(&["--query", "bar"]).unwrap()).unwrap();
                assert_eq!(bar.items, vec!["a", "b"]);
                let a = ItemInfo::parse("a", &send(&["--query", "item", "a"]).unwrap()).unwrap();
                assert_eq!(a.position, crate::model::Position::Left);
                assert!(matches!(
                    send(&["--query", "item", "b"]),
                    Err(IpcError::Daemon(m)) if m.contains("not found")
                ));
            },
        );
    }

    #[test]
    fn client_not_running() {
        let c = Client::new(format!("mbar-ui-test-none-{}", std::process::id()));
        assert_eq!(c.status(), DaemonStatus::NotRunning);
        assert!(matches!(c.fetch_snapshot(), Err(IpcError::NotRunning)));
        assert!(matches!(c.trigger("", ""), Err(IpcError::Io(_))));
    }

    #[test]
    fn monitor_stream_end_to_end() {
        // Fake daemon: answers `--monitor events` with two framed events, then closes.
        let bar_name = format!("mbar-ui-test-mon-{}", std::process::id());
        let path = mbar_ipc::socket_path(&bar_name);
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let req = mbar_ipc::socket::read_frame(&mut s).unwrap();
            assert_eq!(mbar_ipc::decode_args(&req), vec!["--monitor", "events"]);
            let ev = |n: &str| {
                format!("{{\"type\":\"event\",\"name\":\"{n}\",\"sender\":\"s\",\"info\":\"\",\"items\":[],\"ts_ms\":1}}\n")
            };
            mbar_ipc::socket::write_frame(&mut s, ev("one").as_bytes()).unwrap();
            mbar_ipc::socket::write_frame(&mut s, ev("two").as_bytes()).unwrap();
            drop(s);
            drop(listener);
        });
        let (tx, rx) = mpsc::channel();
        let handle = spawn_monitor(&bar_name, "events", move |u| tx.send(u).is_ok()).unwrap();
        let mut names = Vec::new();
        let mut connected = false;
        let mut disconnected = false;
        while let Ok(u) = rx.recv_timeout(Duration::from_secs(5)) {
            match u {
                MonitorUpdate::Connected => connected = true,
                MonitorUpdate::Message(MonitorMessage::Event(e)) => names.push(e.name),
                MonitorUpdate::Disconnected(_) => {
                    disconnected = true;
                    break;
                }
                MonitorUpdate::Message(_) => {}
            }
        }
        server.join().unwrap();
        drop(handle);
        let _ = std::fs::remove_file(&path);
        assert!(connected);
        assert!(disconnected);
        assert_eq!(names, vec!["one", "two"]);
    }
}
