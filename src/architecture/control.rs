//! Transitional coarse Control read projection.
//!
//! Logical stream contract types are owned by L1-Control/interface/stream.
//! This snapshot remains only until Core/Condition replaces old RuntimeState.

use super::types::StreamId;
use crate::plane::l1_control::interface::stream::LeAudioFormat;

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
pub struct ControlSnapshot {
    pub state: ControlState,
    pub stream_id: Option<StreamId>,
    pub format: Option<LeAudioFormat>,
}
