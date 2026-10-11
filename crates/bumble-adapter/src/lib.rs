//! Narrow product adapter over the pinned Bumble-RS host substrate.
//!
//! Do not expose Bumble-RS types throughout the product. This crate will own
//! the attach-without-reset patch seam and translate upstream events/types into
//! BE200AudioHub domain types.

pub const UPSTREAM_REVISION: &str =
    "bbac2a6803b8cab0920ab725a23aa408fc4fed85";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachMode {
    /// Initial R2 mode: controller was already brought to Operational by R1.
    PreserveOperationalNoReset,
    /// Future mode after a dedicated HCI Reset safety experiment.
    StandardHostReset,
}

pub fn initial_attach_mode() -> AttachMode {
    AttachMode::PreserveOperationalNoReset
}
