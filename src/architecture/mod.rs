//! Review architecture contracts.
//!
//! These modules freeze ownership, data-flow and execution-context semantics.
//! Concrete Rust executors, channel crates, OS synchronization primitives and
//! queue capacities remain implementation decisions.

pub mod control;
pub mod execution;
pub mod hci_host;
pub mod iso_tx_bridge;
pub mod lifecycle;
pub mod mailbox;
pub mod media;
pub mod platform;
pub mod runtime_state;
pub mod stream_clock;
pub mod transport;
pub mod types;
