//! Producer-local L1-Media observability.
//!
//! This telemetry mirrors Bridge/Scheduler outcomes only. It never controls
//! FrameId, StreamClock, retry policy, or HCI admission.

pub mod accumulator;
pub mod observation;
pub mod trace;

use crate::plane::l1_media::{
    bridge::scheduler::IntervalGroupSource,
    clock::frame::FrameId,
};
use accumulator::MediaBridgeTelemetryAccumulator;
pub use observation::MediaBridgeTelemetrySnapshot;
pub use trace::MediaBridgeTraceRecord;
use trace::MediaBridgeTrace;

#[derive(Default)]
pub struct MediaTelemetry {
    bridge: MediaBridgeTelemetryAccumulator,
    trace: MediaBridgeTrace,
}

impl MediaTelemetry {
    pub fn observe_submitted(&mut self, frame_id: FrameId, source: IntervalGroupSource, skipped_intervals: u64, stale_frames_dropped: usize) {
        self.bridge.observe_submitted(source, skipped_intervals, stale_frames_dropped);
        self.trace.push(MediaBridgeTraceRecord::Submitted { frame_id, source, skipped_intervals, stale_frames_dropped });
    }

    pub fn observe_dropped_backpressure(&mut self, frame_id: FrameId, source: IntervalGroupSource, skipped_intervals: u64, stale_frames_dropped: usize) {
        self.bridge.observe_dropped_backpressure(source, skipped_intervals, stale_frames_dropped);
        self.trace.push(MediaBridgeTraceRecord::DroppedBackpressure { frame_id, source, skipped_intervals, stale_frames_dropped });
    }

    pub fn bridge_snapshot(&self) -> MediaBridgeTelemetrySnapshot {
        MediaBridgeTelemetrySnapshot {
            submitted_intervals: self.bridge.submitted_intervals(),
            dropped_backpressure_intervals: self.bridge.dropped_backpressure_intervals(),
            fallback_intervals: self.bridge.fallback_intervals(),
            skipped_intervals: self.bridge.skipped_intervals(),
            stale_frames_dropped: self.bridge.stale_frames_dropped(),
            trace: self.trace.stats(),
        }
    }
    pub fn bridge_trace_snapshot(&self) -> Vec<MediaBridgeTraceRecord> { self.trace.snapshot() }
    pub fn take_bridge_dirty(&mut self) -> bool { self.bridge.take_dirty() }
}
