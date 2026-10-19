use crate::{
    core::lifecycle::{SessionId, StreamId},
    plane::{
        l1_control::{
            ascs::operation::AseOperationMachine,
            engine::machine::{
                ControlCleanupFailure, GracefulAseReleaseMode,
            },
            interface::stream::AudioSide,
        },
        l1_hci::{
            acl::connection::ConnectionToken,
            iso::cig_cis::CisRef,
        },
    },
};

/// Temporary progress for one Control rollback/teardown sequence.
///
/// This owns rollback cursors and accumulated cleanup failures only.
/// Ordering and fail-closed decisions remain in the Control engine.
pub(crate) struct RollbackOperation {
    pub(crate) acl_queue: Vec<ConnectionToken>,
    pub(crate) acl_current: Option<ConnectionToken>,
    pub(crate) cleanup_failures: Vec<ControlCleanupFailure>,
    pub(crate) cig_pending: bool,
    pub(crate) stream_route: Option<(SessionId, StreamId)>,
    pub(crate) stream_route_unbind_pending: bool,
    pub(crate) iso_path_queue: Vec<CisRef>,
    pub(crate) iso_path_current: Option<CisRef>,
    pub(crate) cis_queue: Vec<CisRef>,
    pub(crate) cis_current: Option<CisRef>,
    pub(crate) ase_queue: Vec<AudioSide>,
    pub(crate) ase_current: Option<AudioSide>,
    pub(crate) ase_operation: Option<AseOperationMachine>,
    pub(crate) graceful_ase_release_mode: GracefulAseReleaseMode,
    pub(crate) ase_group_operations: [Option<AseOperationMachine>; 2],
}

impl RollbackOperation {
    pub(crate) fn new() -> Self {
        Self {
            acl_queue: Vec::new(),
            acl_current: None,
            cleanup_failures: Vec::new(),
            cig_pending: false,
            stream_route: None,
            stream_route_unbind_pending: false,
            iso_path_queue: Vec::new(),
            iso_path_current: None,
            cis_queue: Vec::new(),
            cis_current: None,
            ase_queue: Vec::new(),
            ase_current: None,
            ase_operation: None,
            graceful_ase_release_mode: GracefulAseReleaseMode::Serial,
            ase_group_operations: [None, None],
        }
    }
}
