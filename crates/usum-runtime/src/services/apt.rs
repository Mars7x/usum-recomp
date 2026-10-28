//! APT (applet manager): `APT:A`, `APT:U` and `APT:S` share one command
//! set. Message formats per 3dbrew "NS and APT Services"; each command is
//! added when the game first sends it.

use super::{header, Message};

/// A request to APT, as the kernel must act on it.
pub enum Request {
    /// GetLockHandle (0x0001, word 1 = applet attributes): reply with the
    /// APT lock.
    GetLockHandle {
        flags: u32,
    },
    /// Initialize (0x0002, app ID, attributes): reply with the
    /// notification and resume events.
    Initialize {
        app_id: u32,
        attributes: u32,
    },
    /// ReceiveParameter (0x000D) / GlanceParameter (0x000E): the pending
    /// parameter for `app_id`, its data into the static buffer at
    /// `buffer` (from the receive descriptor at command buffer + 0x100).
    /// Receive clears it; Glance leaves it pending.
    Parameter {
        glance: bool,
        app_id: u32,
        size: u32,
        buffer: u32,
    },
    /// AppletUtility (0x004B): `utility` with `input_size` bytes at
    /// `input`; up to `output_size` bytes go to the static buffer at
    /// `output`.
    AppletUtility {
        utility: u32,
        input_size: u32,
        input: u32,
        output_size: u32,
        output: u32,
    },
    Other(u32),
}

pub fn parse(message: &Message) -> Request {
    match message.words[0] {
        0x0001_0040 => Request::GetLockHandle { flags: message.words[1] },
        0x0002_0080 => Request::Initialize { app_id: message.words[1], attributes: message.words[2] },
        h @ (0x000D_0080 | 0x000E_0080) => Request::Parameter {
            glance: h == 0x000E_0080,
            app_id: message.words[1],
            size: message.words[2],
            buffer: message.statics[1],
        },
        0x004B_00C2 => Request::AppletUtility {
            utility: message.words[1],
            input_size: message.words[2],
            output_size: message.words[3],
            input: message.words[5],
            output: message.statics[1],
        },
        other => Request::Other(other),
    }
}

/// 3dbrew "APT:GetLockHandle": result, AppletAttr, APT state (bit 0 power
/// button, bit 1 order to close), a copy-handle descriptor, the lock.
/// 3dbrew does not say what AppletAttr the reply carries; this returns the
/// attributes the request gave (a choice). State 0: neither event pending.
/// What the lock is, 3dbrew does not say either; it is a mutex here.
pub fn lock_handle_reply(flags: u32, lock: u32) -> Vec<u32> {
    vec![header(0x1, 3, 2), 0, flags, 0, super::srv::COPY_HANDLE, lock]
}

/// Translate descriptor for copying `n` handles (3dbrew "IPC": the count
/// minus one in bits 26 and up).
pub fn copy_handles(n: u32) -> u32 {
    (n - 1) << 26
}

/// 3dbrew "APT:Initialize": result, a descriptor (0x04000000: two copied
/// handles), the notification event, the resume event.
pub fn initialize_reply(notification: u32, resume: u32) -> Vec<u32> {
    vec![header(0x2, 1, 3), 0, copy_handles(2), notification, resume]
}

/// A message one applet sends another through NS (3dbrew: SendParameter;
/// the "Command" values are listed under "NS and APT Services").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub sender: u32,
    pub command: u32,
    pub data: Vec<u8>,
    /// A handle passed along, 0 for none.
    pub handle: u32,
}

pub const HOME_MENU: u32 = 0x101;
pub const COMMAND_WAKEUP: u32 = 1;

impl Parameter {
    /// What an application finds pending after Initialize, a HYPOTHESIS
    /// (RESEARCH.md R-027): 3dbrew does not say. The game, woken by the
    /// resume event, glances at a parameter and treats a failure as fatal;
    /// a wakeup from the Home Menu takes the path its code treats as
    /// "carry on" (any command but 2, 5, 8, 9).
    pub fn startup() -> Parameter {
        Parameter { sender: HOME_MENU, command: COMMAND_WAKEUP, data: Vec::new(), handle: 0 }
    }
}

/// 3dbrew "APT:GlanceParameter" (header 0x000E0104) and ReceiveParameter
/// (0x000D0104): result, sender, command, data size, a handle descriptor
/// and the handle, the static buffer descriptor and its address. 3dbrew
/// says the two differ in "the word before the output handle" without
/// giving it; 0 (copy) is used for both.
pub fn parameter_reply(glance: bool, p: &Parameter, buffer: u32) -> Vec<u32> {
    let command = if glance { 0xE } else { 0xD };
    let size = p.data.len() as u32;
    vec![header(command, 4, 4), 0, p.sender, p.command, size, super::srv::COPY_HANDLE, p.handle, (size << 14) | 2, buffer]
}

/// AppletUtility IDs (3dbrew "APT:AppletUtility").
pub const SLEEP_IF_SHELL_CLOSED: u32 = 4;
pub const UNLOCK_TRANSITION: u32 = 7;

/// 3dbrew "APT:AppletUtility": result and "Applet Result". 3dbrew does not
/// say what the applet result is for the void utilities; 0 is a choice.
pub fn applet_utility_reply(applet_result: u32) -> Vec<u32> {
    vec![header(0x4B, 2, 0), 0, applet_result]
}
