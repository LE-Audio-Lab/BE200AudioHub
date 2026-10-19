use crossbeam_channel::{Receiver, TryRecvError};

use crate::{
    plane::l1_hci::{
        acl::{
            connection::{
                DisconnectState, FalConnectionState, LeConnectionSetSnapshot,
            },
            credit::AclCreditSnapshot,
            privacy::ControllerPrivacyState,
            scan::ScanSummary,
            security::StoredEncryptionState,
        },
        gatt::transaction::GattReport,
        host::engine::{
            AclLlFeaturesProbeResult, AclRssiProbeResult,
            ControllerCapabilities, ControllerIdentity,
            ControllerIdentityFailure, OperationalAttachState, PassiveScanState,
        },
        iso::{
            cig_cis::{CigConfigureState, CisCreateState, SemanticCisCreateState},
            credit::IsoCreditSnapshot,
            feature::CisHostFeatureState,
            lifecycle::{
                CigRemoveState, CisDisconnectState, SemanticCisDisconnectState,
            },
            path::{IsoPathOperationState, SemanticIsoPathState},
            route::StreamRouteState,
            tx::IsoDrainState,
        },
    },
    runtime::mailbox::RecvError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HciReport {
    AttachState(OperationalAttachState),
    CapabilitiesReady(ControllerCapabilities),
    ControllerIdentity(
        Result<ControllerIdentity, ControllerIdentityFailure>,
    ),
    AclRssiProbe(AclRssiProbeResult),
    AclLlFeaturesProbe(AclLlFeaturesProbeResult),
    PassiveScanState(PassiveScanState),
    ScanComplete(ScanSummary),
    ControllerPrivacyState(ControllerPrivacyState),
    FalConnectionState(FalConnectionState),
    DisconnectState(DisconnectState),
    StoredEncryptionState(StoredEncryptionState),
    CisHostFeatureState(CisHostFeatureState),
    AclTxAudit(AclCreditSnapshot),
    IsoTxAudit(IsoCreditSnapshot),
    CigConfigureState(CigConfigureState),
    CisCreateState(CisCreateState),
    SemanticCisCreateState(SemanticCisCreateState),
    CisDisconnectState(CisDisconnectState),
    SemanticCisDisconnectState(SemanticCisDisconnectState),
    CigRemoveState(CigRemoveState),
    IsoPathState(IsoPathOperationState),
    SemanticIsoPathState(SemanticIsoPathState),
    StreamRouteState(StreamRouteState),
    IsoDrainState(IsoDrainState),
    ConnectionSet(LeConnectionSetSnapshot),
    Gatt(GattReport),
}


pub struct HciReportInbox {
    rx: Receiver<HciReport>,
}

impl HciReportInbox {
    pub(crate) fn new(rx: Receiver<HciReport>) -> Self {
        Self { rx }
    }


    pub fn try_recv(&self) -> Result<HciReport, RecvError> {
        self.rx.try_recv().map_err(|error| match error {
            TryRecvError::Empty => RecvError::Empty,
            TryRecvError::Disconnected => RecvError::Disconnected,
        })
    }
}


