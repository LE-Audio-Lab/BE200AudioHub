//! L2-Transport condition updates.
//!
//! Semantic state and aggregate telemetry are separate so coalesced counters
//! cannot overwrite a newer transport state. Final combines both for
//! fault/teardown flush.

use crate::plane::l2_transport::telemetry::TransportTelemetrySnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportState {
    Detached,
    Starting,
    Ready,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportConditionState {
    pub state: TransportState,
}

impl TransportConditionState {
    pub const fn initial() -> Self {
        Self {
            state: TransportState::Detached,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportConditionUpdate {
    State(TransportConditionState),
    Telemetry(TransportTelemetrySnapshot),
    Final {
        state: TransportConditionState,
        telemetry: TransportTelemetrySnapshot,
    },
}

impl TransportConditionUpdate {
    pub const fn initial() -> Self {
        Self::State(TransportConditionState::initial())
    }
}
