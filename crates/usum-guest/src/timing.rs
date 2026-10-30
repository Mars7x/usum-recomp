//! Fractional guest timer arithmetic for native cadence adaptations.
//!
//! Callers supply elapsed guest-time units, choose which timers advance, and
//! clear state at actual resets or object-lifetime boundaries. This module does
//! not read wall time, schedule the guest, or count rendered frames.

use std::num::NonZeroU32;

/// Converts elapsed units to whole logical ticks without discarding fractions.
///
/// The unit scale stays fixed for the accumulator's lifetime. For example, two
/// units per tick represents half-tick updates. For a 30 Hz timer driven by
/// nanosecond timestamps, use one billion units per tick and supply elapsed
/// nanoseconds multiplied by 30, checking that multiplication at the call site.
/// Changing render cadence does not require clearing the accumulated fraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickAccumulator {
    units_per_tick: NonZeroU32,
    remainder: u32,
}

impl TickAccumulator {
    pub const fn new(units_per_tick: NonZeroU32) -> Self {
        Self { units_per_tick, remainder: 0 }
    }

    /// Advance by elapsed units and return the number of complete logical ticks.
    /// Zero elapsed time retains the fraction and produces no ticks.
    pub fn advance(&mut self, elapsed_units: u64) -> u64 {
        let scale = u64::from(self.units_per_tick.get());
        // Split before adding the remainder: elapsed_units may be u64::MAX.
        // The partial sum is below twice u32::MAX, and the resulting tick count
        // cannot exceed u64::MAX for any nonzero scale.
        let whole = elapsed_units / scale;
        let partial = elapsed_units % scale + u64::from(self.remainder);
        self.remainder = (partial % scale) as u32;
        whole + partial / scale
    }

    /// Discard fractional progress when the guest timer is reset or replaced.
    /// A guest counter being written zero again can still require this reset.
    pub fn clear(&mut self) {
        self.remainder = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::TickAccumulator;
    use std::num::NonZeroU32;

    #[test]
    fn zero_to_zero_reset_discards_pending_half_tick() {
        let mut clock = TickAccumulator::new(NonZeroU32::new(2).unwrap());
        assert_eq!(clock.advance(1), 0);
        assert_eq!(clock.advance(0), 0);
        clock.clear();
        assert_eq!(clock.advance(1), 0);
        assert_eq!(clock.advance(1), 1);
    }

    #[test]
    fn cadence_partitions_preserve_elapsed_guest_time() {
        for hz in [24, 30, 60, 90, 120, 144, 165, 200, 240] {
            let mut clock = TickAccumulator::new(NonZeroU32::new(1_000_000_000).unwrap());
            let mut previous_ns = 0;
            let mut ticks = 0;
            for frame in 1..=hz * 10 {
                let now_ns = frame * 1_000_000_000 / hz;
                ticks += clock.advance((now_ns - previous_ns) * 30);
                assert_eq!(ticks, now_ns * 30 / 1_000_000_000);
                previous_ns = now_ns;
            }
            assert_eq!(ticks, 300);
        }
    }

    #[test]
    fn changing_update_intervals_preserves_pending_time() {
        let mut clock = TickAccumulator::new(NonZeroU32::new(1_000_000_000).unwrap());
        let intervals_ns = [8_000_000, 0, 7_000_000, 25_000_000, 1_000_000, 59_000_000];
        let ticks: Vec<_> = intervals_ns.into_iter().map(|ns| clock.advance(ns * 30)).collect();
        assert_eq!(ticks, [0, 0, 0, 1, 0, 2]);
    }

    #[test]
    fn large_elapsed_values_do_not_overflow_or_lose_remainders() {
        for scale in [1, 2, 3, 1_000_000_000, u32::MAX] {
            let mut clock = TickAccumulator::new(NonZeroU32::new(scale).unwrap());
            let mut total = 0u128;
            let mut emitted = 0u128;
            for elapsed in [u64::from(scale) - 1, u64::MAX, 0, u64::MAX, 1] {
                total += u128::from(elapsed);
                emitted += u128::from(clock.advance(elapsed));
                assert_eq!(emitted, total / u128::from(scale));
            }
        }
    }
}
