//! L2 — Transport Plane.
//!
//! Owns asynchronous HCI packet transport, endpoint packet reassembly,
//! cancellation and transport health. It understands HCI transport packet
//! classes but not LE Audio session semantics or controller credit policy.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportState {
    Detached,
    Starting,
    Ready,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciPacketClass {
    Command,
    Event,
    Acl,
    Iso,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportSnapshot {
    pub state: TransportState,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub faults: u64,
}
