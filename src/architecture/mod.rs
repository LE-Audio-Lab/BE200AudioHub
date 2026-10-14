//! Review architecture contracts.
//!
//! This module intentionally defines ownership and data-flow boundaries only.
//! Threading, async runtime, queue implementation and scheduling primitives are
//! deferred to the next Review step.

pub mod control;
pub mod hci_host;
pub mod iso_tx_bridge;
pub mod lifecycle;
pub mod media;
pub mod platform;
pub mod runtime_state;
pub mod stream_clock;
pub mod transport;
pub mod types;
