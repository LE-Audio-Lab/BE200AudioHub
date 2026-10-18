//! Runtime diagnostics primitives.

pub mod ring;

pub use ring::{
    telemetry_ring, TelemetryOverflowPolicy, TelemetryPushOutcome,
    TelemetryRing, TelemetryRingError, TelemetryRingStats,
};
