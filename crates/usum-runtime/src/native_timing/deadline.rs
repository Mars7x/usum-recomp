//! Update deadlines independent of the emulated display and audio clocks.

use std::num::NonZeroU64;

/// A deadline for one update, armed from its actual start time.
///
/// The caller chooses each interval independently. Late updates become ready
/// immediately; subsequent updates must be armed from their actual start rather
/// than replaying missed deadlines. This does not bypass guest synchronization
/// or schedule a thread itself. Consumers can wait through the existing kernel.
#[derive(Debug, Default)]
pub struct UpdateDeadline {
    interval: Option<(u64, u64)>,
}

impl UpdateDeadline {
    pub const fn new() -> Self {
        Self { interval: None }
    }

    /// Replace the previous deadline. Overflow leaves the deadline unarmed.
    pub fn arm(&mut self, start_ns: u64, interval_ns: NonZeroU64) -> Option<u64> {
        self.interval = None;
        let deadline = start_ns.checked_add(interval_ns.get())?;
        self.interval = Some((start_ns, deadline));
        Some(deadline)
    }

    /// Remaining time, or zero when due. An absent deadline or a clock before
    /// the update's start returns `None`; callers must not interpret it as due.
    /// Polling does not consume, advance, or rearm the deadline.
    pub fn remaining_ns(&self, now_ns: u64) -> Option<u64> {
        let (start, deadline) = self.interval?;
        (now_ns >= start).then(|| deadline.saturating_sub(now_ns))
    }

    /// Invalidate pending pacing on pause, restart, or clock-domain changes.
    pub fn clear(&mut self) {
        self.interval = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_intervals_wait_only_for_remaining_work_time() {
        let mut schedule = UpdateDeadline::new();
        let mut start = 1_000;
        for interval in [5_000_000, 11_111_111, 6_944_444, 23_000_001] {
            let due = schedule.arm(start, NonZeroU64::new(interval).unwrap()).unwrap();
            assert_eq!(due, start + interval);
            assert_eq!(schedule.remaining_ns(start + 731), Some(interval - 731));
            assert_eq!(schedule.remaining_ns(start + 731), Some(interval - 731));
            assert_eq!(schedule.remaining_ns(due), Some(0));
            start = due;
        }
        assert_eq!(schedule.remaining_ns(start + 100_000_000), Some(0));
        let resumed = start + 100_000_000;
        schedule.arm(resumed, NonZeroU64::new(7_000_000).unwrap());
        assert_eq!(schedule.remaining_ns(resumed), Some(7_000_000));
    }

    #[test]
    fn invalid_clock_and_lifecycle_cannot_reuse_a_deadline() {
        let mut schedule = UpdateDeadline::new();
        let interval = NonZeroU64::new(10).unwrap();
        assert_eq!(schedule.remaining_ns(100), None);
        schedule.arm(100, interval);
        assert_eq!(schedule.remaining_ns(99), None);
        schedule.clear();
        assert_eq!(schedule.remaining_ns(110), None);
        schedule.arm(100, interval);
        assert_eq!(schedule.arm(u64::MAX - 5, interval), None);
        assert_eq!(schedule.remaining_ns(u64::MAX), None);
    }
}
