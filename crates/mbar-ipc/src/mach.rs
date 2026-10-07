//! mach client for the `dev.rubeen.<bar_name>` bootstrap service.
//!
//! Message layout (identical to SketchyBar's `struct mach_message`): a complex
//! header followed by one out-of-line descriptor that points at the payload.
//! The server replies on the port passed in `msgh_local_port` with the same layout.

use mach2::bootstrap::bootstrap_look_up;
use mach2::kern_return::KERN_SUCCESS;
use mach2::mach_port::{
    mach_port_allocate, mach_port_deallocate, mach_port_insert_right, mach_port_mod_refs,
};
use mach2::message::*;
use mach2::port::{mach_port_t, MACH_PORT_NULL, MACH_PORT_RIGHT_RECEIVE};
use mach2::task::{task_get_special_port, TASK_BOOTSTRAP_PORT};
use mach2::traps::mach_task_self;
use std::ffi::CString;

/// Response wait time. SketchyBar uses 100 ms; mbar is more patient so that queries
/// on a busy daemon don't come back empty.
pub const RESPONSE_TIMEOUT_MS: mach_msg_timeout_t = 2000;

/// Apple's headers declare mach messages with 4-byte packing, so the descriptor
/// starts at byte 28 and the message is 44 bytes (a plain `repr(C)` would align
/// the pointer-carrying descriptor to 8 and produce a 48-byte message).
#[repr(C, packed(4))]
pub struct MachMessage {
    pub header: mach_msg_header_t,
    pub descriptor_count: mach_msg_size_t,
    pub descriptor: mach_msg_ool_descriptor_t,
}

#[repr(C, packed(4))]
pub struct MachBuffer {
    pub message: MachMessage,
    pub trailer: mach_msg_trailer_t,
}

/// Looks up a bootstrap service. Returns `None` if it is not registered.
pub fn lookup(service: &str) -> Option<mach_port_t> {
    let name = CString::new(service).ok()?;
    unsafe {
        let task = mach_task_self();
        let mut bs: mach_port_t = MACH_PORT_NULL;
        if task_get_special_port(task, TASK_BOOTSTRAP_PORT, &mut bs) != KERN_SUCCESS {
            return None;
        }
        let mut port: mach_port_t = MACH_PORT_NULL;
        if bootstrap_look_up(bs, name.as_ptr(), &mut port) != KERN_SUCCESS || port == MACH_PORT_NULL
        {
            return None;
        }
        Some(port)
    }
}

/// Sends `payload` and waits for the response. `None` if the service is not
/// registered or messaging failed (the caller then falls back to the socket).
pub fn send(service: &str, payload: &[u8]) -> Option<String> {
    let port = lookup(service)?;
    unsafe {
        let task = mach_task_self();
        let mut response_port: mach_port_t = MACH_PORT_NULL;
        if mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut response_port) != KERN_SUCCESS {
            return None;
        }
        if mach_port_insert_right(task, response_port, response_port, MACH_MSG_TYPE_MAKE_SEND)
            != KERN_SUCCESS
        {
            return None;
        }

        let mut msg: MachMessage = std::mem::zeroed();
        msg.header.msgh_remote_port = port;
        msg.header.msgh_local_port = response_port;
        msg.header.msgh_id = response_port as i32;
        msg.header.msgh_bits =
            MACH_MSG_TYPE_COPY_SEND | (MACH_MSG_TYPE_MAKE_SEND << 8) | MACH_MSGH_BITS_COMPLEX;
        msg.header.msgh_size = std::mem::size_of::<MachMessage>() as mach_msg_size_t;
        msg.descriptor_count = 1;
        msg.descriptor = mach_msg_ool_descriptor_t::new(
            payload.as_ptr() as *mut _,
            false,
            MACH_MSG_VIRTUAL_COPY,
            payload.len() as mach_msg_size_t,
        );

        let kr = mach_msg(
            &mut msg.header,
            MACH_SEND_MSG,
            std::mem::size_of::<MachMessage>() as mach_msg_size_t,
            0,
            MACH_PORT_NULL,
            MACH_MSG_TIMEOUT_NONE,
            MACH_PORT_NULL,
        );
        if kr != MACH_MSG_SUCCESS {
            cleanup(task, response_port);
            return None;
        }

        let mut buffer: MachBuffer = std::mem::zeroed();
        let kr = mach_msg(
            &mut buffer.message.header,
            MACH_RCV_MSG | MACH_RCV_TIMEOUT,
            0,
            std::mem::size_of::<MachBuffer>() as mach_msg_size_t,
            response_port,
            RESPONSE_TIMEOUT_MS,
            MACH_PORT_NULL,
        );
        // Copy out of the packed struct before use (no references to packed fields).
        let descriptor = buffer.message.descriptor;
        let rsp = if kr == MACH_MSG_SUCCESS && !descriptor.address.is_null() {
            let ptr = descriptor.address as *const u8;
            let len = descriptor.size as usize;
            let bytes = std::slice::from_raw_parts(ptr, len);
            let end = bytes.iter().position(|b| *b == 0).unwrap_or(len);
            let s = String::from_utf8_lossy(&bytes[..end]).into_owned();
            mach_msg_destroy(&mut buffer.message.header);
            s
        } else {
            String::new()
        };
        cleanup(task, response_port);
        Some(rsp)
    }
}

unsafe fn cleanup(task: mach_port_t, response_port: mach_port_t) {
    mach_port_mod_refs(task, response_port, MACH_PORT_RIGHT_RECEIVE, -1);
    mach_port_deallocate(task, response_port);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_apple_headers() {
        assert_eq!(std::mem::size_of::<MachMessage>(), 44);
        assert_eq!(std::mem::offset_of!(MachMessage, descriptor), 28);
    }
}
