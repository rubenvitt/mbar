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
    let len = u32::try_from(data.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
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
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut data = vec![0u8; len];
    r.read_exact(&mut data)?;
    Ok(data)
}

/// Client: sends one request and waits for its response.
pub fn send(path: &Path, payload: &[u8]) -> io::Result<String> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    write_frame(&mut stream, payload)?;
    let rsp = read_frame(&mut stream)?;
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
                return Err(io::Error::new(io::ErrorKind::AddrInUse, "another instance is running"));
            }
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        Ok(Server { path: path.to_path_buf(), listener })
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
        std::thread::Builder::new().name("mbar-ipc".into()).spawn(move || {
            let server = self;
            for conn in server.listener.incoming() {
                let Ok(mut stream) = conn else { continue };
                let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
                match read_frame(&mut stream) {
                    Ok(payload) => on_request(Request { payload, stream: Some(stream) }),
                    Err(e) => log::debug!("ipc: bad request: {e}"),
                }
            }
        })
    }
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
