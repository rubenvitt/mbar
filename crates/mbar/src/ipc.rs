//! Daemon side of the Unix-socket transport.
//!
//! Same wire format as `mbar_ipc::socket` (u32 LE length + payload, payload = argv
//! joined by `\0`). This accept loop is used instead of `mbar_ipc::socket::Server`
//! because `--monitor` needs to keep the connection after the reply, which
//! `mbar_ipc::socket::Request` does not expose (see [`Responder::into_stream`]).

use mbar_ipc::socket::{read_frame, write_frame, CLIENT_TIMEOUT};
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::driver::{Event, Post};

/// Where the reply of one request goes. Exactly one of [`Responder::respond`] /
/// [`Responder::into_stream`] is used; dropping it closes the connection without a reply
/// (used after `--exit`; clients treat that as an empty response).
pub enum Responder {
    /// A Unix-socket connection.
    Socket(UnixStream),
    /// Any other transport (e.g. the macOS mach server): called with the reply text.
    Callback(Box<dyn FnOnce(String) + Send>),
}

impl Responder {
    pub fn respond(self, text: &str) {
        match self {
            Responder::Socket(mut s) => {
                let _ = write_frame(&mut s, text.as_bytes());
            }
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

impl std::fmt::Debug for Responder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Responder::Socket(_) => f.write_str("Responder::Socket"),
            Responder::Callback(_) => f.write_str("Responder::Callback"),
        }
    }
}

/// The bound socket (`$TMPDIR/mbar_<user>_<bar>.socket`).
pub struct Listener {
    path: PathBuf,
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
        Ok(Listener {
            path: path.to_path_buf(),
            listener,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accept loop on a background thread. Each connection's request is read on a
    /// short-lived thread and posted as [`Event::Request`].
    pub fn spawn(self, post: Post) -> io::Result<()> {
        std::thread::Builder::new()
            .name("mbar-ipc".into())
            .spawn(move || {
                for conn in self.listener.incoming() {
                    let Ok(stream) = conn else { continue };
                    let post = post.clone();
                    let spawned = std::thread::Builder::new()
                        .name("mbar-ipc-conn".into())
                        .spawn(move || read_request(stream, &post));
                    if let Err(e) = spawned {
                        log::warn!("ipc: cannot spawn reader: {e}");
                    }
                }
            })?;
        Ok(())
    }
}

fn read_request(mut stream: UnixStream, post: &Post) {
    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
    match read_frame(&mut stream) {
        Ok(payload) => {
            let _ = stream.set_read_timeout(None);
            let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
            post(Event::Request {
                args: mbar_ipc::decode_args(&payload),
                responder: Responder::Socket(stream),
            });
        }
        Err(e) => log::debug!("ipc: bad request: {e}"),
    }
}
