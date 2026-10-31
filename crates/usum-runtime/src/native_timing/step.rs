//! Immutable elapsed-time intervals shared by an engine update's consumers.

use std::num::NonZeroU64;

/// One engine update's guest-time interval, independent of hardware frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateStep {
    sequence: u64,
    start_ns: u64,
    end_ns: u64,
}

impl UpdateStep {
    /// Unique within this clock's lifetime, including across `clear` calls.
    pub fn sequence(self) -> u64 {
        self.sequence
    }
    pub fn start_ns(self) -> u64 {
        self.start_ns
    }
    pub fn end_ns(self) -> u64 {
        self.end_ns
    }
    pub fn elapsed_ns(self) -> u64 {
        self.end_ns - self.start_ns
    }

    /// Elapsed guest time expressed in the consumer's nominal logical ticks.
    /// The caller supplies the actual game tick duration, not a display rate.
    pub fn logical_ticks(self, nominal_tick_ns: NonZeroU64) -> f64 {
        self.elapsed_ns() as f64 / nominal_tick_ns.get() as f64
    }
}

/// Samples time once at a verified engine-update boundary.
///
/// The first sample establishes an origin without inventing elapsed time.
/// Consumers read the same immutable `current` step until the next boundary.
/// Zero elapsed time, clock rewind and sequence exhaustion publish no step,
/// preventing a consumer from accidentally reusing the preceding interval.
/// This clock does not schedule work or change the source guest clock.
#[derive(Debug, Default)]
pub struct UpdateClock {
    previous_ns: Option<u64>,
    sequence: u64,
    current: Option<UpdateStep>,
}

impl UpdateClock {
    pub const fn new() -> Self {
        Self { previous_ns: None, sequence: 0, current: None }
    }

    pub fn begin_update(&mut self, now_ns: u64) -> Option<UpdateStep> {
        self.current = None;
        let Some(start_ns) = self.previous_ns else {
            self.previous_ns = Some(now_ns);
            return None;
        };
        if now_ns <= start_ns {
            return None;
        }
        let sequence = self.sequence.checked_add(1)?;
        let step = UpdateStep { sequence, start_ns, end_ns: now_ns };
        self.previous_ns = Some(now_ns);
        self.sequence = sequence;
        self.current = Some(step);
        Some(step)
    }

    pub fn current(&self) -> Option<UpdateStep> {
        self.current
    }

    /// Forget the interval at a session/lifecycle boundary or explicit pause.
    /// The next sample anchors a new interval, excluding the intervening gap.
    /// Callers must also invalidate their state-specific timing credits.
    pub fn clear(&mut self) {
        self.previous_ns = None;
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varying_intervals_share_one_step_and_preserve_all_elapsed_time() {
        let nominal = NonZeroU64::new(33_427_360).unwrap();
        let mut clock = UpdateClock::new();
        let mut now = 5_000_000_000;
        assert_eq!(clock.begin_update(now), None);
        let mut total = 0;
        for (index, interval) in [16_713_680, 8_356_840, 23_000_001, 5_000_000, 60_000_000].into_iter().enumerate() {
            now += interval;
            let step = clock.begin_update(now).unwrap();
            assert_eq!(step.sequence(), index as u64 + 1);
            assert_eq!(step.elapsed_ns(), interval);
            assert_eq!(clock.current(), Some(step));
            assert_eq!(clock.current(), Some(step)); // Another consumer does not resample.
            assert_eq!(step.logical_ticks(nominal), interval as f64 / nominal.get() as f64);
            total += step.elapsed_ns();
        }
        assert_eq!(total, now - 5_000_000_000);
    }

    #[test]
    fn paused_and_rewound_samples_never_reuse_an_old_step() {
        let mut clock = UpdateClock::new();
        clock.begin_update(100);
        assert!(clock.begin_update(120).is_some());
        assert_eq!(clock.begin_update(120), None);
        assert_eq!(clock.current(), None);
        assert_eq!(clock.begin_update(110), None);
        assert_eq!(clock.current(), None);
        assert_eq!(clock.begin_update(130).unwrap().elapsed_ns(), 10);
        clock.clear();
        assert_eq!(clock.current(), None);
        assert_eq!(clock.begin_update(10_000), None);
        let step = clock.begin_update(10_005).unwrap();
        assert_eq!(step.elapsed_ns(), 5);
        assert_eq!(step.sequence(), 3);
    }

    #[test]
    fn large_timestamps_and_sequence_exhaustion_do_not_wrap() {
        let mut clock = UpdateClock::new();
        clock.begin_update(0);
        assert_eq!(clock.begin_update(u64::MAX).unwrap().elapsed_ns(), u64::MAX);
        clock.clear();
        clock.begin_update(0);
        clock.sequence = u64::MAX;
        assert_eq!(clock.begin_update(1), None);
        assert_eq!(clock.current(), None);
    }
}
