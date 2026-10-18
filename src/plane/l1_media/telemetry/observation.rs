use crate::runtime::telemetry::TelemetryRingStats;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaBridgeTelemetrySnapshot {
    pub submitted_intervals: u64,
    pub dropped_backpressure_intervals: u64,
    pub fallback_intervals: u64,
    pub skipped_intervals: u64,
    pub stale_frames_dropped: u64,
    pub trace: TelemetryRingStats,
}
