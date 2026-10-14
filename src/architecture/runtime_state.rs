//! Control-owned runtime world model.
//!
//! Invariant: shared-readable, single-writer. Other planes report typed state
//! changes; only the Control Plane reduces those reports into RuntimeState.

use super::{
    control::ControlSnapshot,
    hci_host::HciHostSnapshot,
    lifecycle::LifecycleSnapshot,
    media::MediaSnapshot,
    transport::TransportSnapshot,
    types::Generation,
};

#[derive(Clone, Debug)]
pub struct RuntimeState {
    pub epoch: u64,
    pub generation: Generation,
    pub lifecycle: LifecycleSnapshot,
    pub control: ControlSnapshot,
    pub media: MediaSnapshot,
    pub hci: HciHostSnapshot,
    pub transport: TransportSnapshot,
}
