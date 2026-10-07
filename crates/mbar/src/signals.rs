//! Termination signals (`SIGTERM`, `SIGINT`, `SIGHUP`): the daemon shuts down like
//! `--exit` instead of dying on the default action, so the platform's cleanup runs
//! (menu-bar auto-hide restored, windows closed, socket removed, script process groups
//! terminated; see `platform::platform_main`).
//!
//! Self-pipe: the handler only `write`s the signal number into a non-blocking pipe
//! (async-signal-safe, `errno` preserved); a small forwarding thread blocks in `read` and
//! turns every byte into a callback (the platforms post `Event::Terminate`, which wakes the
//! headless mpsc loop or the macOS main queue). A real handler is used rather than
//! `SIG_IGN` + a dispatch source or a blocked signal mask + `sigwait`, because ignored
//! dispositions and blocked masks are inherited by every child (scripts, `io.popen`,
//! `os.execute`), which would make them deaf to `SIGTERM`/`SIGINT`; a caught signal is
//! reset to its default action by `exec`.
//!
//! [`install`] runs before the platform starts so a signal that arrives during startup is
//! kept in the pipe until [`Signals::forward`] starts the thread.

use std::ffi::c_int;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicI32, Ordering};

/// The signals that terminate the daemon cleanly.
pub const TERMINATION_SIGNALS: [c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

/// Write end of the self-pipe (-1 before [`install`]).
static PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

/// Read end of the self-pipe; [`Signals::forward`] hands it to the forwarding thread.
#[derive(Debug)]
pub struct Signals {
    read: OwnedFd,
}

#[cfg(target_os = "linux")]
fn errno_location() -> *mut c_int {
    // SAFETY: returns the calling thread's errno slot.
    unsafe { libc::__errno_location() }
}

#[cfg(target_os = "macos")]
fn errno_location() -> *mut c_int {
    // SAFETY: returns the calling thread's errno slot.
    unsafe { libc::__error() }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn errno_location() -> *mut c_int {
    std::ptr::null_mut()
}

extern "C" fn on_signal(sig: c_int) {
    let fd = PIPE_WRITE.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    let errno = errno_location();
    // SAFETY: `write` and errno access are async-signal-safe; `errno` is this thread's
    // slot (or null on platforms where it is not preserved).
    unsafe {
        let saved = if errno.is_null() { 0 } else { *errno };
        let byte = sig as u8;
        // A full pipe (EAGAIN) means a signal is already pending: nothing is lost.
        let _ = libc::write(fd, (&byte as *const u8).cast(), 1);
        if !errno.is_null() {
            *errno = saved;
        }
    }
}

fn set_flags(fd: c_int, cloexec: bool, nonblocking: bool) -> io::Result<()> {
    // SAFETY: `fcntl` on a descriptor we own.
    unsafe {
        if cloexec {
            let f = libc::fcntl(fd, libc::F_GETFD);
            if f < 0 || libc::fcntl(fd, libc::F_SETFD, f | libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        if nonblocking {
            let f = libc::fcntl(fd, libc::F_GETFL);
            if f < 0 || libc::fcntl(fd, libc::F_SETFL, f | libc::O_NONBLOCK) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

/// Creates the self-pipe and installs the handler for [`TERMINATION_SIGNALS`]. Call once,
/// before the platform starts; signals received until [`Signals::forward`] are queued.
pub fn install() -> io::Result<Signals> {
    let mut fds = [0 as c_int; 2];
    // SAFETY: `fds` has room for the two descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned here.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    set_flags(read.as_raw_fd(), true, false)?;
    set_flags(write.as_raw_fd(), true, true)?;
    // The write end lives for the rest of the process (the handler may run at any time).
    let write_fd = std::os::fd::IntoRawFd::into_raw_fd(write);
    let old = PIPE_WRITE.swap(write_fd, Ordering::SeqCst);
    if old >= 0 {
        // SAFETY: the previous write end is no longer reachable by the handler.
        unsafe { libc::close(old) };
    }
    for sig in TERMINATION_SIGNALS {
        // SAFETY: `sigaction` with a zeroed struct, an empty mask and a handler that only
        // makes async-signal-safe calls.
        let rc = unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as extern "C" fn(c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut())
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(Signals { read })
}

/// `SIGTERM` → `"SIGTERM"` (log messages).
pub fn name(sig: c_int) -> &'static str {
    match sig {
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        libc::SIGHUP => "SIGHUP",
        _ => "signal",
    }
}

impl Signals {
    /// Starts the forwarding thread: `on_signal(sig)` runs on it for every received
    /// termination signal (queued ones first).
    pub fn forward(self, on_signal: impl Fn(c_int) + Send + 'static) -> io::Result<()> {
        std::thread::Builder::new()
            .name("mbar-signals".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let fd = self.read.as_raw_fd();
                let mut byte = 0u8;
                loop {
                    // SAFETY: reading one byte into a valid buffer from a descriptor we own.
                    let n = unsafe { libc::read(fd, (&mut byte as *mut u8).cast(), 1) };
                    match n {
                        1 => on_signal(c_int::from(byte)),
                        0 => return,
                        _ => {
                            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                                return;
                            }
                        }
                    }
                }
            })?;
        Ok(())
    }
}
