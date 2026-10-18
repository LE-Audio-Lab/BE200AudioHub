use crossbeam_channel::{
    bounded, Receiver, Sender, TryRecvError, TrySendError,
};

use crate::{
    architecture::types::{SessionId, StreamId},
    runtime::mailbox::{RecvError, SendError},
    transport::{
        packet::ControllerPacket,
        sink::{ControllerPacketSink, PacketSinkError},
    },
};

use super::{
    connection::{
        ConnectionToken, DisconnectState, FalConnectRequest,
        FalConnectionState, LeConnectionSetSnapshot,
    },
    engine::{
        ControllerCapabilities, ControllerIdentity,
        ControllerIdentityFailure, OperationalAttachState, PassiveScanState,
    },
    gatt::{GattReport, GattRequest, GattRequestId},
    iso::{
        CigConfig, CigConfigureState, CisCreateRequest, CisCreateState,
        CisRef, SemanticCisCreateState,
    },
    iso_credit::IsoCreditSnapshot,
    iso_host_feature::CisHostFeatureState,
    iso_lifecycle::{
        CigRemoveState, CisDisconnectState,
        SemanticCisDisconnectState,
    },
    iso_path::{
        IsoDataPathConfig, IsoPathOperationState,
        SemanticIsoPathState,
    },
    iso_tx::{IsoDrainRequest, IsoDrainState, IsoSduGroupRequest},
    privacy::{ControllerPrivacyState, ResolvingListConfig},
    scan::ScanSummary,
    security::{StoredEncryptionRequest, StoredEncryptionState},
    stream_route::{
        StereoStreamRouteBinding, StreamRouteState,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HciCommand {
    BeginOperationalAttach,
    StartPassiveExtendedScan,
    StopPassiveExtendedScan,
    ReadControllerIdentity,
    ConfigureControllerPrivacy(ResolvingListConfig),
    ClearControllerPrivacy,
    BeginFalConnection(FalConnectRequest),
    CancelFalConnection,
    Disconnect { connection: ConnectionToken },
    BeginStoredEncryption(StoredEncryptionRequest),
    EnsureCisHostFeature,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HciReport {
    AttachState(OperationalAttachState),
    CapabilitiesReady(ControllerCapabilities),
    ControllerIdentity(
        Result<ControllerIdentity, ControllerIdentityFailure>,
    ),
    PassiveScanState(PassiveScanState),
    ScanComplete(ScanSummary),
    ControllerPrivacyState(ControllerPrivacyState),
    FalConnectionState(FalConnectionState),
    DisconnectState(DisconnectState),
    StoredEncryptionState(StoredEncryptionState),
    CisHostFeatureState(CisHostFeatureState),
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

#[derive(Clone)]
pub struct HciControl {
    tx: Sender<HciCommand>,
    stop_tx: Sender<()>,
    iso_tx: Sender<IsoSduGroupRequest>,
}

#[derive(Clone)]
pub struct HciIsoTx {
    tx: Sender<IsoSduGroupRequest>,
}

#[derive(Clone)]
pub struct HciIngress {
    tx: Sender<ControllerPacket>,
}

pub struct HciReportInbox {
    rx: Receiver<HciReport>,
}

pub(crate) struct HciWorkerPorts {
    pub control_rx: Receiver<HciCommand>,
    pub stop_rx: Receiver<()>,
    pub controller_rx: Receiver<ControllerPacket>,
    pub iso_rx: Receiver<IsoSduGroupRequest>,
    pub report_tx: Sender<HciReport>,
}

const DEFAULT_ISO_DATA_CAPACITY: usize = 4;

pub(crate) fn hci_mailbox(
    controller_capacity: usize,
    control_capacity: usize,
    report_capacity: usize,
) -> (
    HciControl,
    HciIngress,
    HciReportInbox,
    HciWorkerPorts,
) {
    hci_mailbox_with_iso(
        controller_capacity,
        control_capacity,
        report_capacity,
        DEFAULT_ISO_DATA_CAPACITY,
    )
}

pub(crate) fn hci_mailbox_with_iso(
    controller_capacity: usize,
    control_capacity: usize,
    report_capacity: usize,
    iso_capacity: usize,
) -> (
    HciControl,
    HciIngress,
    HciReportInbox,
    HciWorkerPorts,
) {
    assert!(controller_capacity > 0);
    assert!(control_capacity > 0);
    assert!(report_capacity > 0);
    assert!(iso_capacity > 0);

    let (control_tx, control_rx) = bounded(control_capacity);
    let (stop_tx, stop_rx) = bounded(1);
    let (controller_tx, controller_rx) = bounded(controller_capacity);
    let (iso_tx, iso_rx) = bounded(iso_capacity);
    let (report_tx, report_rx) = bounded(report_capacity);

    (
        HciControl {
            tx: control_tx,
            stop_tx,
            iso_tx,
        },
        HciIngress { tx: controller_tx },
        HciReportInbox { rx: report_rx },
        HciWorkerPorts {
            control_rx,
            stop_rx,
            controller_rx,
            iso_rx,
            report_tx,
        },
    )
}

impl HciControl {
    pub fn iso_tx(&self) -> HciIsoTx {
        HciIsoTx {
            tx: self.iso_tx.clone(),
        }
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

impl HciIsoTx {
    pub fn try_submit_group(
        &self,
        request: IsoSduGroupRequest,
    ) -> Result<(), SendError> {
        self.tx.try_send(request).map_err(map_send_error)
    }
}

impl ControllerPacketSink for HciIngress {
    fn try_send_controller(
        &self,
        packet: ControllerPacket,
    ) -> Result<(), PacketSinkError> {
        self.tx.try_send(packet).map_err(|error| match error {
            TrySendError::Full(_) => PacketSinkError::Full,
            TrySendError::Disconnected(_) => PacketSinkError::Disconnected,
        })
    }
}

impl HciReportInbox {
    pub fn try_recv(&self) -> Result<HciReport, RecvError> {
        self.rx.try_recv().map_err(|error| match error {
            TryRecvError::Empty => RecvError::Empty,
            TryRecvError::Disconnected => RecvError::Disconnected,
        })
    }
}

fn map_send_error<T>(error: TrySendError<T>) -> SendError {
    match error {
        TrySendError::Full(_) => SendError::Full,
        TrySendError::Disconnected(_) => SendError::Disconnected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_lane_remains_available_when_controller_lane_is_full() {
        let (control, ingress, _reports, _ports) = hci_mailbox(1, 1, 1);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![1]))
            .unwrap();
        assert_eq!(
            ingress.try_send_controller(ControllerPacket::Event(vec![2])),
            Err(PacketSinkError::Full)
        );

        assert_eq!(control.request_stop(), Ok(()));
        assert_eq!(control.request_stop(), Ok(()));
    }

    #[test]
    fn controller_identity_probe_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        control.try_read_controller_identity().unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::ReadControllerIdentity
        );
    }

    #[test]
    fn privacy_clear_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        control.try_clear_controller_privacy().unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::ClearControllerPrivacy
        );
    }

    fn cig_config() -> CigConfig {
        let cis = |cis_id| super::super::iso::CisConfig {
            cis_id,
            max_sdu_c_to_p: 100,
            max_sdu_p_to_c: 0,
            phy_c_to_p: 2,
            phy_p_to_c: 1,
            rtn_c_to_p: 13,
            rtn_p_to_c: 0,
        };
        CigConfig {
            cig_id: 1,
            sdu_interval_c_to_p: 10_000,
            sdu_interval_p_to_c: 10_000,
            worst_case_sca: 0,
            packing: 0,
            framing: 0,
            max_transport_latency_c_to_p: 95,
            max_transport_latency_p_to_c: 95,
            cis: vec![cis(1), cis(2)],
        }
    }

    #[test]
    fn cis_host_feature_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 1, 1);
        control.try_ensure_cis_host_feature().unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::EnsureCisHostFeature
        );
    }

    #[test]
    fn cig_configuration_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 1, 1);

        let config = cig_config();
        control.try_configure_cig(config.clone()).unwrap();
        assert_eq!(
            control.try_configure_cig(config.clone()),
            Err(SendError::Full)
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::ConfigureCig(config)
        );
    }

    #[test]
    fn semantic_cis_disconnect_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 2, 1, 1);
        let cis = CisRef {
            cig_id: 1,
            cis_id: 2,
        };

        control.try_disconnect_cis_by_ref(cis).unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::DisconnectCisByRef { cis }
        );
    }

    #[test]
    fn cis_lifecycle_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 3, 1);

        let request = CisCreateRequest {
            cig_id: 1,
            bindings: vec![super::super::iso::CisAclBinding {
                cis_id: 1,
                acl: ConnectionToken {
                    connection_handle: 0x0800,
                    generation: 7,
                },
            }],
        };
        control.try_create_cis(request.clone()).unwrap();
        control.try_disconnect_cis(0x0900).unwrap();
        control.try_remove_cig(1).unwrap();

        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::CreateCis(request)
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::DisconnectCis {
                connection_handle: 0x0900,
            }
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::RemoveCig { cig_id: 1 }
        );
    }

    #[test]
    fn semantic_iso_paths_use_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 3, 1, 1);
        let cis = CisRef {
            cig_id: 1,
            cis_id: 2,
        };

        control
            .try_setup_iso_input_path_by_ref(cis)
            .unwrap();
        control
            .try_remove_iso_path_by_ref(cis, 0x01)
            .unwrap();

        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::SetupIsoInputPathByRef { cis }
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::RemoveIsoPathByRef {
                cis,
                directions: 0x01,
            }
        );
    }

    #[test]
    fn iso_paths_use_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        let config = IsoDataPathConfig::hci_input(0x0900);
        control.try_setup_iso_path(config.clone()).unwrap();
        control.try_remove_iso_path(0x0900, 0x01).unwrap();

        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::SetupIsoPath(config)
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::RemoveIsoPath {
                connection_handle: 0x0900,
                directions: 0x01,
            }
        );
    }

    #[test]
    fn logical_stream_routes_use_reliable_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 2, 1, 1);
        let binding = StereoStreamRouteBinding {
            session_id: SessionId(7),
            stream_id: StreamId(11),
            left: CisRef {
                cig_id: 1,
                cis_id: 1,
            },
            right: CisRef {
                cig_id: 1,
                cis_id: 2,
            },
        };

        control.try_bind_stream_route(binding).unwrap();
        control
            .try_unbind_stream_route(
                binding.session_id,
                binding.stream_id,
            )
            .unwrap();

        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::BindStreamRoute(binding)
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::UnbindStreamRoute {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
            }
        );
    }

    #[test]
    fn iso_drain_gate_uses_reliable_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);
        let request = IsoDrainRequest {
            connection_handles: vec![0x0900, 0x0901],
        };

        control.try_begin_iso_drain(request.clone()).unwrap();
        control.try_resume_iso_data(request.clone()).unwrap();

        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::BeginIsoDrain(request.clone())
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::ResumeIsoData(request)
        );
    }

    #[test]
    fn gatt_requests_use_bounded_control_lane() {
        use super::super::gatt::{
            GattOperation, GattRequest,
        };

        let (control, _, _, ports) = hci_mailbox(1, 2, 1);
        let connection = ConnectionToken {
            connection_handle: 0x0800,
            generation: 7,
        };

        control
            .try_begin_gatt(GattRequest {
                request_id: 41,
                connection,
                operation: GattOperation::DiscoverServices,
            })
            .unwrap();
        control.try_cancel_gatt(connection, 41).unwrap();

        assert!(matches!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::BeginGatt(GattRequest {
                request_id: 41,
                ..
            })
        ));
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::CancelGatt {
                connection,
                request_id: 41,
            }
        );
    }

    #[test]
    fn stored_encryption_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        let request = StoredEncryptionRequest {
            connection: ConnectionToken {
                connection_handle: 0x0800,
                generation: 7,
            },
            key: super::super::security::StoredLeKeyMaterial::
                secure_connections([0x55; 16]),
        };
        control
            .try_begin_stored_encryption(request)
            .unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::BeginStoredEncryption(request)
        );
    }

    #[test]
    fn disconnect_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        let connection = ConnectionToken {
            connection_handle: 0x0800,
            generation: 7,
        };
        control.try_disconnect(connection).unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::Disconnect { connection }
        );
    }

    #[test]
    fn fal_cancel_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        control.try_cancel_fal_connection().unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::CancelFalConnection
        );
    }

    #[test]
    fn iso_data_lane_is_bounded_and_independent_from_control() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 1, 1, 1);
        let iso = control.iso_tx();
        let group = IsoSduGroupRequest {
            sdus: vec![super::super::iso_tx::IsoSduRequest {
                connection_handle: 0x0900,
                payload: vec![1, 2, 3],
            }],
        };

        iso.try_submit_group(group.clone()).unwrap();
        assert_eq!(
            iso.try_submit_group(group.clone()),
            Err(SendError::Full)
        );

        control.try_begin_operational_attach().unwrap();
        assert_eq!(
            ports.iso_rx.try_recv().unwrap(),
            group
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::BeginOperationalAttach
        );
    }

    #[test]
    fn control_and_controller_backpressure_are_independent() {
        let (control, ingress, _reports, _ports) = hci_mailbox(1, 1, 1);

        control.try_begin_operational_attach().unwrap();
        assert_eq!(
            control.try_begin_operational_attach(),
            Err(SendError::Full)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![1]))
            .unwrap();
        assert_eq!(
            ingress.try_send_controller(ControllerPacket::Event(vec![2])),
            Err(PacketSinkError::Full)
        );
    }
}
