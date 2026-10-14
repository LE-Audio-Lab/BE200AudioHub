//! L1 — Control Plane.
//!
//! Owns LE Audio semantics: device/set identity, PACS/ASCS/CSIS/CAS, codec/QoS
//! planning, ASE state and semantic CIG/CIS/ISO-path lifecycle.
//! It is also the sole reducer/writer for RuntimeState.

use super::types::StreamId;

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
pub struct LeAudioFormat {
    pub sample_rate_hz: u32,
    pub frame_duration_us: u32,
    pub octets_per_sdu: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlSnapshot {
    pub state: ControlState,
    pub stream_id: Option<StreamId>,
    pub format: Option<LeAudioFormat>,
}
