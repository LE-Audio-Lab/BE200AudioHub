//! Transport-neutral logical stream contract published by L1-Control.
//!
//! SessionId/StreamId are supplied by L0 orchestration. No ASE/CIG/CIS/raw HCI
//! identity or controller-capacity state is allowed in this boundary.

use crate::architecture::types::{SessionId, StreamId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeAudioFormat {
    pub sample_rate_hz: u32,
    pub frame_duration_us: u32,
    pub octets_per_sdu: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamTopology {
    StereoSplit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamBinding {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub format: LeAudioFormat,
    pub presentation_delay_us: u32,
    pub topology: StreamTopology,
}
