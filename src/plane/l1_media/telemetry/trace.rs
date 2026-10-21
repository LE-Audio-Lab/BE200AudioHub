use crate::{
    plane::l1_media::{
        bridge::scheduler::IntervalGroupSource,
        clock::frame::FrameId,
    },
    runtime::telemetry::{telemetry_ring, TelemetryRing, TelemetryRingStats},
};

pub const DEFAULT_MEDIA_BRIDGE_TRACE_CAPACITY: usize = 256;
pub const DEFAULT_MEDIA_TIMING_TRACE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaBridgeTraceRecord {
    Submitted {
        frame_id: FrameId,
        source: IntervalGroupSource,
        skipped_intervals: u64,
        stale_frames_dropped: usize,
    },
    DroppedBackpressure {
        frame_id: FrameId,
        source: IntervalGroupSource,
        skipped_intervals: u64,
        stale_frames_dropped: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaTimingAnomaly {
    pub frame_id: FrameId,
    pub at_us: u64,
    pub pcm_fill_frames: usize,
    pub underflow_frames: usize,
    pub source_intervals_skipped: u64,
    pub bridge_intervals_skipped: u64,
    pub stale_frames_dropped: usize,
    pub startup_trim_frames: usize,
    pub gradual_drift_trim_frames: usize,
    pub emergency_drift_trim_frames: usize,
}

pub struct MediaBridgeTrace {
    ring: TelemetryRing<MediaBridgeTraceRecord>,
}

impl Default for MediaBridgeTrace {
    fn default() -> Self { Self::with_capacity(DEFAULT_MEDIA_BRIDGE_TRACE_CAPACITY) }
}

impl MediaBridgeTrace {
    pub fn with_capacity(capacity: usize) -> Self {
        Self { ring: telemetry_ring(capacity).expect("Media bridge trace capacity must be non-zero") }
    }
    pub fn push(&mut self, record: MediaBridgeTraceRecord) { let _ = self.ring.push(record); }
    pub fn stats(&self) -> TelemetryRingStats { self.ring.stats() }
    pub fn snapshot(&self) -> Vec<MediaBridgeTraceRecord> { self.ring.snapshot() }
}

pub struct MediaTimingTrace {
    ring: TelemetryRing<MediaTimingAnomaly>,
}

impl Default for MediaTimingTrace {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_MEDIA_TIMING_TRACE_CAPACITY)
    }
}

impl MediaTimingTrace {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            ring: telemetry_ring(capacity)
                .expect("Media timing trace capacity must be non-zero"),
        }
    }

    pub fn push(&mut self, record: MediaTimingAnomaly) {
        let _ = self.ring.push(record);
    }

    pub fn stats(&self) -> TelemetryRingStats {
        self.ring.stats()
    }

    pub fn snapshot(&self) -> Vec<MediaTimingAnomaly> {
        self.ring.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_raw_trace_is_bounded_and_keeps_recent_history() {
        let mut trace = MediaBridgeTrace::with_capacity(1);
        trace.push(MediaBridgeTraceRecord::Submitted {
            frame_id: FrameId(1), source: IntervalGroupSource::Encoded,
            skipped_intervals: 0, stale_frames_dropped: 0,
        });
        trace.push(MediaBridgeTraceRecord::DroppedBackpressure {
            frame_id: FrameId(2), source: IntervalGroupSource::Fallback,
            skipped_intervals: 1, stale_frames_dropped: 1,
        });
        assert!(matches!(trace.snapshot()[0], MediaBridgeTraceRecord::DroppedBackpressure { .. }));
        assert_eq!(trace.stats().overwritten_oldest, 1);
    }

    #[test]
    fn timing_raw_trace_is_bounded_and_keeps_latest_anomalies() {
        let mut trace = MediaTimingTrace::with_capacity(1);
        let record = |frame_id| MediaTimingAnomaly {
            frame_id: FrameId(frame_id),
            at_us: frame_id * 10_000,
            pcm_fill_frames: 480,
            underflow_frames: 1,
            source_intervals_skipped: 0,
            bridge_intervals_skipped: 0,
            stale_frames_dropped: 0,
            startup_trim_frames: 0,
            gradual_drift_trim_frames: 0,
            emergency_drift_trim_frames: 0,
        };
        trace.push(record(1));
        trace.push(record(2));

        assert_eq!(trace.snapshot().len(), 1);
        assert_eq!(trace.snapshot()[0].frame_id, FrameId(2));
        assert_eq!(trace.stats().overwritten_oldest, 1);
    }
}
