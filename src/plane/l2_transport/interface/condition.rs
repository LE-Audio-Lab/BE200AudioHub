//! Coarse L2-Transport condition projection.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportState {
    Detached,
    Starting,
    Ready,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportConditionUpdate {
    pub state: TransportState,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub faults: u64,
}

impl TransportConditionUpdate {
    pub const fn initial() -> Self {
        Self { state: TransportState::Detached, rx_packets: 0, tx_packets: 0, faults: 0 }
    }
}
