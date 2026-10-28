//! `ldr:ro`: loads CRO modules (ADR-0007). Commands per 3dbrew "RO
//! Services", added when the game first sends them.

use super::Message;

pub enum Request {
    /// Initialize (0x0001, 3dbrew "RO:Initialize"): the CRS in the
    /// process's memory, its size, where to map it; word 4 must be 0,
    /// then the process handle.
    Initialize {
        buffer: u32,
        size: u32,
        address: u32,
    },
    /// LoadCRR (0x0002, 3dbrew "RO:LoadCRR"): the CRR (module hashes) in
    /// the process's memory.
    LoadCrr {
        buffer: u32,
        size: u32,
    },
    /// LoadCRO_New (0x0009, 3dbrew "RO:LoadExeCRO"): the module in the
    /// process's memory, where to map it, its size, the .data and .bss
    /// buffers and sizes, auto-link, fix level.
    LoadCro(LoadCro),
    /// UnloadCRO (0x0005): the module's mapped address, 0, its buffer.
    UnloadCro {
        address: u32,
        buffer: u32,
    },
    Other(u32),
}

/// A LoadCRO_New request: the module in the process's memory, where to
/// map it, its size, the .data and .bss buffers (address, size),
/// auto-link, fix level.
#[derive(Debug, Clone, Copy)]
pub struct LoadCro {
    pub buffer: u32,
    pub address: u32,
    pub size: u32,
    pub data: (u32, u32),
    pub bss: (u32, u32),
    pub auto_link: bool,
    pub fix_level: u32,
}

pub fn parse(message: &Message) -> Request {
    let w = &message.words;
    match w[0] {
        0x0001_00C2 if w[4] == 0 && w[5] == crate::kernel::CURRENT_PROCESS => {
            Request::Initialize { buffer: w[1], size: w[2], address: w[3] }
        }
        0x0002_0082 if w[3] == 0 && w[4] == crate::kernel::CURRENT_PROCESS => Request::LoadCrr { buffer: w[1], size: w[2] },
        0x0009_02C2 if w[12] == 0 && w[13] == crate::kernel::CURRENT_PROCESS => Request::LoadCro(LoadCro {
            buffer: w[1],
            address: w[2],
            size: w[3],
            data: (w[4], w[6]),
            bss: (w[7], w[8]),
            auto_link: w[9] != 0,
            fix_level: w[10],
        }),
        0x0005_00C2 if w[4] == 0 && w[5] == crate::kernel::CURRENT_PROCESS => Request::UnloadCro { address: w[1], buffer: w[3] },
        other => Request::Other(other),
    }
}

/// The mapped address RO derives from a buffer address (3dbrew "RO
/// Services"): below 0x08000000 unchanged, else shifted so that
/// 0x08000000 lands at the end of the executable's aligned segments.
pub fn mapped_address(buffer: u32, executable_end: u32) -> u32 {
    if buffer < 0x0800_0000 {
        buffer
    } else {
        buffer - (0x0800_0000 - executable_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ultra_suns_crs_maps_where_the_game_asks() {
        // the executable ends at 0x006D4000 (its CRS's last segment starts there)
        assert_eq!(mapped_address(0x081D_E000, 0x006D_4000), 0x008B_2000);
        assert_eq!(mapped_address(0x0050_0000, 0x006D_4000), 0x0050_0000);
    }
}
