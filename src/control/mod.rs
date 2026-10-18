//! L1 Control Plane implementation.
//!
//! Control owns Bluetooth / LE Audio semantics and reduces typed lower-plane
//! reports into a logical stream world. It never owns raw USB transport, HCI
//! packet framing, controller ISO sequence numbers, or media timing.

mod discovery;
mod plan;

pub(crate) use discovery::{
    resolve_connectable_members, ControlDiscoveryError,
    ResolvedMemberPresence,
};
pub(crate) use plan::{
    ControlMemberPlan, ControlPlanError, ControlSetPlan,
};
