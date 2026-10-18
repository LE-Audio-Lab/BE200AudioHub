//! L1 Control Plane.
//!
//! Owns LE Audio semantic planning, discovery, ASCS operations, and the tested
//! ControlMachine. Behavioral decomposition is deferred until after V2 structure lands.

pub mod ascs;
pub mod discovery;
pub mod engine;
pub mod interface;
pub mod model;
pub mod operation;
pub mod planning;
