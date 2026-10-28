//! `srv:`, the service manager: the port every process connects to first,
//! to get sessions to the other services. Commands per 3dbrew "Services
//! API" and libctru's `srv.c` (read for the message formats only).

use super::{header, Message};

/// Result when a process asks for a service its exheader does not permit
/// (3dbrew: srv returns 0xD8E06406 "not authorized").
pub const RESULT_NOT_AUTHORIZED: u32 = 0xD8E0_6406;

/// A request to srv, as the kernel must act on it.
pub enum Request {
    /// RegisterClient (0x0001): reply success.
    RegisterClient,
    /// EnableNotification (0x0002): reply with a new semaphore handle.
    EnableNotification,
    /// GetServiceHandle (0x0005): reply with a session to this service.
    GetServiceHandle(String),
    Other(u32),
}

pub fn parse(message: &Message) -> Request {
    match message.words[0] {
        0x0001_0002 => Request::RegisterClient,
        0x0002_0000 => Request::EnableNotification,
        0x0005_0100 => {
            let bytes: Vec<u8> = message.words[1..3].iter().flat_map(|w| w.to_le_bytes()).collect();
            let len = (message.words[3] as usize).min(8);
            Request::GetServiceHandle(String::from_utf8_lossy(&bytes[..len]).into_owned())
        }
        other => Request::Other(other),
    }
}

pub fn register_client_reply() -> Vec<u32> {
    vec![header(0x1, 1, 0), 0]
}

/// Translate descriptors for one handle (libctru `ipc.h`).
pub const COPY_HANDLE: u32 = 0x0;
pub const MOVE_HANDLE: u32 = 0x10;

/// A reply carrying one handle in word 3 (where libctru reads it), with
/// its translate descriptor in word 2.
pub fn handle_reply(command: u16, descriptor: u32, handle: u32) -> Vec<u32> {
    vec![header(command, 1, 2), 0, descriptor, handle]
}

pub fn error_reply(command: u16, result: u32) -> Vec<u32> {
    vec![header(command, 1, 0), result]
}
