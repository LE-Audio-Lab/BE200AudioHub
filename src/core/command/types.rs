use crate::{
    core::lifecycle::{SessionId, StreamId},
    plane::l1_control::engine::machine::TeardownPolicy,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreCommand {
    StartControl,
    StopControl { policy: TeardownPolicy },
    BeginStreamHandoff {
        session_id: SessionId,
        stream_id: StreamId,
    },
}
