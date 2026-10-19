use crossbeam_channel::{Sender, TrySendError};

use crate::{
    core::lifecycle::{SessionId, StreamId},
    plane::l1_hci::{
        acl::{
            connection::{ConnectionToken, FalConnectRequest},
            privacy::ResolvingListConfig,
            security::StoredEncryptionRequest,
        },
        gatt::transaction::{GattRequest, GattRequestId},
        iso::{
            cig_cis::{CigConfig, CisCreateRequest, CisRef},
            path::IsoDataPathConfig,
            route::StereoStreamRouteBinding,
            tx::{IsoDataGroupRequest, IsoDrainRequest},
        },
        interface::iso_tx::{HciIsoTx, HciLogicalIsoTx},
    },
    runtime::mailbox::SendError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HciCommand {
    BeginOperationalAttach,
    StartPassiveExtendedScan,
    StopPassiveExtendedScan,
    ReadControllerIdentity,
    ReadAclRssiProbe { connection: ConnectionToken },
    ReadAclLlFeaturesProbe { connection: ConnectionToken },
    ConfigureControllerPrivacy(ResolvingListConfig),
    ClearControllerPrivacy,
    BeginFalConnection(FalConnectRequest),
    CancelFalConnection,
    Disconnect { connection: ConnectionToken },
    BeginStoredEncryption(StoredEncryptionRequest),
    EnsureCisHostFeature,
    ReadAclTxAudit,
    ReadAclRuntimeSnapshot,
    ReadCisRuntimeSnapshot,
    ReadIsoTxAudit,
    ConfigureCig(CigConfig),
    CreateCis(CisCreateRequest),
    DisconnectCis { connection_handle: u16 },
    DisconnectCisByRef { cis: CisRef },
    RemoveCig { cig_id: u8 },
    SetupIsoPath(IsoDataPathConfig),
    SetupIsoInputPathByRef { cis: CisRef },
    RemoveIsoPath {
        connection_handle: u16,
        directions: u8,
    },
    RemoveIsoPathByRef {
        cis: CisRef,
        directions: u8,
    },
    BindStreamRoute(StereoStreamRouteBinding),
    UnbindStreamRoute {
        session_id: SessionId,
        stream_id: StreamId,
    },
    BeginIsoDrain(IsoDrainRequest),
    ResumeIsoData(IsoDrainRequest),
    BeginGatt(GattRequest),
    CancelGatt {
        connection: ConnectionToken,
        request_id: GattRequestId,
    },
}


#[derive(Clone)]
pub struct HciControl {
    tx: Sender<HciCommand>,
    stop_tx: Sender<()>,
    iso_tx: Sender<IsoDataGroupRequest>,
}

impl HciControl {
    pub(crate) fn new(
        tx: Sender<HciCommand>,
        stop_tx: Sender<()>,
        iso_tx: Sender<IsoDataGroupRequest>,
    ) -> Self {
        Self { tx, stop_tx, iso_tx }
    }


    pub fn iso_tx(&self) -> HciIsoTx {
        HciIsoTx::new(self.iso_tx.clone())
    }

    pub fn logical_iso_tx(&self) -> HciLogicalIsoTx {
        HciLogicalIsoTx::new(self.iso_tx.clone())
    }

    pub fn try_begin_operational_attach(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BeginOperationalAttach)
            .map_err(map_send_error)
    }

    pub fn try_start_passive_extended_scan(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::StartPassiveExtendedScan)
            .map_err(map_send_error)
    }

    pub fn try_stop_passive_extended_scan(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::StopPassiveExtendedScan)
            .map_err(map_send_error)
    }

    pub fn try_read_controller_identity(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ReadControllerIdentity)
            .map_err(map_send_error)
    }

    pub fn try_read_acl_rssi_probe(
        &self,
        connection: ConnectionToken,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ReadAclRssiProbe { connection })
            .map_err(map_send_error)
    }

    pub fn try_read_acl_ll_features_probe(
        &self,
        connection: ConnectionToken,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ReadAclLlFeaturesProbe { connection })
            .map_err(map_send_error)
    }

    pub fn try_configure_controller_privacy(
        &self,
        config: ResolvingListConfig,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ConfigureControllerPrivacy(config))
            .map_err(map_send_error)
    }

    pub fn try_clear_controller_privacy(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ClearControllerPrivacy)
            .map_err(map_send_error)
    }

    pub fn try_begin_fal_connection(
        &self,
        request: FalConnectRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BeginFalConnection(request))
            .map_err(map_send_error)
    }

    pub fn try_cancel_fal_connection(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::CancelFalConnection)
            .map_err(map_send_error)
    }

    pub fn try_disconnect(
        &self,
        connection: ConnectionToken,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::Disconnect { connection })
            .map_err(map_send_error)
    }

    pub fn try_begin_stored_encryption(
        &self,
        request: StoredEncryptionRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BeginStoredEncryption(request))
            .map_err(map_send_error)
    }

    pub fn try_ensure_cis_host_feature(
        &self,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::EnsureCisHostFeature)
            .map_err(map_send_error)
    }

    pub fn try_read_acl_tx_audit(
        &self,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ReadAclTxAudit)
            .map_err(map_send_error)
    }

    pub fn try_read_acl_runtime_snapshot(
        &self,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ReadAclRuntimeSnapshot)
            .map_err(map_send_error)
    }

    pub fn try_read_cis_runtime_snapshot(
        &self,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ReadCisRuntimeSnapshot)
            .map_err(map_send_error)
    }

    pub fn try_read_iso_tx_audit(
        &self,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ReadIsoTxAudit)
            .map_err(map_send_error)
    }

    pub fn try_configure_cig(
        &self,
        config: CigConfig,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ConfigureCig(config))
            .map_err(map_send_error)
    }

    pub fn try_create_cis(
        &self,
        request: CisCreateRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::CreateCis(request))
            .map_err(map_send_error)
    }

    pub fn try_disconnect_cis(
        &self,
        connection_handle: u16,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::DisconnectCis {
                connection_handle,
            })
            .map_err(map_send_error)
    }

    pub fn try_disconnect_cis_by_ref(
        &self,
        cis: CisRef,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::DisconnectCisByRef { cis })
            .map_err(map_send_error)
    }

    pub fn try_remove_cig(&self, cig_id: u8) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::RemoveCig { cig_id })
            .map_err(map_send_error)
    }

    pub fn try_setup_iso_path(
        &self,
        config: IsoDataPathConfig,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::SetupIsoPath(config))
            .map_err(map_send_error)
    }

    pub fn try_setup_iso_input_path_by_ref(
        &self,
        cis: CisRef,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::SetupIsoInputPathByRef { cis })
            .map_err(map_send_error)
    }

    pub fn try_remove_iso_path(
        &self,
        connection_handle: u16,
        directions: u8,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::RemoveIsoPath {
                connection_handle,
                directions,
            })
            .map_err(map_send_error)
    }

    pub fn try_remove_iso_path_by_ref(
        &self,
        cis: CisRef,
        directions: u8,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::RemoveIsoPathByRef {
                cis,
                directions,
            })
            .map_err(map_send_error)
    }

    pub fn try_bind_stream_route(
        &self,
        binding: StereoStreamRouteBinding,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BindStreamRoute(binding))
            .map_err(map_send_error)
    }

    pub fn try_unbind_stream_route(
        &self,
        session_id: SessionId,
        stream_id: StreamId,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::UnbindStreamRoute {
                session_id,
                stream_id,
            })
            .map_err(map_send_error)
    }

    pub fn try_begin_iso_drain(
        &self,
        request: IsoDrainRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BeginIsoDrain(request))
            .map_err(map_send_error)
    }

    pub fn try_resume_iso_data(
        &self,
        request: IsoDrainRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ResumeIsoData(request))
            .map_err(map_send_error)
    }

    pub fn try_begin_gatt(
        &self,
        request: GattRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BeginGatt(request))
            .map_err(map_send_error)
    }

    pub fn try_cancel_gatt(
        &self,
        connection: ConnectionToken,
        request_id: GattRequestId,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::CancelGatt {
                connection,
                request_id,
            })
            .map_err(map_send_error)
    }

    /// Urgent lifecycle stop has a dedicated one-slot lane. A full stop lane
    /// means a stop is already pending and is therefore treated as success.
    pub fn request_stop(&self) -> Result<(), SendError> {
        match self.stop_tx.try_send(()) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err(SendError::Disconnected),
        }
    }
}


fn map_send_error<T>(error: TrySendError<T>) -> SendError {
    match error {
        TrySendError::Full(_) => SendError::Full,
        TrySendError::Disconnected(_) => SendError::Disconnected,
    }
}
