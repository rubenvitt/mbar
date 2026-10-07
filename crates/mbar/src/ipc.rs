//! Daemon side of the Unix-socket transport.
//!
//! Same wire format as `mbar_ipc::socket` (u32 LE length + payload, payload = argv
//! joined by `\0`). This accept loop is used instead of `mbar_ipc::socket::Server`
//! because `--monitor` needs to keep the connection after the reply, which
//! `mbar_ipc::socket::Request` does not expose (see [`Responder::into_stream`]).

use mbar_ipc::socket::{CLIENT_TIMEOUT, MAX_FRAME};
use std::io::{self, Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Instant;

use crate::driver::{Event, Post};

/// Where the reply of one request goes. Exactly one of [`Responder::respond`] /
/// [`Responder::into_stream`] is used; dropping it closes the connection without a reply
/// (used after `--exit`; clients treat that as an empty response).
pub enum Responder {
    /// A Unix-socket connection.
    Socket(UnixStream),
    /// Any other transport (e.g. the macOS mach server): called with the reply text.
    #[allow(dead_code)] // constructed by the macOS platform (mach transport)
    Callback(Box<dyn FnOnce(String) + Send>),
}

impl Responder {
    /// Sends the reply without blocking the caller (the daemon's main thread): what fits
    /// into the socket buffer is written right away, the rest of a large reply to a slow
    /// reader is finished on a short-lived thread (bounded by the stream's write timeout).
    pub fn respond(self, text: &str) {
        match self {
            Responder::Socket(s) => send_frame_nonblocking(s, encode_frame(text.as_bytes())),
            Responder::Callback(f) => f(text.to_string()),
        }
    }

    /// The connection, for streaming replies (`--monitor`). `Err` gives the responder back
    /// for transports that cannot stream.
    pub fn into_stream(self) -> Result<UnixStream, Responder> {
        match self {
            Responder::Socket(s) => Ok(s),
            other => Err(other),
        }
    }
}

/// `u32` LE length + payload (the `mbar_ipc::socket` framing) as one buffer.
pub fn encode_frame(data: &[u8]) -> Vec<u8> {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    let mut buf = Vec::with_capacity(4 + data.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(data);
    buf
}

/// Writes `frame` to `s` and closes it. Never blocks: a remainder that does not fit into
/// the socket buffer is written by a helper thread.
fn send_frame_nonblocking(mut s: UnixStream, frame: Vec<u8>) {
    if s.set_nonblocking(true).is_err() {
        let _ = s.write_all(&frame);
        return;
    }
    let mut off = 0;
    while off < frame.len() {
        match s.write(&frame[off..]) {
            Ok(0) => return,
            Ok(n) => off += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => return,
        }
    }
    if off == frame.len() || s.set_nonblocking(false).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mbar-ipc-reply".into())
        .spawn(move || {
            let _ = s.write_all(&frame[off..]);
        });
    if let Err(e) = spawned {
        log::warn!("ipc: cannot spawn reply writer: {e}");
    }
}

impl std::fmt::Debug for Responder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Responder::Socket(_) => f.write_str("Responder::Socket"),
            Responder::Callback(_) => f.write_str("Responder::Callback"),
        }
    }
}

/// The bound socket (`mbar_ipc::socket_path`, mode 0600).
pub struct Listener {
    listener: UnixListener,
}

impl Listener {
    /// Binds the socket. Fails with `AddrInUse` if a daemon answers on it; a stale socket
    /// file is replaced.
    pub fn bind(path: &Path) -> io::Result<Listener> {
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another instance is running",
                ));
            }
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        mbar_ipc::socket::restrict_socket_mode(path)?;
        Ok(Listener { listener })
    }

    /// Accept loop on one background thread: a `poll(2)` loop that accepts connections
    /// and reads each request frame without blocking, then posts it as
    /// [`Event::Request`]. No thread per connection (its creation and teardown cost far
    /// more than handling a typical request), and a client that stalls mid-frame only
    /// holds its own connection until [`CLIENT_TIMEOUT`], never the others.
    pub fn spawn(self, post: Post) -> io::Result<()> {
        self.listener.set_nonblocking(true)?;
        std::thread::Builder::new()
            .name("mbar-ipc".into())
            .spawn(move || serve(self.listener, post))?;
        Ok(())
    }
}

/// A connection whose request frame has not fully arrived yet.
struct Pending {
    stream: UnixStream,
    /// Bytes read so far: the 4-byte length header, then the payload.
    buf: Vec<u8>,
    deadline: Instant,
}

/// What [`Pending::advance`] made of the bytes available right now.
enum Progress {
    /// The frame is incomplete; wait for more.
    Waiting,
    /// The complete payload.
    Done(Vec<u8>),
    /// EOF, an I/O error or an oversized frame: drop the connection.
    Failed(io::Error),
}

impl Pending {
    fn new(stream: UnixStream) -> Pending {
        Pending {
            stream,
            buf: Vec::with_capacity(256),
            deadline: Instant::now() + CLIENT_TIMEOUT,
        }
    }

    /// Bytes still needed for the frame (never reads past it, so anything the client
    /// sends after its request stays in the socket, like `read_frame`).
    fn needed(&self) -> Result<usize, io::Error> {
        if self.buf.len() < 4 {
            return Ok(4 - self.buf.len());
        }
        let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        let len = len as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }
        Ok(4 + len - self.buf.len())
    }

    /// Reads what is available without blocking.
    fn advance(&mut self) -> Progress {
        loop {
            let need = match self.needed() {
                Ok(0) => return Progress::Done(self.buf.split_off(4)),
                Ok(n) => n,
                Err(e) => return Progress::Failed(e),
            };
            let old = self.buf.len();
            // Grow in bounded steps: a header announcing a huge frame must not make the
            // daemon allocate it before the bytes arrive.
            let chunk = need.min(64 * 1024);
            self.buf.resize(old + chunk, 0);
            let res = self.stream.read(&mut self.buf[old..]);
            let got = *res.as_ref().unwrap_or(&0);
            self.buf.truncate(old + got);
            match res {
                Ok(0) => {
                    return Progress::Failed(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed mid-request",
                    ))
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Progress::Waiting,
                Err(e) => return Progress::Failed(e),
            }
        }
    }
}

/// The accept/read loop of [`Listener::spawn`] (`listener` is non-blocking).
fn serve(listener: UnixListener, post: Post) {
    let mut pending: Vec<Pending> = Vec::new();
    let mut fds: Vec<libc::pollfd> = Vec::new();
    loop {
        let now = Instant::now();
        pending.retain(|p| {
            let alive = p.deadline > now;
            if !alive {
                log::debug!("ipc: bad request: timed out");
            }
            alive
        });
        let timeout = pending
            .iter()
            .map(|p| p.deadline.saturating_duration_since(now))
            .min()
            .map_or(-1, |d| {
                // Round up so the loop does not spin just before a deadline.
                i32::try_from(d.as_millis() + 1).unwrap_or(i32::MAX)
            });

        fds.clear();
        fds.push(libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        fds.extend(pending.iter().map(|p| libc::pollfd {
            fd: p.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }));
        // SAFETY: `fds` is a valid, initialised array of `fds.len()` pollfd structs that
        // outlives the call.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                log::warn!("ipc: poll failed: {e}");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            continue;
        }

        // Connections with news, newest first so `swap_remove` keeps earlier indices.
        for i in (1..fds.len()).rev() {
            if fds[i].revents == 0 {
                continue;
            }
            match pending[i - 1].advance() {
                Progress::Waiting => {}
                Progress::Done(payload) => {
                    let p = pending.swap_remove(i - 1);
                    dispatch(p.stream, &payload, &post);
                }
                Progress::Failed(e) => {
                    log::debug!("ipc: bad request: {e}");
                    pending.swap_remove(i - 1);
                }
            }
        }

        if fds[0].revents != 0 {
            accept_all(&listener, &mut pending, &post);
        }
    }
}

/// Accepts every queued connection. A request that is already complete is posted right
/// away; the rest wait in `pending`.
fn accept_all(listener: &UnixListener, pending: &mut Vec<Pending>, post: &Post) {
    loop {
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                // E.g. EMFILE: leave the rest queued; back off briefly so a listener
                // that stays readable does not make the loop spin.
                log::warn!("ipc: accept failed: {e}");
                std::thread::sleep(std::time::Duration::from_millis(10));
                return;
            }
        };
        // Defence in depth next to the private socket directory: only this user (or
        // root) may send commands.
        if !mbar_ipc::socket::peer_trusted(&stream) {
            log::warn!("ipc: rejected a connection from another user");
            continue;
        }
        if let Err(e) = stream.set_nonblocking(true) {
            log::debug!("ipc: bad request: {e}");
            continue;
        }
        let mut p = Pending::new(stream);
        match p.advance() {
            Progress::Waiting => pending.push(p),
            Progress::Done(payload) => dispatch(p.stream, &payload, post),
            Progress::Failed(e) => log::debug!("ipc: bad request: {e}"),
        }
    }
}

/// Hands a complete request to the main thread. The stream goes back to blocking mode
/// (replies and `--monitor` streaming expect it) with a write timeout.
fn dispatch(stream: UnixStream, payload: &[u8], post: &Post) {
    if let Err(e) = stream.set_nonblocking(false) {
        log::debug!("ipc: bad request: {e}");
        return;
    }
    let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
    post(Event::Request {
        args: mbar_ipc::decode_args(payload),
        responder: Responder::Socket(stream),
    });
}

#[cfg(test)]
mod tests {
    //! PERF-7: the listener reads requests on its own poll loop instead of one thread per
    //! connection, and a stalled client does not hold up the others.
    use super::*;
    use mbar_ipc::socket::{read_frame, write_frame};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A listener in a fresh private directory whose requests are answered with their
    /// arguments joined by spaces.
    fn echo_server() -> (Dir, PathBuf, Arc<AtomicUsize>) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "mbar-ipc-unit-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = dir.join("s.socket");
        let listener = Listener::bind(&sock).unwrap();
        assert_eq!(
            std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = hits.clone();
        let post: Post = Arc::new(move |ev| {
            if let Event::Request { args, responder } = ev {
                hits2.fetch_add(1, Ordering::SeqCst);
                responder.respond(&args.join(" "));
            }
        });
        listener.spawn(post).unwrap();
        (Dir(dir), sock, hits)
    }

    fn request(sock: &Path, args: &[&str]) -> String {
        mbar_ipc::socket::send(sock, &mbar_ipc::encode_args(args)).unwrap()
    }

    /// Names of this process's threads (Linux only).
    #[cfg(target_os = "linux")]
    fn thread_names() -> Vec<String> {
        std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("comm")).ok())
            .map(|s| s.trim().to_string())
            .collect()
    }

    #[test]
    fn sequential_requests_are_answered() {
        let (_d, sock, hits) = echo_server();
        for i in 0..200 {
            let label = format!("label={i}");
            assert_eq!(
                request(&sock, &["--set", "a", &label]),
                format!("--set a {label}")
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 200);
    }

    #[test]
    fn stalled_clients_do_not_block_others_or_cost_threads() {
        let (_d, sock, hits) = echo_server();
        // Clients that connect and send nothing, or only part of a frame.
        let mut stalled = Vec::new();
        for i in 0..40 {
            let mut s = UnixStream::connect(&sock).unwrap();
            if i % 2 == 1 {
                s.write_all(&[9, 0]).unwrap();
            } else if i % 4 == 2 {
                s.write_all(&[9, 0, 0, 0, b'-']).unwrap();
            }
            stalled.push(s);
        }
        let start = Instant::now();
        assert_eq!(request(&sock, &["--query", "bar"]), "--query bar");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "a stalled client delayed another request ({:?})",
            start.elapsed()
        );
        #[cfg(target_os = "linux")]
        {
            let names = thread_names();
            assert!(
                !names.iter().any(|n| n.starts_with("mbar-ipc-conn")),
                "{names:?}"
            );
            assert!(names.iter().filter(|n| n.starts_with("mbar-ipc")).count() < 10);
        }
        // A stalled client can still finish its request later.
        let mut late = stalled.pop().unwrap(); // i = 39: sent [9, 0]
        late.write_all(&[0, 0]).unwrap();
        late.write_all(b"--update\0\0").unwrap();
        late.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(read_frame(&mut late).unwrap(), b"--update");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn frame_split_across_writes_and_eof_mid_frame() {
        let (_d, sock, hits) = echo_server();
        let mut s = UnixStream::connect(&sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let payload = mbar_ipc::encode_args(&["--set", "a", &"x".repeat(200_000)]);
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&payload);
        for chunk in frame.chunks(7_001) {
            s.write_all(chunk).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        let rsp = read_frame(&mut s).unwrap();
        assert_eq!(rsp.len(), "--set a ".len() + 200_000);

        // Closing mid-frame drops the request without an answer; the server lives on.
        let mut t = UnixStream::connect(&sock).unwrap();
        t.write_all(&[50, 0, 0, 0, b'a']).unwrap();
        drop(t);
        // Oversized frames are refused.
        let mut u = UnixStream::connect(&sock).unwrap();
        u.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        u.write_all(&u32::MAX.to_le_bytes()).unwrap();
        let mut rest = Vec::new();
        let _ = u.read_to_end(&mut rest);
        assert!(rest.is_empty());
        assert_eq!(request(&sock, &["ok"]), "ok");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reply_stream_is_blocking_for_monitor() {
        let dir = std::env::temp_dir().join(format!(
            "mbar-ipc-unit-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let _d = Dir(dir.clone());
        let sock = dir.join("m.socket");
        let listener = Listener::bind(&sock).unwrap();
        let post: Post = Arc::new(|ev| {
            if let Event::Request { responder, .. } = ev {
                let mut s = responder.into_stream().unwrap();
                // Several frames, as `--monitor` streams them, larger than the socket
                // buffer: the stream must be back in blocking mode.
                for i in 0..3 {
                    write_frame(
                        &mut s,
                        format!("event {i} {}", "y".repeat(1 << 20)).as_bytes(),
                    )
                    .unwrap();
                }
            }
        });
        listener.spawn(post).unwrap();
        let mut s = UnixStream::connect(&sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write_frame(&mut s, &mbar_ipc::encode_args(&["--monitor"])).unwrap();
        for i in 0..3 {
            let frame = read_frame(&mut s).unwrap();
            assert!(frame.starts_with(format!("event {i} y").as_bytes()));
            assert_eq!(frame.len(), format!("event {i} ").len() + (1 << 20));
        }
    }
}
