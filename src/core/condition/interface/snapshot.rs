use crate::{
    core::lifecycle::LifecycleSnapshot,
    plane::{
        l1_control::interface::condition::ControlConditionUpdate,
        l1_hci::interface::condition::HciConditionUpdate,
        l1_media::interface::condition::MediaConditionUpdate,
        l2_transport::interface::condition::TransportConditionUpdate,
    },
    runtime::generation::Generation,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConditionSnapshot {
    pub epoch: u64,
    pub generation: Generation,
    pub lifecycle: LifecycleSnapshot,
    pub control: ControlConditionUpdate,
    pub hci: HciConditionUpdate,
    pub media: MediaConditionUpdate,
    pub transport: TransportConditionUpdate,
}
