//! Guest-time observations for native cadence consumers.
//!
//! A dispatch publishes its clock origin and instruction budget. Hooks can
//! observe virtual time including instructions not yet charged to the kernel,
//! or the same monotonic origin used by the live kernel. This does not advance
//! either clock, schedule updates, or infer time from presentation counts.

use std::cell::Cell;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::rc::Rc;
use std::time::Instant;

#[derive(Clone, Copy)]
pub(crate) enum DispatchClock {
    Virtual { ns: u64, budget: i64, cycles_per_second: NonZeroU64 },
    Host { started: Option<Instant> },
}

impl DispatchClock {
    fn timestamp(self, remaining_budget: i64) -> Option<u64> {
        match self {
            Self::Virtual { ns, budget, cycles_per_second } => {
                let spent = u64::try_from(i128::from(budget) - i128::from(remaining_budget)).ok()?;
                let elapsed = u128::from(spent) * 1_000_000_000 / u128::from(cycles_per_second.get());
                ns.checked_add(u64::try_from(elapsed).ok()?)
            }
            Self::Host { started: Some(started) } => u64::try_from(started.elapsed().as_nanos()).ok(),
            Self::Host { started: None } => Some(0),
        }
    }
}

thread_local! {
    static DISPATCH_CLOCK: Cell<Option<DispatchClock>> = const { Cell::new(None) };
}

/// Observe the current guest time without mutating the kernel clock.
///
/// Pass the current CPU instruction budget. Returns `None` outside an enabled
/// dispatch, if the budget moved backwards relative to this dispatch's origin,
/// or if the timestamp cannot be represented. This is a timing source, not an
/// FPS measurement. Consumers must establish their own update/lifecycle bounds.
pub fn guest_time_ns(remaining_budget: i64) -> Option<u64> {
    DISPATCH_CLOCK.with(|clock| clock.get()?.timestamp(remaining_budget))
}

// A guard must be dropped on the host thread that published the context.
pub(crate) struct ClockScope {
    previous: Option<DispatchClock>,
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn enter_dispatch(clock: Option<DispatchClock>) -> ClockScope {
    ClockScope { previous: DISPATCH_CLOCK.with(|current| current.replace(clock)), _thread_bound: PhantomData }
}

impl Drop for ClockScope {
    fn drop(&mut self) {
        DISPATCH_CLOCK.with(|clock| clock.set(self.previous));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn virtual_clock(ns: u64, budget: i64) -> DispatchClock {
        DispatchClock::Virtual { ns, budget, cycles_per_second: NonZeroU64::new(268_111_856).unwrap() }
    }

    #[test]
    fn observations_include_uncharged_work_without_advancing_the_origin() {
        let _scope = enter_dispatch(Some(virtual_clock(900, 1000)));
        assert_eq!(guest_time_ns(1000), Some(900));
        let expected = 900 + 250 * 1_000_000_000 / 268_111_856;
        assert_eq!(guest_time_ns(750), Some(expected));
        assert_eq!(guest_time_ns(750), Some(expected));
        assert_eq!(guest_time_ns(1001), None);
    }

    #[test]
    fn nested_sessions_and_dispatches_restore_their_own_context() {
        let _outer = enter_dispatch(Some(virtual_clock(10, 200)));
        {
            let _disabled_session = enter_dispatch(None);
            assert_eq!(guest_time_ns(200), None);
            {
                let _inner = enter_dispatch(Some(virtual_clock(100, 50)));
                assert_eq!(guest_time_ns(50), Some(100));
            }
            assert_eq!(guest_time_ns(200), None);
        }
        assert_eq!(guest_time_ns(200), Some(10));
    }

    #[test]
    fn dispatch_refills_and_sleep_jumps_use_new_origins() {
        {
            let _scope = enter_dispatch(Some(virtual_clock(1000, 20)));
            assert_eq!(guest_time_ns(10), Some(1037));
        }
        assert_eq!(guest_time_ns(10), None);
        let _scope = enter_dispatch(Some(virtual_clock(1_000_000, 1000)));
        assert_eq!(guest_time_ns(1000), Some(1_000_000));
        assert_eq!(guest_time_ns(990), Some(1_000_037));
    }

    #[test]
    fn context_is_cleared_on_unwind_and_not_shared_with_other_threads() {
        let _outer = enter_dispatch(None);
        let _ = std::panic::catch_unwind(|| {
            let _scope = enter_dispatch(Some(virtual_clock(100, 10)));
            assert_eq!(std::thread::spawn(|| guest_time_ns(10)).join().unwrap(), None);
            panic!("exercise dispatch cleanup");
        });
        assert_eq!(guest_time_ns(10), None);
    }

    #[test]
    fn host_origin_and_timestamp_overflow_are_explicit() {
        assert_eq!(DispatchClock::Host { started: None }.timestamp(0), Some(0));
        let origin = Instant::now();
        let value = DispatchClock::Host { started: Some(origin) }.timestamp(i64::MIN).unwrap();
        assert!(u128::from(value) <= origin.elapsed().as_nanos());
        assert_eq!(virtual_clock(u64::MAX, 1000).timestamp(0), None);
        let huge = DispatchClock::Virtual { ns: 0, budget: i64::MAX, cycles_per_second: NonZeroU64::new(1).unwrap() };
        assert_eq!(huge.timestamp(i64::MIN), None);
    }
}
