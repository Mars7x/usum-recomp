//! The address space a 3DS application starts with, built from the dump.
//!
//! Layout per 3dbrew ("Memory layout", application section): code segments
//! from the exheader at their addresses (text and rodata read-only, data
//! and bss read-write), the main thread's stack ending at 0x10000000, and
//! a thread-local storage block. Everything else is unmapped, so any
//! access to it (heap, shared pages, I/O) is a reported fault rather than
//! a silent zero. Facts not established yet are named as such below.

use usum_guest::{Cpu, Memory, Perm};
use usum_import::Inspection;

const PAGE: u32 = 0x1000;
/// The main thread's stack ends here (3dbrew, "Memory layout").
pub const STACK_TOP: u32 = 0x1000_0000;
/// The first thread's TLS block (3dbrew "Memory layout": TLS starts at
/// 0x1FF82000, 0x200 bytes per thread).
pub const TLS: u32 = 0x1FF8_2000;
/// Bytes of TLS per thread, and the threads this runtime gives TLS to.
pub const TLS_SLOT: u32 = 0x200;
pub const TLS_SLOTS: u32 = 32;
/// Configuration memory and the shared page, read-only to applications
/// (3dbrew "Configuration Memory").
pub const CONFIG_PAGE: u32 = 0x1FF8_0000;
pub const SHARED_PAGE: u32 = 0x1FF8_1000;
const STATE_SHARED: u32 = 6;

/// The console this runtime presents: an Old 3DS retail unit. The kernel
/// version is a chosen value: it must not be older than the 2.50 the
/// exheader declares, and which exact firmware to present is not settled.
const KERNEL_VERSION: (u8, u8, u8) = (2, 58, 0);
const ENVINFO_RETAIL: u8 = 1;
const UNITINFO_PROD: u8 = 0;
const RUNNING_HW_PRODUCT: u8 = 1;

/// APPLICATION/SYSTEM/BASE region sizes for an Old 3DS APPMEMTYPE (3dbrew
/// "Memory layout"); types other than 2-5 use type 0's.
fn region_sizes(appmemtype: u8) -> (u32, u32, u32) {
    match appmemtype {
        2 => (0x0600_0000, 0x00C0_0000, 0x0140_0000),
        3 => (0x0500_0000, 0x01C0_0000, 0x0140_0000),
        4 => (0x0480_0000, 0x0240_0000, 0x0140_0000),
        5 => (0x0200_0000, 0x04C0_0000, 0x0140_0000),
        _ => (0x0400_0000, 0x02C0_0000, 0x0140_0000),
    }
}

/// Configuration memory for an Old 3DS running a title that requests
/// `system_mode` (which selects APPMEMTYPE).
fn config_page(system_mode: u8) -> Vec<u8> {
    let mut page = vec![0u8; PAGE as usize];
    let (major, minor, revision) = KERNEL_VERSION;
    page[0x01] = revision;
    page[0x02] = minor;
    page[0x03] = major;
    page[0x14] = ENVINFO_RETAIL;
    page[0x15] = UNITINFO_PROD;
    let appmemtype = appmemtype(system_mode);
    let (app, sys, base) = region_sizes(appmemtype);
    page[0x30..0x34].copy_from_slice(&u32::from(appmemtype).to_le_bytes());
    page[0x40..0x44].copy_from_slice(&app.to_le_bytes());
    page[0x44..0x48].copy_from_slice(&sys.to_le_bytes());
    page[0x48..0x4C].copy_from_slice(&base.to_le_bytes());
    // the FIRM_* fields default to the KERNEL_* ones
    page[0x61] = revision;
    page[0x62] = minor;
    page[0x63] = major;
    page
}

/// The shared page; time, battery and network fields are left zero until
/// the services that maintain them exist.
fn shared_page() -> Vec<u8> {
    let mut page = vec![0u8; PAGE as usize];
    page[0x04] = RUNNING_HW_PRODUCT;
    page
}
const TLS_SIZE: u32 = TLS_SLOT * TLS_SLOTS;

/// 3dbrew's MemoryState values (the ones used here).
pub const STATE_FREE: u32 = 0;
pub const STATE_IO: u32 = 2;
pub const STATE_STATIC: u32 = 3;
pub const STATE_CODE: u32 = 4;
pub const STATE_PRIVATE: u32 = 5;
/// MemoryPermission bits.
pub const PERM_R: u32 = 1;
pub const PERM_W: u32 = 2;
pub const PERM_X: u32 = 4;

/// Per-process facts the kernel reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Process {
    /// Virtual base of the LINEAR (FCRAM) mapping: 0x30000000 for
    /// processes whose exheader declares kernel version 2.44 or later,
    /// 0x14000000 before (3dbrew "Memory layout").
    pub linear_base: u32,
    /// The APPLICATION memory region size (APPMEMALLOC), which is the
    /// process's commit limit.
    pub app_region: u32,
    /// The main thread's priority (exheader ACI).
    pub main_priority: u32,
    /// The end of the executable's segments (data and bss, page-aligned):
    /// RO maps module buffers relative to it.
    pub executable_end: u32,
    /// The exheader's save data size: the formatted save archive's size.
    pub save_data_size: u64,
}

impl Process {
    pub fn from_exheader(e: &usum_import::exheader::ExHeader) -> Process {
        let new_layout = e.kernel_version.is_some_and(|v| v >= (2, 44));
        Process {
            linear_base: if new_layout { 0x3000_0000 } else { 0x1400_0000 },
            app_region: region_sizes(appmemtype(e.old3ds_system_mode)).0,
            main_priority: u32::from(e.priority),
            executable_end: e.data.address + round_up(e.data.size + e.bss_size),
            save_data_size: e.save_data_size,
        }
    }
}

/// APPMEMTYPE from the requested Old 3DS system mode: types 2-5 as asked,
/// anything else the retail type 0.
fn appmemtype(system_mode: u8) -> u8 {
    if (2..=5).contains(&system_mode) {
        system_mode
    } else {
        0
    }
}

/// A block of the address space as the kernel describes it (QueryMemory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub base: u32,
    pub size: u32,
    pub perm: u32,
    pub state: u32,
}

#[derive(Debug)]
pub enum BootError {
    Layout(&'static str),
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BootError::Layout(why) => write!(f, "cannot lay out the address space: {why}"),
        }
    }
}

fn round_up(n: u32) -> u32 {
    n.div_ceil(PAGE) * PAGE
}

/// The application's memory, its CPU state at the entry point, and the
/// blocks the kernel reports for QueryMemory. Block states: text and
/// rodata are Code, data/bss and the stack Private (3dbrew "Memory
/// Management"); the TLS block's state is not established and is reported
/// as Private.
pub fn boot(inspection: &Inspection) -> Result<(Cpu, Memory, Vec<Block>), BootError> {
    let e = &inspection.exheader;
    let code = &inspection.code.bytes;
    let text_len = (e.text.pages * PAGE) as usize;
    let rodata_len = (e.rodata.pages * PAGE) as usize;
    let data_start = text_len + rodata_len;
    if code.len() < data_start {
        return Err(BootError::Layout("code is shorter than its text and rodata pages"));
    }
    let mut mem = Memory::new();
    let map = |mem: &mut Memory, base: u32, bytes: Vec<u8>, perm: Perm| {
        mem.map(base, bytes, perm).map_err(|_| BootError::Layout("segments overlap"))
    };
    map(&mut mem, e.text.address, code[..text_len].to_vec(), Perm::R)?;
    map(&mut mem, e.rodata.address, code[text_len..data_start].to_vec(), Perm::R)?;
    // data, then bss (zero) in the pages after it
    let mut data = code[data_start..].to_vec();
    let data_bytes = round_up(e.data.size.checked_add(e.bss_size).ok_or(BootError::Layout("data and bss overflow"))?);
    data.resize(data_bytes.max(e.data.pages * PAGE) as usize, 0);
    map(&mut mem, e.data.address, data, Perm::RW)?;
    if e.stack_size == 0 {
        return Err(BootError::Layout("the exheader declares no stack"));
    }
    let stack = round_up(e.stack_size);
    map(&mut mem, STACK_TOP - stack, vec![0; stack as usize], Perm::RW)?;
    map(&mut mem, TLS, vec![0; TLS_SIZE as usize], Perm::RW)?;
    map(&mut mem, CONFIG_PAGE, config_page(e.old3ds_system_mode), Perm::R)?;
    map(&mut mem, SHARED_PAGE, shared_page(), Perm::R)?;
    // what the exheader's kernel capabilities map: for these games the
    // DSP's shared memory (0x1FF50000, 0x1FF70000) and VRAM, read-only.
    // Zeroed here; the services that own them (DSP, GSP) give them meaning.
    let mut mapped = Vec::new();
    for m in e.mappings() {
        let size =
            m.end.checked_sub(m.start).filter(|&n| n > 0 && n % PAGE == 0).ok_or(BootError::Layout("bad mapping range"))?;
        map(&mut mem, m.start, vec![0; size as usize], if m.read_only { Perm::R } else { Perm::RW })?;
        let perm = if m.read_only { PERM_R } else { PERM_R | PERM_W };
        mapped.push(Block { base: m.start, size, perm, state: if m.cacheable { STATE_STATIC } else { STATE_IO } });
    }

    let blocks = vec![
        Block { base: e.text.address, size: text_len as u32, perm: PERM_R | PERM_X, state: STATE_CODE },
        Block { base: e.rodata.address, size: rodata_len as u32, perm: PERM_R, state: STATE_CODE },
        Block { base: e.data.address, size: data_bytes.max(e.data.pages * PAGE), perm: PERM_R | PERM_W, state: STATE_PRIVATE },
        Block { base: STACK_TOP - stack, size: stack, perm: PERM_R | PERM_W, state: STATE_PRIVATE },
        Block { base: TLS, size: TLS_SIZE, perm: PERM_R | PERM_W, state: STATE_PRIVATE },
        Block { base: CONFIG_PAGE, size: 2 * PAGE, perm: PERM_R, state: STATE_SHARED },
    ];
    let mut blocks = blocks;
    blocks.extend(mapped);
    blocks.sort_by_key(|b| b.base);
    let mut cpu = Cpu::default();
    cpu.r[15] = e.text.address;
    cpu.r[13] = STACK_TOP;
    cpu.tls = TLS;
    // The initial FPSCR is not established (RESEARCH.md R-017); 0 is
    // IEEE round-to-nearest without flush-to-zero.
    cpu.fpscr = 0;
    Ok((cpu, mem, blocks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use usum_guest::Fault;
    use usum_import::fixture::{self, Spec};

    #[test]
    fn lays_out_segments_stack_and_tls() {
        let path = std::env::temp_dir().join(format!("usum-runtime-boot-{}.3ds", std::process::id()));
        std::fs::write(&path, fixture::build(&Spec::default())).unwrap();
        let inspection = usum_import::inspect(&path).unwrap();
        let (cpu, mut mem, blocks) = boot(&inspection).unwrap();
        assert_eq!(blocks.len(), 6);
        // configuration memory: APPMEMALLOC for the fixture's system mode 0
        assert_eq!(mem.read_u32(CONFIG_PAGE + 0x40).unwrap(), 0x0400_0000);
        assert!(mem.write_u8(CONFIG_PAGE, 0).is_err(), "read-only");
        let e = &inspection.exheader;
        assert_eq!((cpu.r[15], cpu.r[13], cpu.tls), (e.text.address, STACK_TOP, TLS));
        // text is the code's first bytes and read-only
        assert_eq!(mem.read_u8(e.text.address).unwrap(), inspection.code.bytes[0]);
        assert_eq!(mem.write_u8(e.text.address, 0), Err(Fault::ReadOnly { address: e.text.address }));
        assert!(mem.write_u8(e.rodata.address, 0).is_err());
        // data and bss, the stack and TLS are writable
        assert!(mem.write_u32(e.data.address, 1).is_ok());
        assert!(mem.write_u32(STACK_TOP - 4, 1).is_ok());
        assert!(mem.write_u32(TLS + 0x80, 1).is_ok(), "IPC command buffer area");
        // nothing else is mapped: the heap, for one
        assert!(mem.read_u8(0x0800_0000).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
