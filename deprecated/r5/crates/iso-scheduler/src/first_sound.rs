//! A bounded first-sound clock, not a production clock-recovery/resampling loop.
use std::time::{Duration, Instant};

pub const SDU_PERIOD: Duration = Duration::from_millis(10);
/// Backpressure itself is normal: BE200 reports only eight ISO packet credits,
/// i.e. four stereo frame pairs. Only fail when no stereo pair can be admitted
/// for a genuinely pathological amount of time.
pub const CREDIT_STALL_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacingObservation {
    pub lateness: Duration,
    pub rebased: bool,
}

/// Host-side minimum-rate pacer. Controller ISO credits are the authoritative
/// backpressure signal. When userspace wakes more than one SDU period late, do
/// not replay missed deadlines as a catch-up burst: rebase the next deadline to
/// one period after the actual submission. This matches the upstream Bumble-RS
/// unicast behavior and keeps stereo frame pairs atomic.
pub struct StereoPacer {
    next_deadline: Instant,
}

impl StereoPacer {
    pub fn new(first_deadline: Instant) -> Self {
        Self {
            next_deadline: first_deadline,
        }
    }

    pub fn deadline(&self) -> Instant {
        self.next_deadline
    }

    pub fn submitted(&mut self, now: Instant) -> PacingObservation {
        let lateness = now.saturating_duration_since(self.next_deadline);
        let nominal_next = self.next_deadline + SDU_PERIOD;
        let rebased = nominal_next <= now;
        self.next_deadline = if rebased {
            now + SDU_PERIOD
        } else {
            nominal_next
        };
        PacingObservation { lateness, rebased }
    }
}

/// Reserve room for BOTH ears before queueing either. `pending` includes queued
/// and in-flight HCI packets; max_in_flight is the reported controller window.
pub fn pair_fits(pending: usize, max_in_flight: usize) -> bool {
    max_in_flight >= 2 && pending <= max_in_flight - 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_jitter_preserves_nominal_clock() {
        let now = Instant::now();
        let mut pacer = StereoPacer::new(now + Duration::from_millis(20));
        assert_eq!(pacer.deadline().duration_since(now), Duration::from_millis(20));
        let observation = pacer.submitted(now + Duration::from_millis(25));
        assert_eq!(observation.lateness, Duration::from_millis(5));
        assert!(!observation.rebased);
        assert_eq!(pacer.deadline().duration_since(now), Duration::from_millis(30));
    }

    #[test]
    fn long_stall_rebases_instead_of_catch_up_burst() {
        let now = Instant::now();
        let mut pacer = StereoPacer::new(now + Duration::from_millis(20));
        let submitted = now + Duration::from_millis(52);
        let observation = pacer.submitted(submitted);
        assert_eq!(observation.lateness, Duration::from_millis(32));
        assert!(observation.rebased);
        assert_eq!(pacer.deadline().duration_since(submitted), SDU_PERIOD);
    }

    #[test]
    fn never_spend_the_last_single_credit_on_one_ear_only() {
        assert!(pair_fits(6, 8));
        assert!(!pair_fits(7, 8));
        assert!(!pair_fits(0, 0));
        assert!(!pair_fits(0, 1));
        assert!(!pair_fits(usize::MAX, 8));
    }
}
