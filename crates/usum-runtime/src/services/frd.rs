//! `frd:u`: the friend service (3dbrew "Friend Services"). The emulated
//! console is offline and has no friends: a choice, as no network
//! service is emulated. Commands are added when the game first sends
//! them.

/// MyPresence (3dbrew "FRDU:GetMyPresence"): a 0x2C-byte Presence and a
/// 0x100-byte game mode description.
pub const MY_PRESENCE_SIZE: u32 = 0x12C;

/// A reply carrying `size` bytes in static buffer 0 at `buffer`.
pub fn static_reply(command: u16, size: u32, buffer: u32) -> Vec<u32> {
    vec![super::header(command, 1, 2), 0, (size << 14) | 2, buffer]
}
