//! Mach IPC server `dev.rubeen.<bar_name>` (`docs/ARCHITECTURE.md` "mbar-ipc") and
//! one-way sends to `mach_helper` services (`docs/spec/events.md` §4.6).
//!
//! Wire layout (SketchyBar's `struct mach_message`): a complex header, the descriptor count
//! and one out-of-line descriptor pointing at the payload. Apple's headers pack descriptors
//! with `#pragma pack(4)`, so the descriptor starts at byte 28 and the message is 44 bytes;
//! [`WireMessage`] reproduces that with `repr(C, packed(4))`.
//!
//! The service is registered with `bootstrap_register` (like SketchyBar). Requests are
//! received by a `CFMachPort` source on the **main** run loop and posted as
//! [`SysEvent::MachMessage`]; the [`MachReply`] can be answered at any later time.

use super::{Sink, SysEvent};
use mach2::bootstrap::{bootstrap_look_up, bootstrap_register};
use mach2::kern_return::KERN_SUCCESS;
use mach2::mach_port::{mach_port_allocate, mach_port_deallocate, mach_port_insert_right};
use mach2::message::{
    mach_msg, mach_msg_header_t, mach_msg_ool_descriptor_t, mach_msg_size_t, MACH_MSGH_BITS_COMPLEX,
    MACH_MSG_OOL_DESCRIPTOR, MACH_MSG_SUCCESS, MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND,
    MACH_MSG_TYPE_MOVE_SEND, MACH_MSG_VIRTUAL_COPY, MACH_SEND_MSG, MACH_SEND_TIMEOUT,
};
use mach2::port::{mach_port_t, MACH_PORT_NULL, MACH_PORT_RIGHT_RECEIVE};
use mach2::task::{task_get_special_port, TASK_BOOTSTRAP_PORT};
use mach2::traps::mach_task_self;
use objc2_core_foundation::{
    kCFRunLoopDefaultMode, CFIndex, CFMachPort, CFMachPortContext, CFRetained, CFRunLoop,
    CFRunLoopSource,
};
use std::ffi::{c_void, CString};

/// Send timeout for replies and helper messages (ms). A client that gave up must not block
/// the daemon.
pub const SEND_TIMEOUT_MS: u32 = 200;

/// SketchyBar's message layout (descriptors packed to 4 bytes).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct WireMessage {
    pub header: mach_msg_header_t,
    pub descriptor_count: mach_msg_size_t,
    pub descriptor: mach_msg_ool_descriptor_t,
}

/// Bootstrap name for a bar name (`dev.rubeen.<bar_name>`).
pub fn service_name(bar_name: &str) -> String {
    format!("dev.rubeen.{bar_name}")
}

/// Splits a request payload into argv: `\0`-separated tokens, terminated by an empty token
/// (`\0\0`) or the end of the buffer.
pub fn parse_payload(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for tok in bytes.split(|b| *b == 0) {
        if tok.is_empty() {
            break;
        }
        out.push(String::from_utf8_lossy(tok).into_owned());
    }
    out
}

/// Builds a request payload from argv (inverse of [`parse_payload`]).
pub fn build_payload<S: AsRef<str>>(args: &[S]) -> Vec<u8> {
    let mut out = Vec::new();
    for a in args {
        out.extend_from_slice(a.as_ref().as_bytes());
        out.push(0);
    }
    out.push(0);
    out
}

/// Sends `bytes` as one OOL descriptor to `port` with the given remote disposition.
/// Returns the `mach_msg` result.
fn send_ool(port: mach_port_t, disposition: u32, bytes: &[u8]) -> i32 {
    let mut msg = WireMessage {
        // SAFETY: an all-zero header is a valid starting value (plain integers).
        header: unsafe { std::mem::zeroed() },
        descriptor_count: 1,
        descriptor: mach_msg_ool_descriptor_t::new(
            bytes.as_ptr() as *mut c_void,
            false,
            MACH_MSG_VIRTUAL_COPY,
            bytes.len() as mach_msg_size_t,
        ),
    };
    msg.header.msgh_remote_port = port;
    msg.header.msgh_local_port = MACH_PORT_NULL;
    msg.header.msgh_bits = disposition | MACH_MSGH_BITS_COMPLEX;
    msg.header.msgh_size = std::mem::size_of::<WireMessage>() as mach_msg_size_t;
    // SAFETY: `msg` is a complete, correctly sized message; the OOL buffer outlives the call
    // (virtual copy happens inside mach_msg).
    unsafe {
        mach_msg(
            std::ptr::addr_of_mut!(msg.header),
            MACH_SEND_MSG | MACH_SEND_TIMEOUT,
            msg.header.msgh_size,
            0,
            MACH_PORT_NULL,
            SEND_TIMEOUT_MS,
            MACH_PORT_NULL,
        )
    }
}

/// The reply right of one request. Send exactly once with [`MachReply::send`]; dropping it
/// unanswered sends an empty response so the client never waits for its timeout.
pub struct MachReply {
    port: mach_port_t,
    sent: bool,
}

impl std::fmt::Debug for MachReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachReply").field("port", &self.port).field("sent", &self.sent).finish()
    }
}

impl MachReply {
    /// A reply that goes nowhere (requests without a reply port).
    pub fn none() -> MachReply {
        MachReply {
            port: MACH_PORT_NULL,
            sent: true,
        }
    }

    /// Whether the client asked for a response.
    pub fn wants_reply(&self) -> bool {
        self.port != MACH_PORT_NULL && !self.sent
    }

    /// Sends `response` (NUL-terminated on the wire). Callable from any thread. Returns
    /// whether the kernel accepted the message.
    pub fn send(mut self, response: &str) -> bool {
        self.send_inner(response)
    }

    /// Releases the reply right without answering (e.g. `--exit`); the client times out.
    pub fn discard(mut self) {
        if !self.sent && self.port != MACH_PORT_NULL {
            // SAFETY: we own one send right on `port`.
            unsafe { mach_port_deallocate(mach_task_self(), self.port) };
        }
        self.sent = true;
    }

    fn send_inner(&mut self, response: &str) -> bool {
        if self.sent || self.port == MACH_PORT_NULL {
            self.sent = true;
            return false;
        }
        self.sent = true;
        let mut bytes = Vec::with_capacity(response.len() + 1);
        bytes.extend_from_slice(response.as_bytes());
        bytes.push(0);
        let kr = send_ool(self.port, MACH_MSG_TYPE_MOVE_SEND, &bytes);
        if kr != MACH_MSG_SUCCESS {
            // The send right was not consumed (e.g. the client timed out); release it.
            // SAFETY: we own one send right on `port`.
            unsafe { mach_port_deallocate(mach_task_self(), self.port) };
            return false;
        }
        true
    }
}

impl Drop for MachReply {
    fn drop(&mut self) {
        if !self.sent {
            self.send_inner("");
        }
    }
}

/// A running mach server; dropping it removes the run-loop source (the bootstrap name
/// stays registered until the process exits).
pub struct MachServer {
    port: mach_port_t,
    name: String,
    cf_port: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
    ctx: *mut Sink,
}

impl MachServer {
    /// The bootstrap name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The receive port.
    pub fn port(&self) -> mach_port_t {
        self.port
    }
}

impl Drop for MachServer {
    fn drop(&mut self) {
        if let Some(main) = CFRunLoop::main() {
            // SAFETY: reading an immutable CF constant.
            main.remove_source(Some(&self.source), unsafe { kCFRunLoopDefaultMode });
        }
        self.cf_port.invalidate();
        // SAFETY: after invalidation no callback can observe `ctx`.
        unsafe { drop(Box::from_raw(self.ctx)) };
    }
}

unsafe extern "C-unwind" fn mach_callback(
    _port: *mut CFMachPort,
    msg: *mut c_void,
    size: CFIndex,
    info: *mut c_void,
) {
    if msg.is_null() || info.is_null() || (size as usize) < std::mem::size_of::<mach_msg_header_t>() {
        return;
    }
    // SAFETY: `info` is the leaked `Box<Sink>` owned by the `MachServer`.
    let sink = unsafe { &*(info as *const Sink) };
    // SAFETY: CF hands us the received message (`size` bytes).
    let header: mach_msg_header_t = unsafe { std::ptr::read_unaligned(msg as *const mach_msg_header_t) };
    let reply = MachReply {
        port: header.msgh_remote_port,
        sent: header.msgh_remote_port == MACH_PORT_NULL,
    };
    let mut payload = Vec::new();
    if header.msgh_bits & MACH_MSGH_BITS_COMPLEX != 0 && (size as usize) >= std::mem::size_of::<WireMessage>() {
        // SAFETY: complex message of at least the wire size.
        let wire: WireMessage = unsafe { std::ptr::read_unaligned(msg as *const WireMessage) };
        let desc = wire.descriptor;
        if wire.descriptor_count >= 1 && desc.type_ as u32 == MACH_MSG_OOL_DESCRIPTOR && !desc.address.is_null() {
            let len = desc.size as usize;
            // SAFETY: the kernel mapped `len` bytes at `address` into our address space; we
            // copy them and then release the mapping (we own it after receive).
            unsafe {
                payload.extend_from_slice(std::slice::from_raw_parts(desc.address as *const u8, len));
                mach2::vm::mach_vm_deallocate(mach_task_self(), desc.address as u64, len as u64);
            }
        }
    }
    let args = parse_payload(&payload);
    sink(SysEvent::MachMessage {
        args,
        payload,
        reply,
    });
}

fn bootstrap_port() -> Option<mach_port_t> {
    let mut bs: mach_port_t = MACH_PORT_NULL;
    // SAFETY: valid out pointer for our own task.
    let kr = unsafe { task_get_special_port(mach_task_self(), TASK_BOOTSTRAP_PORT, &mut bs) };
    (kr == KERN_SUCCESS).then_some(bs)
}

/// Registers `dev.rubeen.<bar_name>` and attaches the receive source to the main run loop.
/// Fails if the name is already registered (another daemon with the same bar name).
pub fn start(bar_name: &str, sink: Sink) -> Result<MachServer, String> {
    let name = service_name(bar_name);
    if lookup(&name).is_some() {
        return Err(format!("mach service {name} is already registered"));
    }
    let bs = bootstrap_port().ok_or("task_get_special_port failed")?;
    // SAFETY: returns this task's port name; no preconditions.
    let task = unsafe { mach_task_self() };
    let mut port: mach_port_t = MACH_PORT_NULL;
    // SAFETY: valid out pointer; then insert a send right for the same name.
    unsafe {
        if mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut port) != KERN_SUCCESS {
            return Err("mach_port_allocate failed".into());
        }
        if mach_port_insert_right(task, port, port, MACH_MSG_TYPE_MAKE_SEND) != KERN_SUCCESS {
            return Err("mach_port_insert_right failed".into());
        }
    }
    let cname = CString::new(name.clone()).map_err(|e| e.to_string())?;
    // SAFETY: valid bootstrap port, NUL-terminated name and a port with a send right.
    // `bootstrap_register` is deprecated but still works for per-user services (SketchyBar).
    #[allow(deprecated)]
    let kr = unsafe { bootstrap_register(bs, cname.as_ptr() as *mut _, port) };
    if kr != KERN_SUCCESS {
        return Err(format!("bootstrap_register({name}) failed: {kr}"));
    }
    let ctx = Box::into_raw(Box::new(sink));
    let mut context = CFMachPortContext {
        version: 0,
        info: ctx.cast(),
        retain: None,
        release: None,
        copyDescription: None,
    };
    // SAFETY: `port` holds a receive right; the context pointer stays valid until Drop.
    let cf_port = unsafe {
        CFMachPort::with_port(None, port, Some(mach_callback), &mut context, std::ptr::null_mut())
    };
    let Some(cf_port) = cf_port else {
        // SAFETY: reclaim the leaked context on failure.
        unsafe { drop(Box::from_raw(ctx)) };
        return Err("CFMachPortCreateWithPort failed".into());
    };
    let Some(source) = CFMachPort::new_run_loop_source(None, Some(&cf_port), 0) else {
        cf_port.invalidate();
        // SAFETY: the port is invalidated, no callback can run.
        unsafe { drop(Box::from_raw(ctx)) };
        return Err("CFMachPortCreateRunLoopSource failed".into());
    };
    let main = CFRunLoop::main().ok_or("no main run loop")?;
    // SAFETY: reading an immutable CF constant.
    main.add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
    Ok(MachServer {
        port,
        name,
        cf_port,
        source,
        ctx,
    })
}

/// `bootstrap_look_up(service)`.
pub fn lookup(service: &str) -> Option<mach_port_t> {
    let name = CString::new(service).ok()?;
    let bs = bootstrap_port()?;
    let mut port: mach_port_t = MACH_PORT_NULL;
    // SAFETY: valid bootstrap port, NUL-terminated name and out pointer.
    let kr = unsafe { bootstrap_look_up(bs, name.as_ptr(), &mut port) };
    (kr == KERN_SUCCESS && port != MACH_PORT_NULL).then_some(port)
}

/// One-way message to a looked-up port (`mach_helper` delivery; no reply port).
pub fn send_oneway(port: mach_port_t, payload: &[u8]) -> bool {
    send_ool(port, MACH_MSG_TYPE_COPY_SEND, payload) == MACH_MSG_SUCCESS
}

/// Looks up `service` and sends `payload` one-way (`PlatformRequest::MachSend`).
pub fn send_to_service(service: &str, payload: &[u8]) -> bool {
    match lookup(service) {
        Some(port) => {
            let ok = send_oneway(port, payload);
            // SAFETY: `bootstrap_look_up` gave us a send right we no longer need.
            unsafe { mach_port_deallocate(mach_task_self(), port) };
            ok
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_c() {
        assert_eq!(std::mem::size_of::<WireMessage>(), 44);
        assert_eq!(std::mem::offset_of!(WireMessage, descriptor), 28);
    }

    #[test]
    fn payload_roundtrip() {
        let p = build_payload(&["--set", "clock", "label=12:00"]);
        assert_eq!(p, b"--set\0clock\0label=12:00\0\0");
        assert_eq!(parse_payload(&p), vec!["--set", "clock", "label=12:00"]);
        assert_eq!(parse_payload(b"a\0b"), vec!["a", "b"]);
        assert!(parse_payload(b"").is_empty());
        assert_eq!(parse_payload(b"a\0\0b\0\0"), vec!["a"]);
        assert_eq!(service_name("mbar"), "dev.rubeen.mbar");
    }
}
