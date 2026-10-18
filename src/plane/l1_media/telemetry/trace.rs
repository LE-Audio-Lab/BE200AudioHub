use crate::{
    plane::l1_media::{
        bridge::scheduler::IntervalGroupSource,
        clock::frame::FrameId,
    },
    runtime::telemetry::{telemetry_ring, TelemetryRing, TelemetryRingStats},
};

pub const DEFAULT_MEDIA_BRIDGE_TRACE_CAPACITY: usize = 256;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_trace_is_bounded_and_keeps_recent_bridge_history() {
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
}
