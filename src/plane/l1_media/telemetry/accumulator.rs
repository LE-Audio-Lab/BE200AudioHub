use super::super::{
    bridge::scheduler::IntervalGroupSource,
    clock::frame::FrameId,
    interface::condition::StreamClockSnapshot,
};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaTimingTelemetryAccumulator {
    produced_frame: Option<FrameId>,
    clock: Option<StreamClockSnapshot>,
    pcm_fill_frames: usize,
    intervals_consumed: u64,
    source_intervals_skipped: u64,
    keepalive_intervals: u64,
    pcm_frames_read: u64,
    pcm_underflow_frames: u64,
    startup_trim_frames: u64,
    gradual_drift_trim_frames: u64,
    emergency_drift_trim_frames: u64,
    dirty: bool,
}

impl MediaTimingTelemetryAccumulator {
    pub fn observe_consume(
        &mut self,
        requested_frames: usize,
        read_frames: usize,
        keepalive: bool,
        startup_trim_frames: usize,
        gradual_drift_trim_frames: usize,
        emergency_drift_trim_frames: usize,
    ) {
        self.intervals_consumed =
            self.intervals_consumed.saturating_add(1);
        self.pcm_frames_read = self
            .pcm_frames_read
            .saturating_add(read_frames as u64);
        self.pcm_underflow_frames = self
            .pcm_underflow_frames
            .saturating_add(
                requested_frames.saturating_sub(read_frames) as u64,
            );
        self.startup_trim_frames = self
            .startup_trim_frames
            .saturating_add(startup_trim_frames as u64);
        self.gradual_drift_trim_frames = self
            .gradual_drift_trim_frames
            .saturating_add(gradual_drift_trim_frames as u64);
        self.emergency_drift_trim_frames = self
            .emergency_drift_trim_frames
            .saturating_add(emergency_drift_trim_frames as u64);
        if keepalive {
            self.keepalive_intervals =
                self.keepalive_intervals.saturating_add(1);
        }
        self.dirty = true;
    }

    pub fn observe_interval_context(
        &mut self,
        frame_id: FrameId,
        clock: StreamClockSnapshot,
        pcm_fill_frames: usize,
        source_intervals_skipped: u64,
    ) {
        self.produced_frame = Some(frame_id);
        self.clock = Some(clock);
        self.pcm_fill_frames = pcm_fill_frames;
        self.source_intervals_skipped = self
            .source_intervals_skipped
            .saturating_add(source_intervals_skipped);
        self.dirty = true;
    }

    pub const fn produced_frame(&self) -> Option<FrameId> { self.produced_frame }
    pub const fn clock(&self) -> Option<StreamClockSnapshot> { self.clock }
    pub const fn pcm_fill_frames(&self) -> usize { self.pcm_fill_frames }
    pub const fn intervals_consumed(&self) -> u64 { self.intervals_consumed }
    pub const fn source_intervals_skipped(&self) -> u64 { self.source_intervals_skipped }
    pub const fn keepalive_intervals(&self) -> u64 { self.keepalive_intervals }
    pub const fn pcm_frames_read(&self) -> u64 { self.pcm_frames_read }
    pub const fn pcm_underflow_frames(&self) -> u64 { self.pcm_underflow_frames }
    pub const fn startup_trim_frames(&self) -> u64 { self.startup_trim_frames }
    pub const fn gradual_drift_trim_frames(&self) -> u64 { self.gradual_drift_trim_frames }
    pub const fn emergency_drift_trim_frames(&self) -> u64 { self.emergency_drift_trim_frames }

    pub fn take_dirty(&mut self) -> bool {
        let dirty = self.dirty;
        self.dirty = false;
        dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plane::l1_media::interface::condition::ClockLock;

    #[test]
    fn bridge_counters_coalesce_without_resetting_media_metrics() {
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

    #[test]
    fn timing_counters_keep_event_totals_and_latest_gauges() {
        let mut value = MediaTimingTelemetryAccumulator::default();
        value.observe_consume(480, 470, false, 3, 1, 0);
        value.observe_consume(480, 480, true, 0, 0, 0);
        value.observe_interval_context(
            FrameId(9),
            StreamClockSnapshot {
                period_us: 10_000,
                current_frame: FrameId(9),
                lock: ClockLock::LocalMonotonic,
                rate_error_ppm: 0,
            },
            512,
            2,
        );

        assert_eq!(value.produced_frame(), Some(FrameId(9)));
        assert_eq!(value.pcm_fill_frames(), 512);
        assert_eq!(value.intervals_consumed(), 2);
        assert_eq!(value.source_intervals_skipped(), 2);
        assert_eq!(value.keepalive_intervals(), 1);
        assert_eq!(value.pcm_frames_read(), 950);
        assert_eq!(value.pcm_underflow_frames(), 10);
        assert_eq!(value.startup_trim_frames(), 3);
        assert_eq!(value.gradual_drift_trim_frames(), 1);
        assert_eq!(value.emergency_drift_trim_frames(), 0);
        assert!(value.take_dirty());
        assert!(!value.take_dirty());
    }
}
