//! HLE system services, reached through IPC: the game writes a command
//! into its thread's command buffer (TLS + 0x80) and calls
//! SendSyncRequest on a session; the reply is written back in place.
//!
//! Message format (3dbrew "IPC"; libctru's `ipc.h`): word 0 is the header
//! `command << 16 | normal_words << 6 | translate_words`, then the normal
//! parameters, then translate parameters (handles, buffers). Replies start
//! with a header and the result code.

pub mod apt;
pub mod cfg;
pub mod dsp;
pub mod frd;
pub mod fs;
pub mod gsp;
pub mod hid;
pub mod ro;
pub mod settings;
pub mod srv;
pub mod uds;
pub mod y2r;

use usum_guest::{Fault, Memory};

/// Offset of the IPC command buffer in a thread's TLS.
pub const COMMAND_BUFFER: u32 = 0x80;
/// Words a command buffer holds (3dbrew: 0x100 bytes).
pub const COMMAND_WORDS: usize = 64;

pub fn header(command: u16, normal: u32, translate: u32) -> u32 {
    (u32::from(command) << 16) | ((normal & 0x3F) << 6) | (translate & 0x3F)
}

/// A reply with only a result code.
pub fn result_reply(command: u16) -> Vec<u32> {
    vec![header(command, 1, 0), 0]
}

/// One IPC message, read from and written back to guest memory.
pub struct Message {
    pub words: [u32; COMMAND_WORDS],
    /// The receive static buffer descriptors, right after the command
    /// buffer (TLS + 0x180): 16 pairs of `size << 14 | index << 10 | 2`
    /// and an address (3dbrew "IPC").
    pub statics: [u32; STATIC_WORDS],
}

pub const STATIC_WORDS: usize = 32;

impl Message {
    pub fn read(mem: &Memory, buffer: u32) -> Result<Message, Fault> {
        let mut words = [0u32; COMMAND_WORDS];
        for (i, w) in words.iter_mut().enumerate() {
            *w = mem.read_u32(buffer + 4 * i as u32)?;
        }
        let mut statics = [0u32; STATIC_WORDS];
        for (i, w) in statics.iter_mut().enumerate() {
            *w = mem.read_u32(buffer + 0x100 + 4 * i as u32)?;
        }
        Ok(Message { words, statics })
    }

    pub fn command(&self) -> u16 {
        (self.words[0] >> 16) as u16
    }

    /// Writes a reply: `words[0]` is its header.
    pub fn reply(mem: &mut Memory, buffer: u32, words: &[u32]) -> Result<(), Fault> {
        for (i, &w) in words.iter().enumerate() {
            mem.write_u32(buffer + 4 * i as u32, w)?;
        }
        Ok(())
    }
}

/// What a service did with a request.
pub enum Handled {
    /// Replied; the kernel may need to create objects first (see `srv`).
    Reply(Vec<u32>),
    /// The command is not implemented: execution stops with a report.
    Unimplemented,
}
