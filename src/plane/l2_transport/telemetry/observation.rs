use crate::runtime::telemetry::TelemetryRingStats;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportTelemetrySnapshot {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub faults: u64,
    pub trace: TelemetryRingStats,
}
