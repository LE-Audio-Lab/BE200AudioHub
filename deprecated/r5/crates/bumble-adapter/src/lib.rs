//! Product adapter over the pinned Bumble-RS host substrate.
//!
//! Public API boundaries are intentionally narrow:
//! - scan/attach below ACL establishment;
//! - `LeConnectionManager` creates one ACL;
//! - `LeBondManager` creates provisional peer security material;
//! - CSIS verifies coordinated-set membership;
//! - commit functions are the only persistent write boundary;
//! - stored-security recovery never starts SMP;
//! - encrypted GATT access stays behind one ownership bridge.

#[macro_use]
mod console_ascii;
mod acl_session;
mod bond;
mod commit;
mod connection_manager;
mod csis;
mod gatt_session;
mod cig_control;
mod cis_control;
mod cis_host_feature;
mod init;
mod iso_capabilities;
mod iso_audit;
mod iso_io;
mod probe_guard;
mod privacy;
mod scan;
mod security;
mod set_member_verifier;
mod stored_security;
mod usb_fixed;

pub use acl_session::LeAclReport;
pub use cig_control::{CigAllocation, CigSetupFailure, CisHandleAllocation};
pub use cis_control::EstablishedCis;
pub use cis_host_feature::CisHostFeatureReport;
pub use bond::{BondMemberSummary, BondSetSummary, BondStore, ControllerIdentity};
pub use commit::{
    commit_primary_bond, commit_verified_set_member, CommittedBondReport,
};
pub use connection_manager::LeConnectionManager;
pub use csis::{rsi_matches_sirk, CsisMemberReport};
pub use gatt_session::{AttValueEvent, LeAclState};
pub use iso_capabilities::IsoControllerCapabilities;
pub use iso_audit::IsoTxStats;
pub use iso_io::{IsoPathFailure, IsoStreamReport};
pub use probe_guard::ProbeGuard;
pub use init::{
    attach_be200_existing_operational, attach_be200_existing_operational_raw_trace,
    initialize_existing_operational, AttachReport, AttachedController,
};
pub use privacy::BondedPeerIdentity;
pub use scan::{ScanObservation, ScanSummary};
pub use security::{LeBondManager, ProvisionalBond};
pub use set_member_verifier::SetMemberVerification;

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
