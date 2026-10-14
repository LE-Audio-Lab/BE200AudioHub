//! L0 — LifeCycle / Orchestration.
//!
//! Owns desired state, generation, session lifetime, recovery policy and
//! ordered startup/teardown. It does not own Bluetooth packet semantics.

use super::types::{Generation, SessionId};

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
pub enum RecoveryClass {
    RestartMedia,
    RebuildStream,
    RestartController,
    RestartWorker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecycleSnapshot {
    pub generation: Generation,
    pub session_id: Option<SessionId>,
    pub lifecycle: LifecycleState,
    pub session: SessionState,
}
