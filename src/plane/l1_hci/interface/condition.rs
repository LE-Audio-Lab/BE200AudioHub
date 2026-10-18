//! Coarse L1-HCI condition projection.
//!
//! Raw HCI events and raw telemetry trace records never cross this boundary.

use crate::{
    core::lifecycle::StreamId,
    plane::l1_hci::telemetry::HciIsoTelemetrySnapshot,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciHostState {
    Detached,
    Ready,
    StreamBound,
    Streaming,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HciConditionUpdate {
    pub state: HciHostState,
    pub stream_id: Option<StreamId>,
    pub controller_iso_capacity: u16,
    pub controller_iso_in_flight: u16,
    pub iso_telemetry: Option<HciIsoTelemetrySnapshot>,
}

impl HciConditionUpdate {
    pub const fn initial() -> Self {
        Self {
            state: HciHostState::Detached,
            stream_id: None,
            controller_iso_capacity: 0,
            controller_iso_in_flight: 0,
            iso_telemetry: None,
        }
    }
}
