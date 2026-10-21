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
            telemetry::MediaTelemetrySnapshot,
        },
        l2_transport::{
            interface::condition::TransportConditionState,
            telemetry::TransportTelemetrySnapshot,
        },
    },
    runtime::generation::Generation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HciConditionView {
    pub state: HciConditionState,
    pub iso_telemetry: Option<HciIsoTelemetrySnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaConditionView {
    pub state: MediaConditionState,
    pub telemetry: Option<MediaTelemetrySnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportConditionView {
    pub state: TransportConditionState,
    pub telemetry: Option<TransportTelemetrySnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConditionSnapshot {
    pub epoch: u64,
    pub generation: Generation,
    pub lifecycle: LifecycleSnapshot,
    pub control: ControlConditionUpdate,
    pub hci: HciConditionView,
    pub media: MediaConditionView,
    pub transport: TransportConditionView,
}
