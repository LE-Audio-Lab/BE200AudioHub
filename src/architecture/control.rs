//! L1 — Control Plane.
//!
//! Owns LE Audio semantics: device/set identity, PACS/ASCS/CSIS/CAS, codec/QoS
//! planning, ASE state and semantic CIG/CIS/ISO-path lifecycle.
//! It is also the sole reducer/writer for RuntimeState.

use super::types::{SessionId, StreamId};

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

/// Logical stream topology exported by Control.
///
/// This is deliberately transport-neutral: it describes the audio shape, not
/// ASE/CIG/CIS/HCI identities. The current product stream is one logical stereo
/// stream split into left/right mono legs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamTopology {
    StereoSplit,
}

/// Stable Control -> Media / StreamClock / IsoTxBridge handoff.
///
/// SessionId and StreamId are owned/minted by L0 orchestration and supplied to
/// Control. No Bluetooth transport identity is allowed in this contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamBinding {
    pub session_id: SessionId,
    pub stream_id: StreamId,
    pub format: LeAudioFormat,
    pub presentation_delay_us: u32,
    pub topology: StreamTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlSnapshot {
    pub state: ControlState,
    pub stream_id: Option<StreamId>,
    pub format: Option<LeAudioFormat>,
}
