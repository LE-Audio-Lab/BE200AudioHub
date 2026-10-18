use crate::{
    core::lifecycle::LifecycleSnapshot,
    plane::{
        l1_control::interface::condition::ControlConditionUpdate,
        l1_hci::{
            interface::condition::HciConditionState,
            telemetry::HciIsoTelemetrySnapshot,
        },
        l1_media::{
            interface::condition::MediaConditionState,
            telemetry::MediaBridgeTelemetrySnapshot,
        },
        l2_transport::interface::condition::TransportConditionUpdate,
    },
    runtime::generation::Generation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HciConditionView {
    pub state: HciConditionState,
    pub iso_telemetry: Option<HciIsoTelemetrySnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaConditionView {
    pub state: MediaConditionState,
    pub bridge_telemetry: Option<MediaBridgeTelemetrySnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConditionSnapshot {
    pub epoch: u64,
    pub generation: Generation,
    pub lifecycle: LifecycleSnapshot,
    pub control: ControlConditionUpdate,
    pub hci: HciConditionView,
    pub media: MediaConditionView,
    pub transport: TransportConditionUpdate,
}
