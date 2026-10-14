//! L1 — HCI Host Plane.
//!
//! Sole owner of controller-facing Bluetooth Host state. It consumes encoded
//! SDUs and owns transport sequence numbers, ISO framing/fragmentation,
//! per-CIS queues, controller credits and completion accounting.
//!
//! Controller completions release transport capacity; they never advance media
//! time.

use super::types::StreamId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciHostState {
    Detached,
    Ready,
    StreamBound,
    Streaming,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HciHostSnapshot {
    pub state: HciHostState,
    pub stream_id: Option<StreamId>,
    pub controller_iso_capacity: u16,
    pub controller_iso_in_flight: u16,
}
