use crate::runtime::telemetry::TelemetryRingStats;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HciIsoTelemetrySnapshot {
    pub controller_limit: Option<usize>,
    pub submitted_packets: u64,
    pub completed_packets: u64,
    /// Packets removed from accounting by teardown. Retired packets are not
    /// controller completion evidence and are deliberately counted separately.
    pub retired_packets: u64,
    pub in_flight_packets: usize,
    pub peak_in_flight_packets: usize,
    pub trace: TelemetryRingStats,
}
