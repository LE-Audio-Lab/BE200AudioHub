//! Narrow product adapter over the pinned Bumble-RS host substrate.
//!
//! Do not expose Bumble-RS types throughout the product. This crate owns the
//! attach-without-reset patch seam and translates upstream events/types into
//! BE200AudioHub domain types.

mod init;
mod scan;

pub use init::{
    attach_be200_existing_operational, initialize_existing_operational, AttachReport,
    AttachedController,
};
pub use scan::{ScanObservation, ScanSummary};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_r2_attach_preserves_operational_controller() {
        assert_eq!(
            initial_attach_mode(),
            AttachMode::PreserveOperationalNoReset
        );
    }
}