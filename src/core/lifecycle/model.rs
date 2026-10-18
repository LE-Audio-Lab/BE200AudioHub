use crate::runtime::generation::Generation;

use super::identity::SessionId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleState {
    Stopped,
    Starting,
    Waiting,
    Running,
    Recovering,
    Stopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Preparing,
    Priming,
    Streaming,
    Draining,
    TearingDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecycleSnapshot {
    pub generation: Generation,
    pub session_id: Option<SessionId>,
    pub lifecycle: LifecycleState,
    pub session: SessionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecycleModel {
    generation: Generation,
    session_id: Option<SessionId>,
    lifecycle: LifecycleState,
    session: SessionState,
}

impl LifecycleModel {
    pub const fn stopped(generation: Generation) -> Self {
        Self {
            generation,
            session_id: None,
            lifecycle: LifecycleState::Stopped,
            session: SessionState::Idle,
        }
    }

    pub const fn snapshot(self) -> LifecycleSnapshot {
        LifecycleSnapshot {
            generation: self.generation,
            session_id: self.session_id,
            lifecycle: self.lifecycle,
            session: self.session,
        }
    }

    pub const fn generation(self) -> Generation { self.generation }
    pub const fn session_id(self) -> Option<SessionId> { self.session_id }
    pub const fn lifecycle(self) -> LifecycleState { self.lifecycle }
    pub const fn session(self) -> SessionState { self.session }
}
