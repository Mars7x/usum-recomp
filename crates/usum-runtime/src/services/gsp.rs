//! `gsp::Gpu`: the GPU service (3dbrew "GSP Services"). Commands are
//! added when the game first sends them.

use super::Message;

pub enum Request {
    /// AcquireRight (0x0016): flags, then the process handle. Result only.
    AcquireRight {
        flags: u32,
    },
    /// RegisterInterruptRelayQueue (0x0013): flags, then the event GSP
    /// signals when interrupts are queued.
    RegisterInterruptRelayQueue {
        flags: u32,
        event: u32,
    },
    Other(u32),
}

pub fn parse(message: &Message) -> Request {
    let w = &message.words;
    match w[0] {
        0x0016_0042 if w[2] == super::srv::COPY_HANDLE && w[3] == crate::kernel::CURRENT_PROCESS => {
            Request::AcquireRight { flags: w[1] }
        }
        0x0013_0042 if w[2] == super::srv::COPY_HANDLE => Request::RegisterInterruptRelayQueue { flags: w[1], event: w[3] },
        other => Request::Other(other),
    }
}

/// The GSP shared memory block's size. 3dbrew gives the layout, not the
/// size; Ultra Sun maps it with 0x1000 (its code after the registration,
/// 0x0027A0E0), and the layout's last field (command queues at 0x800 +
/// thread × 0x200) fits one page for thread 0 to 3.
pub const SHARED_MEMORY_SIZE: u32 = 0x1000;

/// 3dbrew "GSPGPU:RegisterInterruptRelayQueue" response (header
/// 0x00130082): result, GSP thread index, a copy descriptor, the shared
/// memory. One client, so index 0.
pub fn register_reply(index: u32, shared_memory: u32) -> Vec<u32> {
    vec![super::header(0x13, 2, 2), 0, index, super::srv::COPY_HANDLE, shared_memory]
}

/// GX command IDs (3dbrew "GSP Shared Memory").
pub const REQUEST_DMA: u32 = 0;
pub const PROCESS_COMMAND_LIST: u32 = 1;
pub const MEMORY_FILL: u32 = 2;
pub const DISPLAY_TRANSFER: u32 = 3;
pub const TEXTURE_COPY: u32 = 4;
pub const FLUSH_CACHE_REGIONS: u32 = 5;

/// Interrupt IDs (3dbrew "GSP Shared Memory").
pub const INTERRUPT_PSC0: u8 = 0;
pub const INTERRUPT_PDC0: u8 = 2;
pub const INTERRUPT_PPF: u8 = 4;
pub const INTERRUPT_P3D: u8 = 5;
pub const INTERRUPT_DMA: u8 = 6;

/// Offsets in the shared memory block for GSP thread `index`.
pub fn interrupt_queue(index: u32) -> u32 {
    index * 0x40
}
pub fn command_queue(index: u32) -> u32 {
    0x800 + index * 0x200
}
/// Command entries a queue holds.
pub const QUEUE_ENTRIES: u32 = 15;
/// Interrupt list entries (3dbrew: count at most 0x34 for non-PDC).
pub const INTERRUPT_ENTRIES: u32 = 0x34;

/// The bytes of a MemoryFill buffer (3dbrew "GPU/External Registers":
/// control bit 0 starts it, bits 8-9 the width: 0 16-bit, 1 or 3 24-bit,
/// 2 32-bit). `end` is exclusive.
pub fn fill(start: u32, value: u32, end: u32, control: u32) -> Result<Vec<u8>, String> {
    let len = end.checked_sub(start).filter(|&n| n <= 0x0100_0000).ok_or_else(|| format!("MemoryFill {start:#x}..{end:#x}"))?;
    let pattern: Vec<u8> = match (control >> 8) & 3 {
        0 => value.to_le_bytes()[..2].to_vec(),
        2 => value.to_le_bytes().to_vec(),
        _ => value.to_le_bytes()[..3].to_vec(),
    };
    let mut bytes = pattern.repeat((len as usize).div_ceil(pattern.len()));
    bytes.truncate(len as usize);
    Ok(bytes)
}

/// `size` bytes read from `at` in lines of `width` bytes, skipping `gap`
/// bytes after each (width 0: one contiguous run).
pub fn gather(mem: &usum_guest::Memory, at: u32, size: u32, width: u32, gap: u32) -> Result<Vec<u8>, usum_guest::Fault> {
    if width == 0 || gap == 0 {
        return mem.slice(at, size).map(<[u8]>::to_vec).map_or_else(|| mem.read_range(at, size), Ok);
    }
    let mut out = Vec::with_capacity(size as usize);
    let mut from = at;
    while (out.len() as u32) < size {
        let n = width.min(size - out.len() as u32);
        out.extend(mem.read_range(from, n)?);
        from += width + gap;
    }
    Ok(out)
}

/// Writes `bytes` from `at` in lines of `width`, skipping `gap` after each
/// (a device write: VRAM is read-only to the process).
pub fn scatter(mem: &mut usum_guest::Memory, at: u32, bytes: &[u8], width: u32, gap: u32) -> Result<(), usum_guest::Fault> {
    if width == 0 || gap == 0 {
        return mem.device_write(at, bytes);
    }
    let mut to = at;
    for line in bytes.chunks(width as usize) {
        mem.device_write(to, line)?;
        to += width + gap;
    }
    Ok(())
}
