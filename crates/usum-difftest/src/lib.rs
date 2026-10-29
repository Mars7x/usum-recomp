//! Runs recompiled test programs the way QEMU's Linux user mode runs the
//! originals, so their outputs can be compared byte for byte.
//!
//! The "host" here implements the two Linux syscalls the harness uses
//! (`write` and `exit`), nothing else. It is test scaffolding, not the
//! game runtime.

use usum_guest::{run, Code, Control, Cpu, Fault, Memory, Perm, Stopped};

pub struct Program {
    pub name: &'static str,
    pub image: &'static [u8],
    /// stdout of the original under `qemu-arm -cpu arm11mpcore`.
    pub expected: &'static [u8],
    pub entries: &'static [(u32, Code)],
    /// Where execution starts: the harness's `_start`.
    pub entry: u32,
    /// The function table, for the program translated in split mode.
    pub table: Option<&'static [Code]>,
    pub functions: usize,
    /// Functions found only through pointer seeds.
    pub from_pointers: usize,
    pub instructions: usize,
    /// Words per output record for generated random programs, 0 otherwise.
    pub record: usize,
    /// The instruction text of each random case, one per line.
    pub cases: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

/// Where link.ld puts the image.
pub const BASE: u32 = 0x0010_0000;
/// Linux EABI syscall numbers (r7).
const SYS_EXIT: u32 = 1;
const SYS_WRITE: u32 = 4;
const SYS_EXIT_GROUP: u32 = 248;
/// ARM-private `set_tls` (__ARM_NR_set_tls), which sets TPIDRURO.
const SYS_SET_TLS: u32 = 0x000F_0005;
/// Room after the image, as the reference's address space has some too.
const SLACK: usize = 0x1000;

#[derive(Debug)]
pub enum Failure {
    Fault(Fault, u32),
    Hang,
    UnknownSyscall(u32),
}

/// Runs `program` from its entry and returns what it wrote to stdout.
pub fn execute(program: &Program) -> Result<Vec<u8>, Failure> {
    execute_with_indexed_layout(program, false)
}

/// Also exercise scalar accesses through the full-page region index. The
/// extra mappings are disjoint from the linked image and its stack; the guest
/// programs cannot observe them through the harness's write/exit syscalls.
pub fn execute_with_indexed_layout(program: &Program, indexed: bool) -> Result<Vec<u8>, Failure> {
    if let Some(table) = program.table {
        // only one program uses the table; installing it twice is harmless
        usum_guest::install_functions(table);
    }
    let mut mem = Memory::new();
    let mut bytes = program.image.to_vec();
    bytes.resize(bytes.len().next_multiple_of(0x1000) + SLACK, 0);
    mem.map(BASE, bytes, Perm::RW).expect("one region cannot overlap");
    if indexed {
        for i in 0..7 {
            mem.map(0x1000 + i * 0x2000, vec![0; 0x1000], Perm::RW).expect("disjoint index fixtures");
        }
    }
    let mut cpu = Cpu::default();
    cpu.r[15] = program.entry;
    let mut stdout = Vec::new();
    let mut unknown = None;
    // real game functions may loop for long on random inputs (a counting
    // loop up to an input value): they get 64x the synthetic programs' budget
    let refills = if program.name.starts_with("game_") { 1 << 18 } else { 1 << 12 };
    let result = run(&mut cpu, &mut mem, program.entries, 1 << 20, refills, |cpu, mem, _number| match cpu.r[7] {
        SYS_WRITE => {
            stdout.extend(mem.read_range(cpu.r[1], cpu.r[2])?);
            cpu.r[0] = cpu.r[2];
            Ok(Control::Continue)
        }
        SYS_SET_TLS => {
            cpu.tls = cpu.r[0];
            cpu.r[0] = 0;
            Ok(Control::Continue)
        }
        SYS_EXIT | SYS_EXIT_GROUP => Ok(Control::Stop),
        other => {
            unknown = Some(other);
            Ok(Control::Stop)
        }
    });
    match result {
        Ok(Stopped::Svc) => match unknown {
            Some(n) => Err(Failure::UnknownSyscall(n)),
            None => Ok(stdout),
        },
        Ok(Stopped::OutOfBudget) => Err(Failure::Hang),
        Err(fault) => Err(Failure::Fault(fault, cpu.r[15])),
    }
}
