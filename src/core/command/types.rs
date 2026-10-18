use crate::core::lifecycle::{SessionId, StreamId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreCommand {
    StartControl,
    BeginStreamHandoff {
        session_id: SessionId,
        stream_id: StreamId,
    },
}
