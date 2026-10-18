//! L1 Control Plane implementation.
//!
//! Control owns Bluetooth / LE Audio semantics and reduces typed lower-plane
//! reports into a logical stream world. It never owns raw USB transport, HCI
//! packet framing, controller ISO sequence numbers, or media timing.

pub(crate) mod discovery;
pub(crate) mod machine;
pub(crate) mod plan;
