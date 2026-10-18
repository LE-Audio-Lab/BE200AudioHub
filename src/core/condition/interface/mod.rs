pub mod snapshot;

use crate::{
    core::lifecycle::{SessionId, LifecycleSnapshot},
    runtime::generation::Generation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConditionScope {
    pub generation: Generation,
    pub session_id: Option<SessionId>,
}

impl ConditionScope {
    pub const fn runtime(generation: Generation) -> Self {
        Self { generation, session_id: None }
    }
    pub const fn session(generation: Generation, session_id: SessionId) -> Self {
        Self { generation, session_id: Some(session_id) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConditionChange {
    pub changed: bool,
    pub stale: bool,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartControlAdmission { Allowed, ControlNotIdle }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopControlAdmission {
    Allowed,
    ControlIdle,
    ControlNotBound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamHandoffAdmission {
    Allowed,
    SessionMismatch,
    ControlNotReady,
    StreamAlreadyPresent,
}

pub type RuntimeStatus = LifecycleSnapshot;
