//! Producer-local L1-Media observability.
//!
//! Bridge/Scheduler outcomes and Media timing/PCM observations are aggregated
//! locally. Raw traces are bounded diagnostics only; none of this telemetry
//! controls FrameId, StreamClock, retry policy, or HCI admission.

pub mod accumulator;
pub mod observation;
pub mod trace;

use crate::plane::l1_media::{
    bridge::scheduler::IntervalGroupSource,
    clock::frame::FrameId,
    interface::condition::StreamClockSnapshot,
    pcm::relay_boundary::PcmConsumeResult,
};
use accumulator::{
    MediaBridgeTelemetryAccumulator, MediaTimingTelemetryAccumulator,
};
pub use observation::{
    MediaBridgeTelemetrySnapshot, MediaTelemetrySnapshot,
    MediaTimingTelemetrySnapshot,
};
pub use trace::{MediaBridgeTraceRecord, MediaTimingAnomaly};
use trace::{MediaBridgeTrace, MediaTimingTrace};

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

    pub fn bridge_trace_snapshot(&self) -> Vec<MediaBridgeTraceRecord> {
        self.trace.snapshot()
    }

    pub fn take_bridge_dirty(&mut self) -> bool {
        self.bridge.take_dirty()
    }
}

#[derive(Default)]
pub struct MediaTimingTelemetry {
    timing: MediaTimingTelemetryAccumulator,
    trace: MediaTimingTrace,
}

impl MediaTimingTelemetry {
    pub fn observe_consume(&mut self, result: PcmConsumeResult) {
        self.timing.observe_consume(
            result.requested_frames,
            result.read_frames,
            result.keepalive,
            result.startup_trim_frames,
            result.drift_trim.gradual_frames,
            result.drift_trim.emergency_frames,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn observe_interval(
        &mut self,
        frame_id: FrameId,
        at_us: u64,
        clock: StreamClockSnapshot,
        pcm_fill_frames: usize,
        source_intervals_skipped: u64,
        bridge_intervals_skipped: u64,
        stale_frames_dropped: usize,
        pcm: PcmConsumeResult,
    ) {
        self.timing.observe_interval_context(
            frame_id,
            clock,
            pcm_fill_frames,
            source_intervals_skipped,
        );

        let underflow_frames =
            pcm.requested_frames.saturating_sub(pcm.read_frames);
        let anomalous = !pcm.keepalive
            && (underflow_frames > 0
                || source_intervals_skipped > 0
                || bridge_intervals_skipped > 0
                || stale_frames_dropped > 0
                || pcm.drift_trim.gradual_frames > 0
                || pcm.drift_trim.emergency_frames > 0);

        if anomalous {
            self.trace.push(MediaTimingAnomaly {
                frame_id,
                at_us,
                pcm_fill_frames,
                underflow_frames,
                source_intervals_skipped,
                bridge_intervals_skipped,
                stale_frames_dropped,
                startup_trim_frames: pcm.startup_trim_frames,
                gradual_drift_trim_frames: pcm.drift_trim.gradual_frames,
                emergency_drift_trim_frames: pcm.drift_trim.emergency_frames,
            });
        }
    }

    pub fn snapshot(&self) -> MediaTimingTelemetrySnapshot {
        MediaTimingTelemetrySnapshot {
            produced_frame: self.timing.produced_frame(),
            clock: self.timing.clock(),
            pcm_fill_frames: self.timing.pcm_fill_frames(),
            intervals_consumed: self.timing.intervals_consumed(),
            source_intervals_skipped: self.timing.source_intervals_skipped(),
            keepalive_intervals: self.timing.keepalive_intervals(),
            pcm_frames_read: self.timing.pcm_frames_read(),
            pcm_underflow_frames: self.timing.pcm_underflow_frames(),
            startup_trim_frames: self.timing.startup_trim_frames(),
            gradual_drift_trim_frames: self.timing.gradual_drift_trim_frames(),
            emergency_drift_trim_frames: self.timing.emergency_drift_trim_frames(),
            trace: self.trace.stats(),
        }
    }

    pub fn trace_snapshot(&self) -> Vec<MediaTimingAnomaly> {
        self.trace.snapshot()
    }

    pub fn take_dirty(&mut self) -> bool {
        self.timing.take_dirty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        plane::l1_media::{
            interface::condition::ClockLock,
            pcm::drift::DriftTrimResult,
        },
        runtime::telemetry::TelemetryRingStats,
    };

    fn clock(frame: u64) -> StreamClockSnapshot {
        StreamClockSnapshot {
            period_us: 10_000,
            current_frame: FrameId(frame),
            lock: ClockLock::LocalMonotonic,
            rate_error_ppm: 0,
        }
    }

    fn pcm(
        read_frames: usize,
        keepalive: bool,
        startup_trim_frames: usize,
        gradual: usize,
        emergency: usize,
    ) -> PcmConsumeResult {
        PcmConsumeResult {
            requested_frames: 480,
            read_frames,
            keepalive,
            startup_trim_frames,
            drift_trim: DriftTrimResult {
                gradual_frames: gradual,
                emergency_frames: emergency,
            },
        }
    }

    #[test]
    fn timing_ring_records_anomalies_but_not_clean_or_startup_only_intervals() {
        let mut telemetry = MediaTimingTelemetry::default();

        let startup_only = pcm(480, false, 480, 0, 0);
        telemetry.observe_consume(startup_only);
        telemetry.observe_interval(
            FrameId(0),
            20_000,
            clock(0),
            480,
            0,
            0,
            0,
            startup_only,
        );
        assert!(telemetry.trace_snapshot().is_empty());

        let underflow = pcm(470, false, 0, 0, 0);
        telemetry.observe_consume(underflow);
        telemetry.observe_interval(
            FrameId(1),
            30_000,
            clock(1),
            470,
            0,
            0,
            0,
            underflow,
        );

        let records = telemetry.trace_snapshot();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].frame_id, FrameId(1));
        assert_eq!(records[0].underflow_frames, 10);
        assert_eq!(
            telemetry.snapshot().trace,
            TelemetryRingStats {
                capacity: 256,
                len: 1,
                stored: 1,
                dropped_newest: 0,
                overwritten_oldest: 0,
            }
        );
    }

    #[test]
    fn timing_dirty_is_coalesced_without_resetting_aggregate_state() {
        let mut telemetry = MediaTimingTelemetry::default();
        let result = pcm(480, false, 0, 0, 0);
        telemetry.observe_consume(result);
        telemetry.observe_interval(
            FrameId(3),
            50_000,
            clock(3),
            480,
            2,
            1,
            0,
            result,
        );

        assert!(telemetry.take_dirty());
        assert!(!telemetry.take_dirty());
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.produced_frame, Some(FrameId(3)));
        assert_eq!(snapshot.source_intervals_skipped, 2);
        assert_eq!(snapshot.intervals_consumed, 1);
    }
}
