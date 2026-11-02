//! Native dynamic FPS (ADR-0008), experimental and off by default.
//!
//! With `USUM_NATIVE_FPS=1` the engine is asked for its own higher update
//! rate, and each engine update publishes its elapsed guest time to the
//! translator's patch points (`usum_guest::native`), which scale the
//! per-update quantities that would otherwise run too fast. Only the
//! corrections that have replay evidence are in the patch table; scenes
//! they do not cover still run at the wrong speed.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use usum_guest::{Code, Cpu, Exit, Fault, Memory};

use crate::native_timing::UpdateClock;

/// Where the native cadence hooks sit in one version of the game.
struct Profile {
    program_id: u64,
    /// Engine update start (r0 = engine manager) and its first word.
    update_start: (u32, u32),
    /// Instructions in the update start's first block: the step is published
    /// only when the original can enter it, so a budget retry does not
    /// publish a second, zero-length step.
    update_cost: i64,
    /// Global holding the engine manager: +8 pending update rate (2 = none),
    /// +9 current rate (1 = normal, 0 = higher).
    manager: u32,
}

const ULTRA_SUN: Profile = Profile {
    program_id: 0x0004_0000_001B_5000,
    update_start: (0x0040_1438, 0xe92d_4070),
    update_cost: 15,
    manager: 0x006a_3984,
};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static UPDATE_ORIGINAL: OnceLock<Code> = OnceLock::new();

thread_local! {
    static CLOCK: RefCell<UpdateClock> = const { RefCell::new(UpdateClock::new()) };
}

/// Whether `USUM_NATIVE_FPS=1` asks for native cadence.
pub fn requested() -> bool {
    std::env::var("USUM_NATIVE_FPS").as_deref() == Ok("1")
}

/// The function and entry tables with the update-start hook installed.
/// Returns the tables unchanged if the entry is not there; call [`activate`]
/// once the program ID is known.
pub fn install(functions: &'static [Code], entries: &'static [(u32, Code)]) -> (&'static [Code], &'static [(u32, Code)]) {
    let address = ULTRA_SUN.update_start.0;
    let Ok(index) = entries.binary_search_by_key(&address, |e| e.0) else { return (functions, entries) };
    let original = entries[index].1;
    if UPDATE_ORIGINAL.set(original).is_err() && !UPDATE_ORIGINAL.get().is_some_and(|o| std::ptr::fn_addr_eq(*o, original)) {
        return (functions, entries);
    }
    let mut entries = entries.to_vec();
    entries[index].1 = update_start;
    let functions: Vec<Code> =
        functions.iter().map(|&f| if std::ptr::fn_addr_eq(f, original) { update_start as Code } else { f }).collect();
    (Box::leak(functions.into_boxed_slice()), Box::leak(entries.into_boxed_slice()))
}

/// Enables step publication for a supported program. Returns whether it did.
pub fn activate(program_id: u64) -> bool {
    let supported = program_id == ULTRA_SUN.program_id && UPDATE_ORIGINAL.get().is_some();
    ACTIVE.store(supported, Ordering::Relaxed);
    supported
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Keeps requesting the engine's higher update rate (call between
/// dispatches). The update-start code is checked before the manager
/// pointer is trusted.
pub fn request_rate(mem: &mut Memory) {
    if mem.read_u32(ULTRA_SUN.update_start.0).ok() != Some(ULTRA_SUN.update_start.1) {
        return;
    }
    let Ok(manager) = mem.read_u32(ULTRA_SUN.manager) else { return };
    if manager != 0 && mem.read_u8(manager + 9).ok() == Some(1) && mem.read_u8(manager + 8).ok() == Some(2) {
        let _ = mem.write_u8(manager + 8, 0);
    }
}

fn update_start(cpu: &mut Cpu, mem: &mut Memory) -> Result<Exit, Fault> {
    let original = *UPDATE_ORIGINAL.get().expect("installed with its original");
    if active() && cpu.r[15] == ULTRA_SUN.update_start.0 && cpu.budget >= ULTRA_SUN.update_cost {
        let higher = mem.read_u8(cpu.r[0].wrapping_add(9)).ok() == Some(0);
        let step = CLOCK.with(|clock| {
            let mut clock = clock.borrow_mut();
            if !higher {
                clock.clear();
                return None;
            }
            crate::native_timing::guest_time_ns(cpu.budget).and_then(|now| clock.begin_update(now))
        });
        usum_guest::native::publish(
            step.map(|step| usum_guest::native::Step { sequence: step.sequence(), elapsed_ns: step.elapsed_ns() }),
        );
    }
    original(cpu, mem)
}
