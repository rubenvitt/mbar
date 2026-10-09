//! Unix domain socket transport. Frame = `u32` little-endian length + bytes.
//! Every request gets exactly one response frame (possibly empty), so clients can
//! rely on the command having been applied when `send` returns.

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Upper bound for a single frame; protects the daemon from garbage writers.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// How long a client waits for the daemon's response.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

pub fn write_frame(w: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let len = u32::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    let mut buf = Vec::with_capacity(4 + data.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(data);
    w.write_all(&buf)?;
    w.flush()
}

pub fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut data = vec![0u8; len];
    r.read_exact(&mut data)?;
    Ok(data)
}

/// Effective uid of the process on the other end of `stream` (`SO_PEERCRED` /
/// `getpeereid`).
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: `ucred` is plain data; getsockopt writes at most `len` bytes into it.
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: valid fd and out pointers.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
}

/// Whether the peer may use this connection: it runs as our effective uid, or as root.
pub fn peer_trusted(stream: &UnixStream) -> bool {
    match peer_uid(stream) {
        Ok(uid) => uid == crate::euid() || uid == 0,
        Err(e) => {
            log::debug!("ipc: cannot read peer credentials: {e}");
            false
        }
    }
}

/// Client: connects to the daemon's socket and verifies that the listener runs as the
/// same user (or root), so a socket bound by another local user is never talked to.
pub fn connect(path: &Path) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(path)?;
    if !peer_trusted(&stream) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is served by another user", path.display()),
        ));
    }
    Ok(stream)
}

/// Client: sends one request and waits for its response.
pub fn send(path: &Path, payload: &[u8]) -> io::Result<String> {
    let mut stream = connect(path)?;
    exchange(&mut stream, payload)
}

/// Client: sends one request on a connected `stream` and waits for its response.
pub fn exchange(stream: &mut UnixStream, payload: &[u8]) -> io::Result<String> {
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    write_frame(stream, payload)?;
    let rsp = read_frame(stream)?;
    Ok(String::from_utf8_lossy(&rsp).into_owned())
}

/// A pending request. The daemon must call [`Request::respond`] exactly once
/// (dropping it sends an empty response).
pub struct Request {
    pub payload: Vec<u8>,
    stream: Option<UnixStream>,
}

impl Request {
    pub fn args(&self) -> Vec<String> {
        crate::decode_args(&self.payload)
    }

    pub fn respond(mut self, response: &str) {
        if let Some(mut s) = self.stream.take() {
            let _ = write_frame(&mut s, response.as_bytes());
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Some(mut s) = self.stream.take() {
            let _ = write_frame(&mut s, b"");
        }
    }
}

/// Server bound to a socket path. Removes the socket file on drop.
pub struct Server {
    path: PathBuf,
    listener: UnixListener,
}

impl Server {
    /// Binds the socket. Fails with `AddrInUse` if another daemon answers on it;
    /// a stale socket file (no listener) is replaced.
    pub fn bind(path: &Path) -> io::Result<Server> {
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
        restrict_socket_mode(path)?;
        Ok(Server {
            path: path.to_path_buf(),
            listener,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Spawns the accept loop on a background thread. Each request is read on its own
    /// short-lived thread and handed to `on_request`, which typically forwards it to
    /// the main thread. Ordering between connections is preserved per client because
    /// every client waits for its response before sending the next request.
    pub fn spawn<F>(self, on_request: F) -> io::Result<std::thread::JoinHandle<()>>
    where
        F: Fn(Request) + Send + Sync + 'static,
    {
        let on_request = std::sync::Arc::new(on_request);
        std::thread::Builder::new()
            .name("mbar-ipc".into())
            .spawn(move || {
                let server = self;
                for conn in server.listener.incoming() {
                    let Ok(mut stream) = conn else { continue };
                    if !peer_trusted(&stream) {
                        log::warn!("ipc: rejected a connection from another user");
                        continue;
                    }
                    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
                    match read_frame(&mut stream) {
                        Ok(payload) => on_request(Request {
                            payload,
                            stream: Some(stream),
                        }),
                        Err(e) => log::debug!("ipc: bad request: {e}"),
                    }
                }
            })
    }
}

/// `chmod 0600` on a freshly bound socket: only this user may connect.
pub fn restrict_socket_mode(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_response() {
        let dir = std::env::temp_dir().join(format!("mbar-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.socket");
        let server = Server::bind(&path).unwrap();
        server
            .spawn(|req| {
                let args = req.args();
                req.respond(&args.join(","));
            })
            .unwrap();
        let rsp = send(&path, &crate::encode_args(&["--query", "bar"])).unwrap();
        assert_eq!(rsp, "--query,bar");
        // Dropped request → empty response.
        let rsp = send(&path, &crate::encode_args::<&str>(&[])).unwrap();
        assert_eq!(rsp, "");
        assert!(Server::bind(&path).is_err());
    }
}
