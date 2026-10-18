use super::super::bridge::scheduler::IntervalGroupSource;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaBridgeTelemetryAccumulator {
    submitted_intervals: u64,
    dropped_backpressure_intervals: u64,
    fallback_intervals: u64,
    skipped_intervals: u64,
    stale_frames_dropped: u64,
    dirty: bool,
}

impl MediaBridgeTelemetryAccumulator {
    fn observe_interval(
        &mut self,
        source: IntervalGroupSource,
        skipped_intervals: u64,
        stale_frames_dropped: usize,
    ) {
        if source == IntervalGroupSource::Fallback {
            self.fallback_intervals = self.fallback_intervals.saturating_add(1);
        }
        self.skipped_intervals = self.skipped_intervals.saturating_add(skipped_intervals);
        self.stale_frames_dropped = self
            .stale_frames_dropped
            .saturating_add(stale_frames_dropped as u64);
        self.dirty = true;
    }

    pub fn observe_submitted(
        &mut self,
        source: IntervalGroupSource,
        skipped_intervals: u64,
        stale_frames_dropped: usize,
    ) {
        self.observe_interval(source, skipped_intervals, stale_frames_dropped);
        self.submitted_intervals = self.submitted_intervals.saturating_add(1);
    }

    pub fn observe_dropped_backpressure(
        &mut self,
        source: IntervalGroupSource,
        skipped_intervals: u64,
        stale_frames_dropped: usize,
    ) {
        self.observe_interval(source, skipped_intervals, stale_frames_dropped);
        self.dropped_backpressure_intervals = self
            .dropped_backpressure_intervals
            .saturating_add(1);
    }

    pub const fn submitted_intervals(&self) -> u64 { self.submitted_intervals }
    pub const fn dropped_backpressure_intervals(&self) -> u64 { self.dropped_backpressure_intervals }
    pub const fn fallback_intervals(&self) -> u64 { self.fallback_intervals }
    pub const fn skipped_intervals(&self) -> u64 { self.skipped_intervals }
    pub const fn stale_frames_dropped(&self) -> u64 { self.stale_frames_dropped }

    pub fn take_dirty(&mut self) -> bool {
        let dirty = self.dirty;
        self.dirty = false;
        dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_coalesce_without_resetting_media_metrics() {
        let mut value = MediaBridgeTelemetryAccumulator::default();
        value.observe_submitted(IntervalGroupSource::Encoded, 2, 1);
        value.observe_dropped_backpressure(IntervalGroupSource::Fallback, 1, 3);
        assert_eq!(value.submitted_intervals(), 1);
        assert_eq!(value.dropped_backpressure_intervals(), 1);
        assert_eq!(value.fallback_intervals(), 1);
        assert_eq!(value.skipped_intervals(), 3);
        assert_eq!(value.stale_frames_dropped(), 4);
        assert!(value.take_dirty());
        assert!(!value.take_dirty());
        assert_eq!(value.submitted_intervals(), 1);
    }
}
