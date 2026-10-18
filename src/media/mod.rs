//! L1 Media Plane implementation.
//!
//! The first Review media slice is intentionally hardware-free: a deterministic
//! PCM producer feeds later codec/scheduler stages without knowing Bluetooth,
//! CIS handles, controller credits, or USB transport.

pub mod lc3;
pub mod synthetic;

