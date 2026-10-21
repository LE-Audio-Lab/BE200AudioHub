//! L1-HCI condition updates.
//!
//! State changes are immediate. High-rate telemetry is published separately at
//! a bounded/coalesced cadence. Final combines both for fault/teardown flush.

use crate::{
    core::lifecycle::StreamId,
    plane::l1_hci::telemetry::HciIsoTelemetrySnapshot,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciHostState {
    Detached,
    Attaching,
    Ready,
    StreamBound,
    Streaming,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HciConditionState {
    pub state: HciHostState,
    pub stream_id: Option<StreamId>,
}

impl HciConditionState {
    pub const fn initial() -> Self {
        Self {
            state: HciHostState::Detached,
            stream_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciConditionUpdate {
    State(HciConditionState),
    Telemetry(HciIsoTelemetrySnapshot),
    Final {
        state: HciConditionState,
        iso_telemetry: HciIsoTelemetrySnapshot,
    },
}
