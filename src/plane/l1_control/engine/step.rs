//! One nonblocking semantic Control transition exposed to Core composition.

use crate::{
    architecture::control::ControlSnapshot,
    plane::l1_control::interface::stream::StreamBinding,
    plane::l1_control::engine::machine::ControlAction,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlStep {
    pub actions: Vec<ControlAction>,
    pub snapshot: ControlSnapshot,
    pub stream_binding: Option<StreamBinding>,
}
