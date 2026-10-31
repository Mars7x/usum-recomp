//! Guest timing helpers for native cadence adaptations.
//!
//! Callers supply elapsed guest-time units, choose which timers advance, and
//! clear state at actual resets or object-lifetime boundaries. This module does
//! not read wall time, schedule the guest, or count rendered frames.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// Convert a per-logical-tick exponential blend to an elapsed-time blend.
///
/// `elapsed_ticks` is elapsed guest time divided by the nominal tick duration,
/// not a render-frame count. With a constant target, splitting an interval
/// preserves its total decay: the remaining fraction is `(1 - blend)^time`.
/// Callers retain responsibility for moving targets and discrete event timing.
/// Unsupported values return `None` so the caller can retain guest behavior.
pub fn blend_for_elapsed(blend: f64, elapsed_ticks: f64) -> Option<f64> {
    if !blend.is_finite() || !(0.0..=1.0).contains(&blend) || !elapsed_ticks.is_finite() || elapsed_ticks < 0.0 {
        return None;
    }
    if elapsed_ticks == 0.0 || blend == 0.0 {
        return Some(0.0);
    }
    if elapsed_ticks == 1.0 || blend == 1.0 {
        return Some(blend);
    }
    // Preserve the previously audited half-tick calculation, including its
    // rounding, while avoiding cancellation for small nominal blends.
    if elapsed_ticks == 0.5 {
        return Some(blend / (1.0 + (1.0 - blend).sqrt()));
    }
    Some(-((-blend).ln_1p() * elapsed_ticks).exp_m1())
}

/// Single-use timing credits owned by individual states in one guest frame.
///
/// A credit can pair a pose update with its later event query without allowing
/// another state's update to overwrite it. Frame identifiers come from the
/// caller's guest update domain, not presentation or wall time. The caller must
/// validate the live owner and animation fields before consuming a credit, and
/// invalidate it on reset, rebinding, destruction or address reuse, even when
/// guest values do not change. This container does not infer object lifetimes.
#[derive(Clone, Debug)]
pub struct FrameCredits<K, V> {
    frame: Option<u64>,
    entries: BTreeMap<K, V>,
}

impl<K: Ord, V> Default for FrameCredits<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord, V> FrameCredits<K, V> {
    pub const fn new() -> Self {
        Self { frame: None, entries: BTreeMap::new() }
    }

    /// Publish a credit, replacing this state's previous credit. Publishing in
    /// a different frame discards all older metadata, including on a rewind.
    pub fn insert(&mut self, frame: u64, state: K, value: V) -> Option<V> {
        if self.frame != Some(frame) {
            self.entries.clear();
            self.frame = Some(frame);
        }
        self.entries.insert(state, value)
    }

    /// Inspect a credit before checking the caller's live ownership guards.
    pub fn peek(&self, frame: u64, state: &K) -> Option<&V> {
        if self.frame != Some(frame) {
            return None;
        }
        self.entries.get(state)
    }

    /// Consume only this state's credit, and only in its publication frame.
    pub fn take(&mut self, frame: u64, state: &K) -> Option<V> {
        if self.frame != Some(frame) {
            return None;
        }
        self.entries.remove(state)
    }

    /// Invalidate a state at a lifecycle boundary, independently of other states.
    pub fn invalidate(&mut self, state: &K) -> Option<V> {
        self.entries.remove(state)
    }

    /// Discard every credit, for example when replacing the guest session.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.frame = None;
    }
}

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
    use super::{blend_for_elapsed, FrameCredits, TickAccumulator};
    use std::num::NonZeroU32;

    #[test]
    fn changing_cadence_preserves_constant_target_decay() {
        for blend in [1e-12, 0.01, 0.25, 0.75, 0.99] {
            // Changing cadence, including a zero-time update and a long step.
            let steps = [0.125, 0.5, 0.0, 0.3, 1.25, 0.025, 0.8];
            let remaining = steps.into_iter().fold(1.0, |r, dt| r * (1.0 - blend_for_elapsed(blend, dt).unwrap()));
            let expected = 1.0 - blend_for_elapsed(blend, steps.into_iter().sum()).unwrap();
            assert!((remaining - expected).abs() < 2e-15);
        }
    }

    #[test]
    fn elapsed_blend_preserves_known_steps_and_small_values() {
        for blend in [f64::from(f32::MIN_POSITIVE), 1e-12, 0.1, 0.9, 1.0] {
            assert_eq!(blend_for_elapsed(blend, 0.0), Some(0.0));
            assert_eq!(blend_for_elapsed(blend, 1.0), Some(blend));
            assert_eq!(blend_for_elapsed(blend, 0.5), Some(blend / (1.0 + (1.0 - blend).sqrt())));
            for hz in [24, 30, 40, 60, 90, 120, 144, 165, 240] {
                let step = blend_for_elapsed(blend, 30.0 / f64::from(hz)).unwrap();
                assert!(step > 0.0 && step <= 1.0);
            }
        }
        assert_eq!(blend_for_elapsed(0.0, f64::MAX), Some(0.0));
        assert_eq!(blend_for_elapsed(1.0, 0.1), Some(1.0));
    }

    #[test]
    fn unsupported_blends_and_elapsed_times_are_not_silently_clamped() {
        for blend in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            assert_eq!(blend_for_elapsed(blend, 0.5), None);
        }
        for dt in [-0.1, f64::NAN, f64::INFINITY] {
            assert_eq!(blend_for_elapsed(0.5, dt), None);
        }
    }

    #[test]
    fn interleaved_states_consume_independently_once() {
        let mut credits = FrameCredits::new();
        credits.insert(20, "player", 1);
        credits.insert(20, "npc", 2);
        assert_eq!(credits.peek(20, &"player"), Some(&1));
        assert_eq!(credits.take(20, &"player"), Some(1));
        assert_eq!(credits.take(20, &"player"), None);
        assert_eq!(credits.take(20, &"npc"), Some(2));
    }

    #[test]
    fn same_value_reset_and_address_reuse_need_a_new_credit() {
        let mut credits = FrameCredits::new();
        credits.insert(20, 0x1000, 1);
        credits.insert(20, 0x2000, 1);
        credits.invalidate(&0x1000);
        assert_eq!(credits.take(20, &0x1000), None);
        assert_eq!(credits.take(20, &0x2000), Some(1));
        credits.insert(20, 0x1000, 1);
        assert_eq!(credits.take(20, &0x1000), Some(1));
    }

    #[test]
    fn frame_changes_and_session_reset_do_not_resurrect_credits() {
        let mut credits = FrameCredits::new();
        credits.insert(20, 1, 7);
        assert_eq!(credits.peek(21, &1), None);
        assert_eq!(credits.take(21, &1), None);
        credits.insert(21, 2, 8);
        assert_eq!(credits.take(20, &1), None);
        credits.insert(19, 3, 9);
        assert_eq!(credits.take(21, &2), None);
        credits.clear();
        assert_eq!(credits.take(19, &3), None);
        credits.insert(19, 3, 10);
        assert_eq!(credits.take(19, &3), Some(10));
    }

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
