//! Control-owned runtime world model.
//!
//! Invariant: shared-readable, single-writer. Other planes report typed state
//! changes; only the Control Plane reduces those reports into RuntimeState.

use super::{
    control::{ControlSnapshot, ControlState},
    hci_host::{HciHostSnapshot, HciHostState},
    lifecycle::{LifecycleSnapshot, LifecycleState, SessionState},
    media::{MediaSnapshot, MediaState},
    transport::{TransportSnapshot, TransportState},
};
use crate::runtime::generation::Generation;

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeState {
    /// Monotonic observation-model revision. It advances only when a reduced
    /// projection actually changes.
    pub epoch: u64,
    pub generation: Generation,
    pub lifecycle: LifecycleSnapshot,
    pub control: ControlSnapshot,
    pub media: MediaSnapshot,
    pub hci: HciHostSnapshot,
    pub transport: TransportSnapshot,
}

impl RuntimeState {
    pub fn initial(generation: Generation) -> Self {
        Self {
            epoch: 0,
            generation,
            lifecycle: LifecycleSnapshot {
                generation,
                session_id: None,
                lifecycle: LifecycleState::Stopped,
                session: SessionState::Idle,
            },
            control: ControlSnapshot {
                state: ControlState::Idle,
                stream_id: None,
                format: None,
            },
            media: MediaSnapshot {
                state: MediaState::Stopped,
                produced_frame: None,
                source_ring_fill_frames: 0,
                encoded_ring_fill_frames: 0,
                asrc_ratio: 1.0,
            },
            hci: HciHostSnapshot {
                state: HciHostState::Detached,
                stream_id: None,
                controller_iso_capacity: 0,
                controller_iso_in_flight: 0,
            },
            transport: TransportSnapshot {
                state: TransportState::Detached,
                rx_packets: 0,
                tx_packets: 0,
                faults: 0,
            },
        }
    }
}
