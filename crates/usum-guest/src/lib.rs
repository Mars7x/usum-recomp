//! The guest machine as recompiled code sees it.
//!
//! Generated code may use only what this crate exports: [`Cpu`] (registers
//! and flags), [`Memory`] (the 32-bit guest address space), the helpers in
//! [`ops`] (instruction semantics shared by all generated code, so each is
//! written and tested once) and the [`Exit`]/[`Fault`] protocol. Guest
//! addresses are plain `u32` values, never host pointers, and every guest
//! integer operation is explicitly wrapping or checked.

pub mod memory;
pub mod ops;
pub mod vfp;

use std::fmt;

pub use memory::{Memory, Perm};

/// ARM processor state visible to user-mode code (ARMv6K with VFPv2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cpu {
    /// r0–r15. `r[15]` holds the address of the instruction being entered
    /// or resumed at, not "address + 8"; reads of PC as an operand are
    /// resolved to constants at translation time.
    pub r: [u32; 16],
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub q: bool,
    /// APSR.GE[3:0].
    pub ge: u8,
    pub thumb: bool,
    /// How many recompiled calls deep the host stack is.
    pub depth: u32,
    /// Instructions left before the code must return to the host.
    pub budget: i64,
    /// TPIDRURO, read by `mrc p15, 0, rX, c13, c0, 3`; the host sets it to
    /// the thread's TLS address.
    pub tls: u32,
    /// s0–s31 as raw bits; dN is s(2N) (low word) and s(2N+1).
    pub s: [u32; 32],
    pub fpscr: u32,
    /// The address of the last indirect branch taken (BX, BLX register,
    /// loads and pops into PC, data-processing writes to PC). Diagnostics
    /// only: it names who jumped when a target has no translation.
    pub from: u32,
    /// The local exclusive monitor: the address an LDREX marked, if any.
    /// One core, so STREX succeeds exactly when this matches its address.
    pub exclusive: Option<u32>,
    /// For code translated from a CRO module: its load address minus the
    /// virtual base it was translated at (ADR-0007). The host sets it on
    /// every entry into module code; executable code ignores it.
    pub module_delta: u32,
}

/// Why recompiled code returned to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Execution continues at `cpu.r[15]` (`cpu.thumb` gives the state).
    /// Covers returns, indirect jumps, tail calls and unwinding.
    Jump,
    /// The guest executed `svc #n`; `cpu.r[15]` is the next instruction.
    Svc(u32),
    /// The budget ran out; `cpu.r[15]` is the block that did not run.
    Budget,
}

/// Something the guest did that the host must not paper over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Unmapped {
        address: u32,
        write: bool,
    },
    /// A host device could not supply its deferred memory result.
    Device {
        address: u32,
    },
    ReadOnly {
        address: u32,
    },
    Unaligned {
        address: u32,
    },
    /// An instruction the translator does not implement.
    Unsupported {
        address: u32,
        raw: u32,
    },
    /// A VFP operation in a mode not implemented yet (directed rounding in
    /// arithmetic, or an UNPREDICTABLE short-vector configuration).
    UnsupportedFp {
        address: u32,
        fpscr: u32,
    },
    /// Execution reached an address with no translation. With no
    /// interpreter, this is fatal by design (FEASIBILITY.md, R1).
    NoTranslation {
        address: u32,
        thumb: bool,
    },
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Fault::Unmapped { address, write } => {
                write!(f, "{} unmapped address {address:#010x}", if *write { "write to" } else { "read from" })
            }
            Fault::Device { address } => write!(f, "device read failed at {address:#010x}"),
            Fault::ReadOnly { address } => write!(f, "write to read-only address {address:#010x}"),
            Fault::Unaligned { address } => write!(f, "unaligned access at {address:#010x}"),
            Fault::Unsupported { address, raw } => write!(f, "unsupported instruction {raw:#010x} at {address:#010x}"),
            Fault::UnsupportedFp { address, fpscr } => {
                write!(f, "VFP operation at {address:#010x} in an unsupported mode (FPSCR {fpscr:#010x})")
            }
            Fault::NoTranslation { address, thumb } => {
                write!(f, "no translation for {} code at {address:#010x}", if *thumb { "Thumb" } else { "ARM" })
            }
        }
    }
}

impl std::error::Error for Fault {}

/// A recompiled function.
pub type Code = fn(&mut Cpu, &mut Memory) -> Result<Exit, Fault>;

/// Calls nested deeper than this unwind to the host, which resumes from
/// `r[15]`; every return address is an entry, so nothing is lost.
pub const DEPTH_LIMIT: u32 = 256;

/// Entry points of a translation, sorted by address. The address has bit
/// 0 set for Thumb code.
pub fn lookup(entries: &[(u32, Code)], address: u32) -> Option<Code> {
    entries.binary_search_by_key(&address, |&(a, _)| a).ok().map(|i| entries[i].1)
}

/// What the host's SVC handler tells the run loop.
pub enum Control {
    Continue,
    Stop,
}

/// Why [`run`] stopped without a fault.
#[derive(Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The SVC handler said so.
    Svc,
    /// `max_budget_refills` budgets were used up: likely a hang.
    OutOfBudget,
}

/// Runs translated code from `cpu.r[15]` until the SVC handler stops it,
/// a fault occurs, or the budget has been refilled too often.
pub fn run(
    cpu: &mut Cpu,
    mem: &mut Memory,
    entries: &[(u32, Code)],
    budget: i64,
    max_budget_refills: u32,
    mut on_svc: impl FnMut(&mut Cpu, &mut Memory, u32) -> Result<Control, Fault>,
) -> Result<Stopped, Fault> {
    let mut refills = 0;
    cpu.budget = budget;
    loop {
        let key = cpu.r[15] | u32::from(cpu.thumb);
        let code = lookup(entries, key).ok_or(Fault::NoTranslation { address: cpu.r[15], thumb: cpu.thumb })?;
        cpu.depth = 0;
        match code(cpu, mem)? {
            Exit::Jump => {}
            Exit::Svc(number) => {
                if let Control::Stop = on_svc(cpu, mem, number)? {
                    return Ok(Stopped::Svc);
                }
            }
            Exit::Budget => {
                refills += 1;
                if refills > max_budget_refills {
                    return Ok(Stopped::OutOfBudget);
                }
                cpu.budget = budget;
            }
        }
    }
}

/// Where execution goes after a direct call returns.
pub enum Flow {
    /// Continue in the calling function at this address (same state).
    Local(u32),
    /// Leave the calling function with this exit.
    Leave(Exit),
}

/// A direct call (BL / BLX immediate) from translated code: sets LR and the
/// target's state, enforces [`DEPTH_LIMIT`], runs the callee and decides
/// where the caller continues. One shared, tested helper keeps call sites
/// to a single line of generated code. Deliberately not `#[inline]`: one
/// out-of-line copy keeps every call site small for the compiler
/// (RESEARCH.md R-022).
/// A debugging watch on direct calls: when `call` targets `WATCH` (0:
/// none), the return address is stored in `WATCH_HIT` (the call site + 4
/// or + 2). Lets a probe name the caller of, say, a panic routine.
pub static WATCH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static WATCH_HIT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A debugging profile of calls: while `PROFILE` is set, `call` counts
/// each (target, return address, module delta) in `PROFILE_COUNTS`.
pub static PROFILE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    pub static PROFILE_COUNTS: std::cell::RefCell<std::collections::HashMap<(u32, u32, u32), u64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

pub fn call(
    cpu: &mut Cpu,
    mem: &mut Memory,
    lr: u32,
    target: u32,
    target_thumb: bool,
    caller_thumb: bool,
    code: Code,
) -> Result<Flow, Fault> {
    if target == WATCH.load(std::sync::atomic::Ordering::Relaxed) && target != 0 {
        WATCH_HIT.store(lr, std::sync::atomic::Ordering::Relaxed);
    }
    if PROFILE.load(std::sync::atomic::Ordering::Relaxed) {
        PROFILE_COUNTS.with(|p| *p.borrow_mut().entry((target, lr, cpu.module_delta)).or_insert(0) += 1);
    }
    cpu.r[14] = lr;
    cpu.thumb = target_thumb;
    cpu.r[15] = target;
    if cpu.depth >= DEPTH_LIMIT {
        return Ok(Flow::Leave(Exit::Jump));
    }
    cpu.depth += 1;
    let budget = cpu.budget;
    let exit = code(cpu, mem)?;
    cpu.depth -= 1;
    // Every block spends budget, so a callee that spent none ran nothing:
    // `code` has no label for `target`. Execution would still be correct
    // (the host finds the right code through ENTRIES), so only debug builds
    // check, to catch a wrong call target or function-table index.
    debug_assert!(
        !(exit == Exit::Jump && cpu.budget == budget && cpu.r[15] == target && cpu.thumb == target_thumb),
        "the code called for {target:#010x} has no entry there"
    );
    if exit != Exit::Jump || cpu.thumb != caller_thumb {
        return Ok(Flow::Leave(exit));
    }
    Ok(Flow::Local(cpu.r[15]))
}

/// Charges a block's instructions to the budget; true when the block must
/// not run because the budget is spent (the block is refunded).
#[inline]
pub fn spend(cpu: &mut Cpu, count: i64) -> bool {
    cpu.budget -= count;
    if cpu.budget < 0 {
        cpu.budget += count;
        return true;
    }
    false
}

/// The table translated functions call each other through when they are in
/// different crates. Crates of generated code cannot depend on each other
/// (calls are cyclic), so a call across crates names its callee by index;
/// the top crate, which depends on all of them, installs the table once at
/// startup ([`install_functions`]). Calls within a crate stay direct.
static FUNCTIONS: std::sync::OnceLock<&'static [Code]> = std::sync::OnceLock::new();

/// Installs the function table; returns false if one was installed already.
pub fn install_functions(table: &'static [Code]) -> bool {
    FUNCTIONS.set(table).is_ok()
}

/// The function at `index` in the installed table. Translation assigns the
/// indices, so a missing table or index is a build defect, not guest
/// behavior: it panics with a message saying so.
#[inline]
pub fn function(index: usize) -> Code {
    let table = FUNCTIONS.get().expect("translated code ran before the function table was installed");
    table[index]
}
