//! One nonblocking semantic Control transition exposed to Core composition.

use crate::{
    plane::l1_control::interface::{
        condition::ControlConditionUpdate,
        stream::StreamBinding,
    },
    plane::l1_control::engine::machine::ControlAction,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlStep {
    pub actions: Vec<ControlAction>,
    pub condition: ControlConditionUpdate,
    pub stream_binding: Option<StreamBinding>,
}
