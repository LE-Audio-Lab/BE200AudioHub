//! Narrow product adapter over the pinned Bumble-RS host substrate.
//!
//! Do not expose Bumble-RS types throughout the product. This crate owns the
//! attach-without-reset patch seam and translates upstream events/types into
//! BE200AudioHub domain types.

#[macro_use]
mod console_ascii;
mod bond;
mod bond_commit;
mod bond_manager_v4;
mod connect;
mod connection_manager;
mod csis;
mod init;
mod pairing;
mod primary_commit;
mod scan;
mod set_member_verifier;
mod usb_fixed;

pub use bond::{
    BondMemberSummary, BondSetSummary, BondStore, ControllerIdentity,
};
pub use bond_commit::{commit_verified_set_member, CommittedBondReport};
pub use bond_manager_v4::{LeBondManager, ProvisionalBond};
pub use connect::LeAclReport;
pub use connection_manager::LeConnectionManager;
pub use csis::{rsi_matches_sirk, CsisMemberReport};
pub use init::{
    attach_be200_existing_operational, attach_be200_existing_operational_raw_trace,
    initialize_existing_operational, AttachReport, AttachedController,
};
pub use pairing::{BondResumeReport, PairPersistReport};
pub use primary_commit::commit_primary_bond;
pub use scan::{ScanObservation, ScanSummary};
pub use set_member_verifier::{require_verified, SetMemberVerification};

pub const UPSTREAM_REVISION: &str =
    "bbac2a6803b8cab0920ab725a23aa408fc4fed85";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachMode {
    PreserveOperationalNoReset,
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
