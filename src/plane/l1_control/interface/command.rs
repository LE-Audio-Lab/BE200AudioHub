//! L0/Core -> L1-Control semantic command vocabulary.

use crate::{
    core::lifecycle::{SessionId, StreamId},
    plane::l1_control::engine::machine::ControlTimer,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlIntent {
    Start,
    Stop,
    BeginStreamHandoff {
        session_id: SessionId,
        stream_id: StreamId,
    },
    DiscoveryWindowElapsed,
    TimerExpired(ControlTimer),
}
