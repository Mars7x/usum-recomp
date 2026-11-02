//! Native cadence patch points (ADR-0008).
//!
//! While native cadence is active, the runtime publishes each engine
//! update's elapsed guest time at the update boundary. Translator patch
//! points call into this module to scale per-update quantities by that
//! interval. With no step published every hook leaves guest state as it
//! is, so normal cadence runs exactly as translated.

use std::cell::Cell;

use crate::{vfp, Cpu, Memory};

/// Guest time of one engine update at the game's normal rate: two
/// hardware frames of 16,713,680 ns.
pub const NORMAL_UPDATE_NS: u64 = 33_427_360;

/// One engine update's interval, as published by the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// Increases by one per update; consumers use it to count a step once.
    pub sequence: u64,
    pub elapsed_ns: u64,
}

impl Step {
    /// Elapsed time in normal-rate updates (0.5 at a steady doubled rate).
    pub fn logical(self) -> f64 {
        self.elapsed_ns as f64 / NORMAL_UPDATE_NS as f64
    }
}

thread_local! {
    static STEP: Cell<Option<Step>> = const { Cell::new(None) };
}

/// Publish the current update's step, or `None` outside native cadence.
pub fn publish(step: Option<Step>) {
    STEP.with(|current| current.set(step));
}

pub fn current() -> Option<Step> {
    STEP.with(Cell::get)
}

/// Patch point: multiply single-precision register `reg` by the update's
/// logical step, rounded as the guest's FPSCR would round it. Skipped when
/// no step is published or the FPSCR selects short vectors or a non-default
/// rounding mode, which the audited call sites never use.
pub fn scale_single(cpu: &mut Cpu, reg: usize) {
    let Some(step) = current() else { return };
    if cpu.fpscr & vfp::SLOW_BITS != 0 {
        return;
    }
    let factor = vfp::f64_to_f32(cpu, step.logical().to_bits());
    cpu.s[reg] = vfp::arith32(cpu, vfp::Arith::Mul, cpu.s[reg], factor);
}

/// Patch point at a call's return label: [`scale_single`], but only when
/// the call went to `callee` (still in callee-saved `callee_reg`) on behalf
/// of `caller` (the return address `sp_offset` bytes up the stack). For a
/// shared dispatcher that calls many getters, of which one feeds a
/// per-update consumer.
pub fn scale_result(cpu: &mut Cpu, mem: &Memory, reg: usize, callee_reg: usize, callee: u32, sp_offset: u32, caller: u32) {
    if cpu.r[callee_reg] == callee && mem.read_u32(cpu.r[13].wrapping_add(sp_offset)).ok() == Some(caller) {
        scale_single(cpu, reg);
    }
}

/// Patch point for exponential smoothing: a per-update blend factor in
/// (0, 1) becomes the factor with the same effect over the elapsed step
/// ([`crate::timing::blend_for_elapsed`]). Other values are left alone.
pub fn blend_single(cpu: &mut Cpu, reg: usize) {
    let Some(step) = current() else { return };
    if cpu.fpscr & vfp::SLOW_BITS != 0 {
        return;
    }
    let factor = f32::from_bits(cpu.s[reg]);
    if !(factor > 0.0 && factor < 1.0) {
        return;
    }
    if let Some(adjusted) = crate::timing::blend_for_elapsed(f64::from(factor), step.logical()) {
        cpu.s[reg] = vfp::f64_to_f32(cpu, adjusted.to_bits());
    }
}

struct CounterClock {
    sequence: u64,
    /// The value this clock last left in the counter; anything else means
    /// the game reset or replaced the counter.
    expected: u32,
    fraction: crate::timing::TickAccumulator,
}

thread_local! {
    static COUNTERS: std::cell::RefCell<std::collections::HashMap<u32, CounterClock>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Patch point after an integer counter's `±1` (register `reg`, counter at
/// `cpu.r[base_reg] + offset`): the counter instead moves by the number of
/// whole normal-rate updates elapsed, carrying the remainder per counter.
/// A second call in one update moves it by nothing. A counter whose value
/// differs from what this clock left starts a new clock: its first tick is
/// immediate if `first_immediate`, else accumulated. Down-counters stop at
/// zero (R-149–R-152, R-156, R-169).
pub fn counter(cpu: &mut Cpu, reg: usize, base_reg: usize, offset: u32, down: bool, first_immediate: bool) {
    let Some(step) = current() else { return };
    let before = if down { cpu.r[reg].wrapping_add(1) } else { cpu.r[reg].wrapping_sub(1) };
    let key = cpu.r[base_reg].wrapping_add(offset);
    let unit = std::num::NonZeroU32::new(NORMAL_UPDATE_NS as u32).expect("nonzero update");
    let ticks = COUNTERS.with(|clocks| {
        let mut clocks = clocks.borrow_mut();
        let mut fraction = crate::timing::TickAccumulator::new(unit);
        let ticks = match clocks.get(&key) {
            Some(clock) if clock.expected == before && clock.sequence == step.sequence => 0,
            Some(clock) if clock.expected == before && clock.sequence.checked_add(1) == Some(step.sequence) => {
                fraction = clock.fraction;
                fraction.advance(step.elapsed_ns)
            }
            _ if first_immediate => 1,
            _ => fraction.advance(step.elapsed_ns),
        };
        let ticks = u32::try_from(ticks).unwrap_or(u32::MAX);
        let ticks = if down { ticks.min(before) } else { ticks };
        let after = if down { before - ticks } else { before.wrapping_add(ticks) };
        clocks.insert(key, CounterClock { sequence: step.sequence, expected: after, fraction });
        ticks
    });
    cpu.r[reg] = if down { before - ticks } else { before.wrapping_add(ticks) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_step<T>(step: Option<Step>, f: impl FnOnce() -> T) -> T {
        publish(step);
        let result = f();
        publish(None);
        result
    }

    #[test]
    fn unpublished_steps_leave_registers_unchanged() {
        let mut cpu = Cpu::default();
        cpu.s[2] = 19.0f32.to_bits();
        scale_single(&mut cpu, 2);
        assert_eq!(f32::from_bits(cpu.s[2]), 19.0);
    }

    #[test]
    fn scaling_follows_elapsed_time() {
        for (elapsed, expected) in [(16_713_680, 9.5), (7_313_000, 19.0 * 7_313_000.0 / 33_427_360.0), (83_157_602, 47.267)] {
            let mut cpu = Cpu::default();
            cpu.s[2] = 19.0f32.to_bits();
            with_step(Some(Step { sequence: 1, elapsed_ns: elapsed }), || scale_single(&mut cpu, 2));
            assert!((f32::from_bits(cpu.s[2]) - expected as f32).abs() < 1e-3, "{elapsed}");
        }
    }

    #[test]
    fn short_vector_mode_is_left_alone() {
        let mut cpu = Cpu { fpscr: 1 << 16, ..Cpu::default() }; // LEN = 2
        cpu.s[1] = 1.0f32.to_bits();
        with_step(Some(Step { sequence: 1, elapsed_ns: 16_713_680 }), || scale_single(&mut cpu, 1));
        assert_eq!(f32::from_bits(cpu.s[1]), 1.0);
    }

    #[test]
    fn results_are_scaled_only_for_the_named_callee_and_caller() {
        let mut mem = Memory::new();
        mem.map(0x1000, vec![0; 0x1000], crate::Perm::RW).unwrap();
        mem.write_u32(0x1000 + 140, 0x3a975c).unwrap();
        let step = Some(Step { sequence: 1, elapsed_ns: 16_713_680 });
        for (callee, caller, expected) in [(0x4b7f94, 0x3a975c, 0.5), (0x4b7f00, 0x3a975c, 1.0), (0x4b7f94, 0x3a9000, 1.0)] {
            let mut cpu = Cpu::default();
            cpu.r[13] = 0x1000;
            cpu.r[7] = callee;
            cpu.s[0] = 1.0f32.to_bits();
            mem.write_u32(0x1000 + 140, caller).unwrap();
            with_step(step, || scale_result(&mut cpu, &mem, 0, 7, 0x4b7f94, 140, 0x3a975c));
            assert_eq!(f32::from_bits(cpu.s[0]), expected);
        }
    }

    #[test]
    fn blends_compound_to_the_same_result_over_elapsed_time() {
        let mut cpu = Cpu::default();
        cpu.s[16] = 0.25f32.to_bits();
        with_step(Some(Step { sequence: 1, elapsed_ns: NORMAL_UPDATE_NS * 2 }), || blend_single(&mut cpu, 16));
        // two normal updates of 0.25 leave 0.75^2 of the distance
        assert!((f32::from_bits(cpu.s[16]) - 0.4375).abs() < 1e-6);
        for value in [0.0f32, 1.0, -0.5, f32::NAN] {
            cpu.s[16] = value.to_bits();
            with_step(Some(Step { sequence: 1, elapsed_ns: 1 }), || blend_single(&mut cpu, 16));
            assert_eq!(cpu.s[16], value.to_bits());
        }
    }

    fn run_counter(steps: &[u64], down: bool, first_immediate: bool, start: u32) -> Vec<u32> {
        let mut value = start;
        let mut out = Vec::new();
        for (index, &elapsed) in steps.iter().enumerate() {
            let mut cpu = Cpu::default();
            cpu.r[4] = 0x5000;
            cpu.r[0] = if down { value - 1 } else { value + 1 };
            with_step(Some(Step { sequence: index as u64 + 1, elapsed_ns: elapsed }), || {
                counter(&mut cpu, 0, 4, 0x48, down, first_immediate)
            });
            value = cpu.r[0];
            out.push(value);
        }
        COUNTERS.with(|c| c.borrow_mut().clear());
        out
    }

    #[test]
    fn counters_advance_once_per_normal_update_of_elapsed_time() {
        let half = NORMAL_UPDATE_NS / 2;
        assert_eq!(run_counter(&[half; 6], false, false, 0), [0, 1, 1, 2, 2, 3]);
        assert_eq!(run_counter(&[NORMAL_UPDATE_NS * 3, half, half], false, false, 10), [13, 13, 14]);
        assert_eq!(run_counter(&[half; 4], true, true, 3), [2, 2, 1, 1]);
        assert_eq!(run_counter(&[NORMAL_UPDATE_NS * 5], true, false, 2), [0]);
    }

    #[test]
    fn resets_and_repeated_calls_are_respected() {
        let mut cpu = Cpu::default();
        cpu.r[4] = 0x6000;
        let call = |cpu: &mut Cpu, value: u32, sequence: u64| {
            cpu.r[0] = value + 1;
            with_step(Some(Step { sequence, elapsed_ns: NORMAL_UPDATE_NS }), || counter(cpu, 0, 4, 0, false, false));
            cpu.r[0]
        };
        assert_eq!(call(&mut cpu, 5, 1), 6);
        assert_eq!(call(&mut cpu, 6, 1), 6); // same update: no second tick
        assert_eq!(call(&mut cpu, 6, 2), 7);
        assert_eq!(call(&mut cpu, 0, 3), 1); // reset by the game: a new clock
        COUNTERS.with(|c| c.borrow_mut().clear());
        cpu.r[0] = 8;
        counter(&mut cpu, 0, 4, 0, false, false); // no step: unchanged
        assert_eq!(cpu.r[0], 8);
    }
}
