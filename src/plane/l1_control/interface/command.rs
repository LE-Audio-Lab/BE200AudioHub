//! L0/Core -> L1-Control semantic command vocabulary.

use crate::{
    core::lifecycle::{SessionId, StreamId},
    plane::l1_control::engine::machine::{ControlTimer, TeardownPolicy},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlIntent {
    Start,
    Stop { policy: TeardownPolicy },
    BeginStreamHandoff {
        session_id: SessionId,
        stream_id: StreamId,
    },
    DiscoveryWindowElapsed,
    TimerExpired(ControlTimer),
}
