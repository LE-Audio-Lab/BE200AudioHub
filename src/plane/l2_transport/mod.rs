//! L2 Transport Plane.
//!
//! Owns asynchronous HCI packet movement, packet reassembly, cancellation,
//! transport backpressure, and transport faults. Bluetooth semantics stay in L1-HCI.

pub mod packet;
pub mod reactor;
pub mod sink;
pub mod worker;

pub mod interface;
