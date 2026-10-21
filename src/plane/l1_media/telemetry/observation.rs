use crate::{
    plane::l1_media::{
        clock::frame::FrameId,
        interface::condition::StreamClockSnapshot,
    },
    runtime::telemetry::TelemetryRingStats,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaBridgeTelemetrySnapshot {
    pub submitted_intervals: u64,
    pub dropped_backpressure_intervals: u64,
    pub fallback_intervals: u64,
    pub skipped_intervals: u64,
    pub stale_frames_dropped: u64,
    pub trace: TelemetryRingStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaTimingTelemetrySnapshot {
    pub produced_frame: Option<FrameId>,
    pub clock: Option<StreamClockSnapshot>,
    pub pcm_fill_frames: usize,
    pub intervals_consumed: u64,
    pub source_intervals_skipped: u64,
    pub keepalive_intervals: u64,
    pub pcm_frames_read: u64,
    pub pcm_underflow_frames: u64,
    pub startup_trim_frames: u64,
    pub gradual_drift_trim_frames: u64,
    pub emergency_drift_trim_frames: u64,
    pub trace: TelemetryRingStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaTelemetrySnapshot {
    pub timing: MediaTimingTelemetrySnapshot,
    pub bridge: MediaBridgeTelemetrySnapshot,
}
