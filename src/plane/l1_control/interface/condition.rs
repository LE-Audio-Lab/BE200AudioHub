//! Coarse L1-Control condition projection published upward.

use crate::core::lifecycle::StreamId;

use super::stream::LeAudioFormat;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlState {
    Idle,
    Resolving,
    Connecting,
    Securing,
    Discovering,
    Configuring,
    StreamReady,
    Streaming,
    TearingDown,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlConditionUpdate {
    pub state: ControlState,
    pub stream_id: Option<StreamId>,
    pub format: Option<LeAudioFormat>,
}

impl ControlConditionUpdate {
    pub const fn initial() -> Self {
        Self { state: ControlState::Idle, stream_id: None, format: None }
    }
}
