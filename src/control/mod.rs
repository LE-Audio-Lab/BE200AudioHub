//! L1 Control Plane implementation.
//!
//! Control owns Bluetooth / LE Audio semantics and reduces typed lower-plane
//! reports into a logical stream world. It never owns raw USB transport, HCI
//! packet framing, controller ISO sequence numbers, or media timing.

pub(crate) mod audio_discovery;
pub(crate) mod audio_snapshot;
pub(crate) mod ascs_control;
pub(crate) mod codec_plan;
pub(crate) mod discovery;
pub(crate) mod enable_plan;
pub(crate) mod iso_plan;
pub(crate) mod machine;
pub(crate) mod plan;
pub(crate) mod qos_plan;
