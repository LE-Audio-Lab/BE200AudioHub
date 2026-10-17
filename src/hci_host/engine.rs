use std::collections::{BTreeMap, VecDeque};

use bumble_hci::{
    codes::{
        HCI_DISCONNECT_COMMAND,
        HCI_READ_BD_ADDR_COMMAND,
        HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        HCI_LE_CREATE_CONNECTION_CANCEL_COMMAND,
        HCI_LE_ENABLE_ENCRYPTION_COMMAND,
        HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
    },
    Command,
};
use bumble_host::{
    CisControlEvent, Device, DeviceConnectionTransport, DeviceEvent,
    HostTransport, IsoControlEvent,
};

use crate::transport::{
    packet::ControllerPacket,
    worker::TransportTx,
};

use super::{
    acl_credit::{AclCreditAudit, AclCreditFault},
    connection::{
        ConnectionEvent, ConnectionToken, DisconnectAction, DisconnectConfigError,
        DisconnectEvent, DisconnectState, DisconnectTransaction,
        FalConnectAction, FalConnectConfigError, FalConnectRequest,
        FalConnectTransaction, FalConnectionState,
        LeConnectionSetSnapshot, LeConnectionSnapshot,
    },
    gatt::{
        GattAction, GattFailure, GattFailureKind, GattReport, GattRequest,
        GattRequestId, GattResult, GattTransaction, GattValueEvent,
        ATT_DEFAULT_MTU,
    },
    iso::{
        CigConfig, CigConfigError, CigConfigureAction, CigConfigureState,
        CigConfigureTransaction, CisCreateAction, CisCreateConfigError,
        CisCreateRequest, CisCreateState, CisCreateTransaction,
    },
    iso_lifecycle::{
        CigRemoveAction, CigRemoveState, CigRemoveTransaction,
        CisDisconnectAction, CisDisconnectEvent, CisDisconnectState,
        CisDisconnectTransaction,
    },
    iso_path::{
        IsoDataPathConfig, IsoPathConfigError, IsoPathOperationState,
        IsoPathRemoveAction, IsoPathRemoveTransaction, IsoPathSetupAction,
        IsoPathSetupTransaction,
    },
    link::{HciObservation, LinkFault, QueueHostTransport},
    privacy::{
        ControllerPrivacyState, PrivacyAction, PrivacyClearAction,
        PrivacyClearTransaction, PrivacyConfigError, PrivacyTransaction,
        ResolvingListConfig,
    },
    scan::{ScanAccumulator, ScanSummary},
    security::{
        EncryptionEvent, StoredEncryptionAction,
        StoredEncryptionConfigError, StoredEncryptionRequest,
        StoredEncryptionState, StoredEncryptionTransaction,
    },
};

const DEFAULT_POLL_BUDGET: usize = 64;
const MAX_DEFERRED_DEVICE_EVENTS: usize = 64;
const MAX_GATT_REPORTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineFault {
    Link(LinkFault),
    PollBudgetExceeded,
    InvalidState,
    PrivacyConfig(PrivacyConfigError),
    FalConnectConfig(FalConnectConfigError),
    DisconnectConfig(DisconnectConfigError),
    StoredEncryptionConfig(StoredEncryptionConfigError),
    CigConfig(CigConfigError),
    CisCreateConfig(CisCreateConfigError),
    IsoPathConfig(IsoPathConfigError),
    IsoPathSendRejected { connection_handle: u16 },
    UnexpectedIsoControlEvent,
    IsoControlJournalMismatch { connection_handle: u16 },
    IsoPathProjectionMismatch {
        connection_handle: u16,
        direction: u8,
    },
    StaleConnection {
        connection_handle: u16,
        requested_generation: u64,
        current_generation: Option<u64>,
    },
    AclCredit(AclCreditFault),
    GattSendRejected { connection_handle: u16 },
    GattReportBacklogFull,
    DeviceEventBacklogFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationalAttachState {
    Idle,
    Discovering,
    Ready,
    Failed(OperationalAttachFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationalAttachFailure {
    SupportedCommandsStatus(u8),
    HostInitializationFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveScanState {
    Idle,
    Configuring,
    Enabling,
    Scanning,
    Disabling,
    Failed {
        opcode: u16,
        status: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketPoolSnapshot {
    pub data_packet_length: u16,
    pub total_num_data_packets: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerCapabilities {
    pub supported_commands: [u8; 64],
    pub hci_version: Option<u8>,
    pub hci_subversion: Option<u16>,
    pub manufacturer: Option<u16>,
    pub local_le_features: Vec<u8>,
    pub local_le_features_max_page: Option<u8>,
    pub le_acl_pool: Option<PacketPoolSnapshot>,
    pub iso_pool: Option<PacketPoolSnapshot>,
    pub number_of_supported_advertising_sets: u8,
    pub maximum_advertising_data_length: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerIdentity {
    pub public_address_le: [u8; 6],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerIdentityFailure {
    Status(u8),
    MalformedResponse,
}

pub struct HciEngine {
    device: Device,
    link: QueueHostTransport,
    attach_state: OperationalAttachState,
    scan_state: PassiveScanState,
    scan_pending_opcode: Option<u16>,
    scan_accumulator: ScanAccumulator,
    completed_scan: Option<ScanSummary>,
    controller_identity_pending: bool,
    controller_identity_result:
        Option<Result<ControllerIdentity, ControllerIdentityFailure>>,
    privacy_state: ControllerPrivacyState,
    privacy_transaction: Option<PrivacyTransaction>,
    privacy_clear_transaction: Option<PrivacyClearTransaction>,
    fal_connection_state: FalConnectionState,
    fal_connection_transaction: Option<FalConnectTransaction>,
    disconnect_state: DisconnectState,
    disconnect_transaction: Option<DisconnectTransaction>,
    stored_encryption_state: StoredEncryptionState,
    stored_encryption_transaction: Option<StoredEncryptionTransaction>,
    connection_generations: BTreeMap<u16, u64>,
    next_connection_generation: u64,
    acl_credit_audit: AclCreditAudit,
    gatt_transactions: BTreeMap<u16, GattTransaction>,
    gatt_mtus: BTreeMap<u16, u16>,
    gatt_poisoned: BTreeMap<u16, GattRequestId>,
    gatt_reports: VecDeque<GattReport>,
    cig_configure_state: CigConfigureState,
    cig_configure_transaction: Option<CigConfigureTransaction>,
    cig_remove_state: CigRemoveState,
    cig_remove_transaction: Option<CigRemoveTransaction>,
    cis_create_state: CisCreateState,
    cis_create_transaction: Option<CisCreateTransaction>,
    cis_disconnect_state: CisDisconnectState,
    cis_disconnect_transaction: Option<CisDisconnectTransaction>,
    iso_path_state: IsoPathOperationState,
    iso_path_setup_transaction: Option<IsoPathSetupTransaction>,
    iso_path_remove_transaction: Option<IsoPathRemoveTransaction>,
    deferred_device_events: VecDeque<DeviceEvent>,
}

impl HciEngine {
    pub fn new(tx: TransportTx) -> Self {
        Self {
            device: Device::new(0),
            link: QueueHostTransport::new(tx),
            attach_state: OperationalAttachState::Idle,
            scan_state: PassiveScanState::Idle,
            scan_pending_opcode: None,
            scan_accumulator: ScanAccumulator::default(),
            completed_scan: None,
            controller_identity_pending: false,
            controller_identity_result: None,
            privacy_state: ControllerPrivacyState::Idle,
            privacy_transaction: None,
            privacy_clear_transaction: None,
            fal_connection_state: FalConnectionState::Idle,
            fal_connection_transaction: None,
            disconnect_state: DisconnectState::Idle,
            disconnect_transaction: None,
            stored_encryption_state: StoredEncryptionState::Idle,
            stored_encryption_transaction: None,
            connection_generations: BTreeMap::new(),
            next_connection_generation: 1,
            acl_credit_audit: AclCreditAudit::default(),
            gatt_transactions: BTreeMap::new(),
            gatt_mtus: BTreeMap::new(),
            gatt_poisoned: BTreeMap::new(),
            gatt_reports: VecDeque::new(),
            cig_configure_state: CigConfigureState::Idle,
            cig_configure_transaction: None,
            cig_remove_state: CigRemoveState::Idle,
            cig_remove_transaction: None,
            cis_create_state: CisCreateState::Idle,
            cis_create_transaction: None,
            cis_disconnect_state: CisDisconnectState::Idle,
            cis_disconnect_transaction: None,
            iso_path_state: IsoPathOperationState::Idle,
            iso_path_setup_transaction: None,
            iso_path_remove_transaction: None,
            deferred_device_events: VecDeque::new(),
        }
    }

    pub fn ingest(&mut self, packet: ControllerPacket) -> Result<(), EngineFault> {
        self.link.ingest(packet).map_err(EngineFault::Link)
    }

    pub fn attach_state(&self) -> OperationalAttachState {
        self.attach_state
    }

    pub fn passive_scan_state(&self) -> PassiveScanState {
        self.scan_state
    }

    pub fn take_completed_scan(&mut self) -> Option<ScanSummary> {
        self.completed_scan.take()
    }

    pub fn begin_controller_identity_probe(
        &mut self,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.controller_identity_pending
        {
            return Err(EngineFault::InvalidState);
        }

        self.controller_identity_result = None;
        self.link
            .suppress_next_command_complete(HCI_READ_BD_ADDR_COMMAND);
        self.link.handle_command(0, Command::ReadBdAddr);
        self.check_link_fault()?;
        self.controller_identity_pending = true;
        Ok(())
    }

    pub fn take_controller_identity_result(
        &mut self,
    ) -> Option<Result<ControllerIdentity, ControllerIdentityFailure>> {
        self.controller_identity_result.take()
    }

    pub fn controller_privacy_state(&self) -> &ControllerPrivacyState {
        &self.privacy_state
    }

    pub fn fal_connection_state(&self) -> &FalConnectionState {
        &self.fal_connection_state
    }

    pub fn connection_set_snapshot(&self) -> LeConnectionSetSnapshot {
        LeConnectionSetSnapshot {
            connections: self
                .device
                .le_connections()
                .map(|info| {
                    let generation = *self
                        .connection_generations
                        .get(&info.connection_handle)
                        .expect("live LE connection missing generation");
                    snapshot_le_connection(info, generation)
                })
                .collect(),
        }
    }

    pub fn disconnect_state(&self) -> &DisconnectState {
        &self.disconnect_state
    }

    pub fn stored_encryption_state(&self) -> &StoredEncryptionState {
        &self.stored_encryption_state
    }

    pub fn begin_gatt_request(
        &mut self,
        request: GattRequest,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready {
            return Err(EngineFault::InvalidState);
        }

        self.validate_connection_token(request.connection)?;
        let handle = request.connection.connection_handle;
        let request_id = request.request_id;
        let connection = request.connection;

        if self
            .gatt_transactions
            .values()
            .any(|transaction| transaction.request_id() == request_id)
        {
            return self.push_gatt_report(GattReport::Failed(GattFailure {
                request_id,
                connection,
                kind: GattFailureKind::DuplicateRequestId,
            }));
        }

        if let Some(previous_request_id) =
            self.gatt_poisoned.get(&handle).copied()
        {
            return self.push_gatt_report(GattReport::Failed(GattFailure {
                request_id,
                connection,
                kind: GattFailureKind::BearerPoisoned {
                    previous_request_id,
                },
            }));
        }

        if self.gatt_transactions.contains_key(&handle) {
            return self.push_gatt_report(GattReport::Failed(GattFailure {
                request_id,
                connection,
                kind: GattFailureKind::Busy,
            }));
        }

        if !self.device.is_encrypted_on_handle(handle) {
            return self.push_gatt_report(GattReport::Failed(GattFailure {
                request_id,
                connection,
                kind: GattFailureKind::NotEncrypted,
            }));
        }

        let mtu = self
            .gatt_mtus
            .get(&handle)
            .copied()
            .unwrap_or(ATT_DEFAULT_MTU);
        let (transaction, first_pdu) =
            match GattTransaction::begin(request, mtu) {
                Ok(value) => value,
                Err(failure) => {
                    return self.push_gatt_report(
                        GattReport::Failed(failure),
                    )
                }
            };

        self.gatt_transactions.insert(handle, transaction);
        self.handle_gatt_action(handle, GattAction::Send(first_pdu))
    }

    pub fn cancel_gatt_request(
        &mut self,
        connection: ConnectionToken,
        request_id: GattRequestId,
    ) -> Result<(), EngineFault> {
        self.validate_connection_token(connection)?;
        let handle = connection.connection_handle;

        let action = {
            let transaction = self
                .gatt_transactions
                .get_mut(&handle)
                .ok_or(EngineFault::InvalidState)?;
            if transaction.request_id() != request_id
                || transaction.connection() != connection
            {
                return Err(EngineFault::InvalidState);
            }
            transaction.cancel()
        };

        self.gatt_poisoned.insert(handle, request_id);
        self.handle_gatt_action(handle, action)?;
        self.push_gatt_report(GattReport::BearerPoisoned {
            connection,
            request_id,
        })
    }

    pub fn take_gatt_reports(&mut self) -> Vec<GattReport> {
        self.gatt_reports.drain(..).collect()
    }

    pub fn gatt_mtu(&self, connection: ConnectionToken) -> Option<u16> {
        (self
            .connection_generations
            .get(&connection.connection_handle)
            .copied()
            == Some(connection.generation))
        .then(|| {
            self.gatt_mtus
                .get(&connection.connection_handle)
                .copied()
                .unwrap_or(ATT_DEFAULT_MTU)
        })
    }

    pub fn gatt_in_flight_count(&self) -> usize {
        self.gatt_transactions.len()
    }

    pub fn cig_configure_state(&self) -> &CigConfigureState {
        &self.cig_configure_state
    }

    pub fn begin_cig_configuration(
        &mut self,
        config: CigConfig,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.cig_configure_transaction.is_some()
        {
            return Err(EngineFault::InvalidState);
        }

        let (transaction, command) =
            CigConfigureTransaction::begin(config)
                .map_err(EngineFault::CigConfig)?;

        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.cig_configure_transaction = Some(transaction);
        self.cig_configure_state = CigConfigureState::Configuring;
        Ok(())
    }

    pub fn cig_remove_state(&self) -> &CigRemoveState {
        &self.cig_remove_state
    }

    pub fn begin_cig_removal(
        &mut self,
        cig_id: u8,
    ) -> Result<(), EngineFault> {
        let cis_may_be_active = match &self.cis_create_state {
            CisCreateState::Ready { established, .. } => established.iter().any(
                |entry| {
                    self.device
                        .cis_link(entry.link.connection_handle)
                        .is_some()
                },
            ),
            CisCreateState::Failed(failure) => failure.established.iter().any(
                |entry| {
                    self.device
                        .cis_link(entry.link.connection_handle)
                        .is_some()
                },
            ),
            _ => false,
        };

        if self.attach_state != OperationalAttachState::Ready
            || self.cig_configure_transaction.is_some()
            || self.cig_remove_transaction.is_some()
            || self.cis_create_transaction.is_some()
            || self.cis_disconnect_transaction.is_some()
            || cis_may_be_active
        {
            return Err(EngineFault::InvalidState);
        }

        let (transaction, command) = CigRemoveTransaction::begin(cig_id);
        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.cig_remove_transaction = Some(transaction);
        self.cig_remove_state = CigRemoveState::Removing { cig_id };
        Ok(())
    }

    pub fn cis_disconnect_state(&self) -> &CisDisconnectState {
        &self.cis_disconnect_state
    }

    pub fn begin_cis_disconnect(
        &mut self,
        connection_handle: u16,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.disconnect_transaction.is_some()
            || self.cis_create_transaction.is_some()
            || self.cis_disconnect_transaction.is_some()
            || self.iso_path_setup_transaction.is_some()
            || self.iso_path_remove_transaction.is_some()
            || self.cig_remove_transaction.is_some()
            || !self.cis_state_contains_established_handle(connection_handle)
            || self.device.cis_link(connection_handle).is_none()
            || self.device.iso_data_path(connection_handle, 0).is_some()
            || self.device.iso_data_path(connection_handle, 1).is_some()
        {
            return Err(EngineFault::InvalidState);
        }

        let (transaction, command) =
            CisDisconnectTransaction::begin(connection_handle);
        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.cis_disconnect_transaction = Some(transaction);
        self.cis_disconnect_state =
            CisDisconnectState::Submitting { connection_handle };
        Ok(())
    }

    pub fn iso_path_state(&self) -> &IsoPathOperationState {
        &self.iso_path_state
    }

    fn cis_ready_for_iso_path(&self, connection_handle: u16) -> bool {
        let ready = matches!(
            &self.cis_create_state,
            CisCreateState::Ready { established, .. }
                if established.iter().any(|cis| {
                    cis.link.connection_handle == connection_handle
                })
        );
        ready && self.device.cis_link(connection_handle).is_some()
    }

    pub fn begin_iso_path_setup(
        &mut self,
        config: IsoDataPathConfig,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.iso_path_setup_transaction.is_some()
            || self.iso_path_remove_transaction.is_some()
            || !self.cis_ready_for_iso_path(config.connection_handle)
            || self
                .device
                .iso_data_path(
                    config.connection_handle,
                    config.direction as u8,
                )
                .is_some()
        {
            return Err(EngineFault::InvalidState);
        }

        let (transaction, _command) =
            IsoPathSetupTransaction::begin(config.clone())
                .map_err(EngineFault::IsoPathConfig)?;
        let parameters = config.to_bumble_parameters();
        if !self.device.setup_iso_data_path_with_parameters(
            &mut self.link,
            config.connection_handle,
            parameters,
        ) {
            return Err(EngineFault::IsoPathSendRejected {
                connection_handle: config.connection_handle,
            });
        }
        self.check_link_fault()?;

        self.iso_path_setup_transaction = Some(transaction);
        self.iso_path_state = IsoPathOperationState::SettingUp(config);
        Ok(())
    }

    pub fn begin_iso_path_remove(
        &mut self,
        connection_handle: u16,
        directions: u8,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.iso_path_setup_transaction.is_some()
            || self.iso_path_remove_transaction.is_some()
            || !self.cis_ready_for_iso_path(connection_handle)
        {
            return Err(EngineFault::InvalidState);
        }

        let (transaction, _command) =
            IsoPathRemoveTransaction::begin(connection_handle, directions)
                .map_err(EngineFault::IsoPathConfig)?;

        for direction in 0..=1 {
            if directions & (1 << direction) != 0
                && self
                    .device
                    .iso_data_path(connection_handle, direction)
                    .is_none()
            {
                return Err(EngineFault::InvalidState);
            }
        }

        if !self.device.remove_iso_data_path(
            &mut self.link,
            connection_handle,
            directions,
        ) {
            return Err(EngineFault::IsoPathSendRejected {
                connection_handle,
            });
        }
        self.check_link_fault()?;

        self.iso_path_remove_transaction = Some(transaction);
        self.iso_path_state = IsoPathOperationState::Removing {
            connection_handle,
            directions,
        };
        Ok(())
    }

    pub fn cis_create_state(&self) -> &CisCreateState {
        &self.cis_create_state
    }

    pub fn begin_cis_creation(
        &mut self,
        request: CisCreateRequest,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.cis_create_transaction.is_some()
        {
            return Err(EngineFault::InvalidState);
        }

        let allocation = match &self.cig_configure_state {
            CigConfigureState::Ready(allocation)
                if allocation.cig_id == request.cig_id =>
            {
                allocation.clone()
            }
            _ => return Err(EngineFault::InvalidState),
        };

        for binding in &request.bindings {
            self.validate_connection_token(binding.acl)?;
        }

        let (transaction, command) =
            CisCreateTransaction::begin(allocation, request.bindings)
                .map_err(EngineFault::CisCreateConfig)?;

        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.cis_create_transaction = Some(transaction);
        self.cis_create_state =
            CisCreateState::Submitting { cig_id: request.cig_id };
        Ok(())
    }

    pub fn begin_stored_encryption(
        &mut self,
        request: StoredEncryptionRequest,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.stored_encryption_transaction.is_some()
            || self.disconnect_transaction.is_some()
        {
            return Err(EngineFault::InvalidState);
        }

        self.validate_connection_token(request.connection)?;
        let connection = self
            .device
            .le_connection(request.connection.connection_handle)
            .expect("validated live connection missing from Bumble");

        if self
            .device
            .is_encrypted_on_handle(request.connection.connection_handle)
        {
            self.stored_encryption_state = StoredEncryptionState::Encrypted {
                connection: request.connection,
                encryption_key_size: connection.encryption_key_size,
            };
            return Ok(());
        }

        let (transaction, command) =
            StoredEncryptionTransaction::begin(request)
                .map_err(EngineFault::StoredEncryptionConfig)?;

        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.stored_encryption_transaction = Some(transaction);
        self.stored_encryption_state = StoredEncryptionState::Submitting;
        Ok(())
    }

    pub fn begin_disconnect(
        &mut self,
        connection: ConnectionToken,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.disconnect_transaction.is_some()
            || self.cis_disconnect_transaction.is_some()
        {
            return Err(EngineFault::InvalidState);
        }

        self.validate_connection_token(connection)?;

        let (transaction, command) =
            DisconnectTransaction::begin(connection)
                .map_err(EngineFault::DisconnectConfig)?;

        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.disconnect_transaction = Some(transaction);
        self.disconnect_state = DisconnectState::Submitting;
        Ok(())
    }

    fn validate_connection_token(
        &self,
        connection: ConnectionToken,
    ) -> Result<(), EngineFault> {
        let current_generation = self
            .connection_generations
            .get(&connection.connection_handle)
            .copied();

        if current_generation != Some(connection.generation)
            || !self
                .device
                .is_connected_on_handle(connection.connection_handle)
        {
            return Err(EngineFault::StaleConnection {
                connection_handle: connection.connection_handle,
                requested_generation: connection.generation,
                current_generation,
            });
        }

        Ok(())
    }

    pub fn cancel_fal_connection(&mut self) -> Result<(), EngineFault> {
        let action = {
            let transaction = self
                .fal_connection_transaction
                .as_mut()
                .ok_or(EngineFault::InvalidState)?;
            transaction.request_cancel()
        };

        if matches!(action, FalConnectAction::Ignored) {
            return Err(EngineFault::InvalidState);
        }

        self.handle_fal_connection_action(action)
    }

    pub fn begin_fal_connection(
        &mut self,
        request: FalConnectRequest,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.scan_state != PassiveScanState::Idle
            || self.privacy_transaction.is_some()
            || self.privacy_clear_transaction.is_some()
            || self.fal_connection_transaction.is_some()
            || self.device.is_scanning()
            || self.device.is_le_connecting()
        {
            return Err(EngineFault::InvalidState);
        }

        let (transaction, command) =
            FalConnectTransaction::begin(request)
                .map_err(EngineFault::FalConnectConfig)?;

        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.fal_connection_transaction = Some(transaction);
        self.fal_connection_state = FalConnectionState::Preparing;
        Ok(())
    }

    pub fn begin_controller_privacy_clear(
        &mut self,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.scan_state != PassiveScanState::Idle
            || self.privacy_transaction.is_some()
            || self.privacy_clear_transaction.is_some()
            || self.fal_connection_transaction.is_some()
            || self.device.is_scanning()
            || self.device.is_le_connecting()
        {
            return Err(EngineFault::InvalidState);
        }

        let (transaction, command) = PrivacyClearTransaction::begin();
        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.privacy_clear_transaction = Some(transaction);
        self.privacy_state = ControllerPrivacyState::Clearing;
        Ok(())
    }

    pub fn begin_controller_privacy(
        &mut self,
        config: ResolvingListConfig,
    ) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.scan_state != PassiveScanState::Idle
            || self.privacy_transaction.is_some()
            || self.fal_connection_transaction.is_some()
            || self.device.is_scanning()
            || self.device.is_le_connecting()
        {
            return Err(EngineFault::InvalidState);
        }

        let (transaction, command) =
            PrivacyTransaction::begin(config).map_err(EngineFault::PrivacyConfig)?;

        self.link.handle_command(0, command);
        self.check_link_fault()?;

        self.privacy_transaction = Some(transaction);
        self.privacy_state = ControllerPrivacyState::Configuring;
        Ok(())
    }

    pub fn begin_passive_extended_scan(&mut self) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.scan_state != PassiveScanState::Idle
            || self.privacy_transaction.is_some()
            || self.fal_connection_transaction.is_some()
        {
            return Err(EngineFault::InvalidState);
        }

        // Discard any stale advertisements left by an earlier probe before
        // beginning a new bounded scan session.
        let _ = self.device.take_advertisements();
        self.scan_accumulator.reset();
        self.completed_scan = None;

        let command = Command::LeSetExtendedScanParameters {
            own_address_type: 0,
            scanning_filter_policy: 0,
            scanning_phys: 0x01,
            scan_types: vec![0],
            scan_intervals: vec![0x0060],
            scan_windows: vec![0x0030],
        };
        self.scan_pending_opcode = Some(command.op_code());
        self.link.handle_command(0, command);
        self.check_link_fault()?;
        self.scan_state = PassiveScanState::Configuring;
        Ok(())
    }

    pub fn stop_passive_extended_scan(&mut self) -> Result<(), EngineFault> {
        if self.scan_state == PassiveScanState::Idle {
            return Ok(());
        }
        if self.scan_state != PassiveScanState::Scanning {
            return Err(EngineFault::InvalidState);
        }

        let command = Command::LeSetExtendedScanEnable {
            enable: 0,
            filter_duplicates: 0,
            duration: 0,
            period: 0,
        };
        self.scan_pending_opcode = Some(command.op_code());
        self.link.handle_command(0, command);
        self.check_link_fault()?;
        self.scan_state = PassiveScanState::Disabling;
        Ok(())
    }

    pub fn capabilities(&self) -> Option<ControllerCapabilities> {
        if self.attach_state != OperationalAttachState::Ready {
            return None;
        }

        let supported_commands = *self.device.local_supported_commands()?;
        let version = self.device.local_version();

        let map_pool = |pool: bumble_host::ControllerBufferInfo| {
            PacketPoolSnapshot {
                data_packet_length: pool.data_packet_length,
                total_num_data_packets: pool.total_num_data_packets,
            }
        };

        Some(ControllerCapabilities {
            supported_commands,
            hci_version: version.map(|version| version.hci_version),
            hci_subversion: version.map(|version| version.hci_subversion),
            manufacturer: version.map(|version| version.company_identifier),
            local_le_features: self
                .device
                .local_le_features()
                .unwrap_or(&[])
                .to_vec(),
            local_le_features_max_page: self.device.local_le_features_max_page(),
            le_acl_pool: self.device.le_acl_buffer().map(map_pool),
            iso_pool: self.device.iso_buffer().map(map_pool),
            number_of_supported_advertising_sets: self
                .device
                .number_of_supported_advertising_sets(),
            maximum_advertising_data_length: self
                .device
                .maximum_advertising_data_length(),
        })
    }

    /// Attach to an already-operational controller without issuing HCI Reset.
    ///
    /// Bumble Device already contains the asynchronous capability-discovery and
    /// host-initialization completion chain. Kicking only Supported Commands is
    /// enough for Device::poll() to schedule the remaining capability/event-mask/
    /// buffer-size commands as completions arrive.
    pub fn begin_operational_attach(&mut self) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Idle {
            return Ok(());
        }

        self.link
            .handle_command(0, Command::ReadLocalSupportedCommands);

        if let Some(fault) = self.link.take_fault() {
            return Err(EngineFault::Link(fault));
        }

        self.attach_state = OperationalAttachState::Discovering;
        Ok(())
    }

    /// Drive Bumble's transport-neutral Device until it reports no more
    /// controller events for this turn. The bound prevents a malformed or
    /// self-generating event stream from monopolizing HciContext forever.
    pub fn poll_until_quiescent(&mut self) -> Result<usize, EngineFault> {
        self.poll_with_budget(DEFAULT_POLL_BUDGET)
    }

    pub fn poll_with_budget(&mut self, budget: usize) -> Result<usize, EngineFault> {
        let mut turns = 0usize;

        while turns < budget {
            if let Some(fault) = self.link.take_fault() {
                return Err(EngineFault::Link(fault));
            }

            if !self.device.poll(&mut self.link) {
                self.check_link_fault()?;
                self.collect_scan_observations();
                self.process_observations()?;
                self.process_cis_control_events()?;
                self.process_iso_control_events()?;
                self.process_gatt_inbox()?;
                self.process_device_events()?;
                self.check_link_fault()?;
                self.refresh_attach_state()?;
                return Ok(turns);
            }

            turns += 1;
        }

        Err(EngineFault::PollBudgetExceeded)
    }

    fn check_link_fault(&mut self) -> Result<(), EngineFault> {
        if let Some(fault) = self.link.take_fault() {
            Err(EngineFault::Link(fault))
        } else {
            Ok(())
        }
    }

    fn collect_scan_observations(&mut self) {
        if matches!(
            self.scan_state,
            PassiveScanState::Idle | PassiveScanState::Failed { .. }
        ) {
            // Still drain Device's journal so stale pre-session reports cannot
            // leak into the next scan.
            let _ = self.device.take_advertisements();
            return;
        }

        for advertisement in self.device.take_advertisements() {
            self.scan_accumulator.push(advertisement);
        }
    }

    fn process_observations(&mut self) -> Result<(), EngineFault> {
        for observation in self.link.take_observations() {
            self.process_acl_credit_observation(&observation)?;
            self.process_controller_identity_observation(&observation);
            self.process_privacy_observation(&observation)?;
            self.process_fal_connection_observation(&observation)?;
            self.process_disconnect_observation(&observation)?;
            self.process_cis_disconnect_observation(&observation)?;
            self.process_stored_encryption_observation(&observation)?;
            self.process_cig_observation(&observation)?;
            self.process_scan_observation(&observation)?;
        }
        Ok(())
    }

    fn process_cis_control_events(&mut self) -> Result<(), EngineFault> {
        for event in self.device.take_cis_control_events() {
            let action = match event {
                CisControlEvent::CigConfigured { .. } => None,
                CisControlEvent::CommandStatus {
                    command_opcode,
                    status,
                } => self
                    .cis_create_transaction
                    .as_mut()
                    .map(|transaction| {
                        transaction
                            .on_command_status(command_opcode, status)
                    }),
                CisControlEvent::Established { status, link } => self
                    .cis_create_transaction
                    .as_mut()
                    .map(|transaction| {
                        transaction.on_established(status, link)
                    }),
            };

            let Some(action) = action else {
                continue;
            };
            self.handle_cis_create_action(action);
        }
        Ok(())
    }

    fn process_iso_control_events(&mut self) -> Result<(), EngineFault> {
        for event in self.device.take_iso_control_events() {
            match event {
                IsoControlEvent::DataPathSetup {
                    status,
                    connection_handle,
                    parameters,
                } => {
                    let Some(transaction) =
                        self.iso_path_setup_transaction.as_mut()
                    else {
                        return Err(EngineFault::UnexpectedIsoControlEvent);
                    };
                    let expected = transaction.config().clone();
                    if expected.connection_handle != connection_handle
                        || expected.to_bumble_parameters() != parameters
                    {
                        return Err(EngineFault::IsoControlJournalMismatch {
                            connection_handle,
                        });
                    }

                    let action = transaction.on_command_complete(
                        bumble_hci::codes::HCI_LE_SETUP_ISO_DATA_PATH_COMMAND,
                        status,
                        &connection_handle.to_le_bytes(),
                    );
                    match action {
                        IsoPathSetupAction::Complete {
                            connection_handle,
                            direction,
                        } => {
                            if self
                                .device
                                .iso_data_path(
                                    connection_handle,
                                    direction as u8,
                                )
                                != Some(&parameters)
                            {
                                return Err(
                                    EngineFault::IsoPathProjectionMismatch {
                                        connection_handle,
                                        direction: direction as u8,
                                    },
                                );
                            }
                            self.iso_path_setup_transaction = None;
                            self.iso_path_state =
                                IsoPathOperationState::SetupComplete {
                                    connection_handle,
                                    direction,
                                };
                        }
                        IsoPathSetupAction::Failed(failure) => {
                            self.iso_path_setup_transaction = None;
                            self.iso_path_state =
                                IsoPathOperationState::Failed(failure);
                        }
                        IsoPathSetupAction::Ignored => {
                            return Err(EngineFault::UnexpectedIsoControlEvent);
                        }
                    }
                }
                IsoControlEvent::DataPathRemoved {
                    status,
                    connection_handle,
                    directions,
                } => {
                    let Some(transaction) =
                        self.iso_path_remove_transaction.as_mut()
                    else {
                        return Err(EngineFault::UnexpectedIsoControlEvent);
                    };
                    if transaction.connection_handle() != connection_handle
                        || transaction.directions() != directions
                    {
                        return Err(EngineFault::IsoControlJournalMismatch {
                            connection_handle,
                        });
                    }

                    let action = transaction.on_command_complete(
                        bumble_hci::codes::HCI_LE_REMOVE_ISO_DATA_PATH_COMMAND,
                        status,
                        &connection_handle.to_le_bytes(),
                    );
                    match action {
                        IsoPathRemoveAction::Complete {
                            connection_handle,
                            directions,
                        } => {
                            for direction in 0..=1 {
                                if directions & (1 << direction) != 0
                                    && self
                                        .device
                                        .iso_data_path(
                                            connection_handle,
                                            direction,
                                        )
                                        .is_some()
                                {
                                    return Err(
                                        EngineFault::IsoPathProjectionMismatch {
                                            connection_handle,
                                            direction,
                                        },
                                    );
                                }
                            }
                            self.iso_path_remove_transaction = None;
                            self.iso_path_state =
                                IsoPathOperationState::RemoveComplete {
                                    connection_handle,
                                    directions,
                                };
                        }
                        IsoPathRemoveAction::Failed(failure) => {
                            self.iso_path_remove_transaction = None;
                            self.iso_path_state =
                                IsoPathOperationState::Failed(failure);
                        }
                        IsoPathRemoveAction::Ignored => {
                            return Err(EngineFault::UnexpectedIsoControlEvent);
                        }
                    }
                }
                IsoControlEvent::TxSync { .. } => {
                    return Err(EngineFault::UnexpectedIsoControlEvent);
                }
            }
        }
        Ok(())
    }

    fn handle_cis_create_action(&mut self, action: CisCreateAction) {
        let cig_id = self
            .cis_create_transaction
            .as_ref()
            .map(|transaction| transaction.allocation().cig_id);

        match action {
            CisCreateAction::AwaitingEstablished => {
                if let Some(cig_id) = cig_id {
                    self.cis_create_state =
                        CisCreateState::AwaitingEstablished {
                            cig_id,
                            established: Vec::new(),
                        };
                }
            }
            CisCreateAction::Progress(established) => {
                if let Some(cig_id) = cig_id {
                    self.cis_create_state =
                        CisCreateState::AwaitingEstablished {
                            cig_id,
                            established,
                        };
                }
            }
            CisCreateAction::Complete(established) => {
                let cig_id = cig_id.expect(
                    "terminal CIS action missing active transaction",
                );
                self.cis_create_transaction = None;
                self.cis_create_state =
                    CisCreateState::Ready {
                        cig_id,
                        established,
                    };
            }
            CisCreateAction::Failed(failure) => {
                self.cis_create_transaction = None;
                self.cis_create_state =
                    CisCreateState::Failed(failure);
            }
            CisCreateAction::Ignored => {}
        }
    }

    fn process_acl_credit_observation(
        &mut self,
        observation: &HciObservation,
    ) -> Result<(), EngineFault> {
        match observation {
            HciObservation::AclSubmitted {
                connection_handle,
            } => {
                if !self
                    .connection_generations
                    .contains_key(connection_handle)
                {
                    return Err(EngineFault::AclCredit(
                        AclCreditFault::UnknownCompletion {
                            connection_handle: *connection_handle,
                            completed: 0,
                        },
                    ));
                }
                self.acl_credit_audit
                    .submitted(*connection_handle)
                    .map_err(EngineFault::AclCredit)?;
            }
            HciObservation::CompletedPackets { completions } => {
                for (connection_handle, completed) in completions {
                    self.acl_credit_audit
                        .completed(
                            *connection_handle,
                            usize::from(*completed),
                        )
                        .map_err(EngineFault::AclCredit)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn process_controller_identity_observation(
        &mut self,
        observation: &HciObservation,
    ) {
        if !self.controller_identity_pending {
            return;
        }

        let HciObservation::CommandComplete {
            opcode,
            status,
            parameters_after_status,
        } = observation
        else {
            return;
        };
        if *opcode != HCI_READ_BD_ADDR_COMMAND {
            return;
        }

        self.controller_identity_pending = false;
        self.controller_identity_result = Some(match status {
            Some(0) if parameters_after_status.len() == 6 => {
                let mut public_address_le = [0u8; 6];
                public_address_le.copy_from_slice(parameters_after_status);
                Ok(ControllerIdentity { public_address_le })
            }
            Some(0) => Err(ControllerIdentityFailure::MalformedResponse),
            Some(status) => Err(ControllerIdentityFailure::Status(*status)),
            None => Err(ControllerIdentityFailure::MalformedResponse),
        });
    }

    fn process_privacy_observation(
        &mut self,
        observation: &HciObservation,
    ) -> Result<(), EngineFault> {
        let HciObservation::CommandComplete {
            opcode,
            status,
            parameters_after_status,
        } = observation
        else {
            return Ok(());
        };

        if self.privacy_clear_transaction.is_some() {
            let action = self
                .privacy_clear_transaction
                .as_mut()
                .expect("checked above")
                .on_command_complete(
                    *opcode,
                    status.unwrap_or(0xff),
                );
            match action {
                PrivacyClearAction::Send(command) => {
                    self.link.handle_command(0, command);
                    self.check_link_fault()?;
                }
                PrivacyClearAction::Complete => {
                    self.privacy_clear_transaction = None;
                    self.privacy_state = ControllerPrivacyState::Idle;
                }
                PrivacyClearAction::Failed(failure) => {
                    self.privacy_clear_transaction = None;
                    self.privacy_state =
                        ControllerPrivacyState::Failed(failure);
                }
                PrivacyClearAction::Ignored => {}
            }
            return Ok(());
        }

        let action = {
            let Some(transaction) = self.privacy_transaction.as_mut() else {
                return Ok(());
            };
            transaction.on_command_complete(
                *opcode,
                status.unwrap_or(0xff),
                parameters_after_status,
            )
        };

        match action {
            PrivacyAction::Send(command) => {
                self.link.handle_command(0, command);
                self.check_link_fault()?;
            }
            PrivacyAction::Complete => {
                self.privacy_transaction = None;
                self.privacy_state = ControllerPrivacyState::Ready;
            }
            PrivacyAction::Failed(failure) => {
                self.privacy_transaction = None;
                self.privacy_state = ControllerPrivacyState::Failed(failure);
            }
            PrivacyAction::Ignored => {}
        }
        Ok(())
    }

    fn process_cig_observation(
        &mut self,
        observation: &HciObservation,
    ) -> Result<(), EngineFault> {
        let HciObservation::CommandComplete {
            opcode,
            status,
            parameters_after_status,
        } = observation
        else {
            return Ok(());
        };

        if let Some(transaction) = self.cig_remove_transaction.as_mut() {
            match transaction.on_command_complete(
                *opcode,
                status.unwrap_or(0xff),
                parameters_after_status,
            ) {
                CigRemoveAction::Complete { cig_id } => {
                    self.cig_remove_transaction = None;
                    self.cig_remove_state = CigRemoveState::Removed { cig_id };

                    let clear_cig = matches!(
                        &self.cig_configure_state,
                        CigConfigureState::Ready(allocation)
                            if allocation.cig_id == cig_id
                    ) || matches!(
                        &self.cig_configure_state,
                        CigConfigureState::Failed(failure)
                            if failure.cig_id == cig_id
                    );
                    if clear_cig {
                        self.cig_configure_state = CigConfigureState::Idle;
                    }

                    let clear_cis = matches!(
                        &self.cis_create_state,
                        CisCreateState::Failed(failure)
                            if failure.cig_id == cig_id
                                && failure.established.is_empty()
                    );
                    if clear_cis {
                        self.cis_create_state = CisCreateState::Idle;
                    }
                    return Ok(());
                }
                CigRemoveAction::Failed(failure) => {
                    self.cig_remove_transaction = None;
                    self.cig_remove_state = CigRemoveState::Failed(failure);
                    return Ok(());
                }
                CigRemoveAction::Ignored => {}
            }
        }

        let action = {
            let Some(transaction) =
                self.cig_configure_transaction.as_mut()
            else {
                return Ok(());
            };
            transaction.on_command_complete(
                *opcode,
                status.unwrap_or(0xff),
                parameters_after_status,
            )
        };

        match action {
            CigConfigureAction::Complete(allocation) => {
                self.cig_configure_transaction = None;
                self.cig_configure_state =
                    CigConfigureState::Ready(allocation);
            }
            CigConfigureAction::Failed(failure) => {
                self.cig_configure_transaction = None;
                self.cig_configure_state =
                    CigConfigureState::Failed(failure);
            }
            CigConfigureAction::Ignored => {}
        }
        Ok(())
    }

    fn process_stored_encryption_observation(
        &mut self,
        observation: &HciObservation,
    ) -> Result<(), EngineFault> {
        let HciObservation::CommandStatus { opcode, status } = observation else {
            return Ok(());
        };
        if *opcode != HCI_LE_ENABLE_ENCRYPTION_COMMAND {
            return Ok(());
        }

        let action = {
            let Some(transaction) =
                self.stored_encryption_transaction.as_mut()
            else {
                return Ok(());
            };
            transaction.on_command_status(*opcode, *status)
        };

        self.handle_stored_encryption_action(action)
    }

    fn handle_stored_encryption_action(
        &mut self,
        action: StoredEncryptionAction,
    ) -> Result<(), EngineFault> {
        match action {
            StoredEncryptionAction::AwaitChange => {
                self.stored_encryption_state =
                    StoredEncryptionState::AwaitingChange;
            }
            StoredEncryptionAction::Encrypted {
                connection,
                encryption_key_size,
            } => {
                self.validate_connection_token(connection)?;
                if !self
                    .device
                    .is_encrypted_on_handle(connection.connection_handle)
                {
                    return Err(EngineFault::InvalidState);
                }
                self.stored_encryption_transaction = None;
                self.stored_encryption_state =
                    StoredEncryptionState::Encrypted {
                        connection,
                        encryption_key_size,
                    };
            }
            StoredEncryptionAction::Failed(failure) => {
                self.stored_encryption_transaction = None;
                self.stored_encryption_state =
                    StoredEncryptionState::Failed(failure);
            }
            StoredEncryptionAction::Ignored => {}
        }
        Ok(())
    }

    fn process_disconnect_observation(
        &mut self,
        observation: &HciObservation,
    ) -> Result<(), EngineFault> {
        let HciObservation::CommandStatus { opcode, status } = observation else {
            return Ok(());
        };
        if *opcode != HCI_DISCONNECT_COMMAND {
            return Ok(());
        }

        let action = {
            let Some(transaction) = self.disconnect_transaction.as_mut() else {
                return Ok(());
            };
            transaction.on_command_status(*opcode, *status)
        };
        self.handle_disconnect_action(action)
    }

    fn process_cis_disconnect_observation(
        &mut self,
        observation: &HciObservation,
    ) -> Result<(), EngineFault> {
        let HciObservation::CommandStatus { opcode, status } = observation else {
            return Ok(());
        };
        if *opcode != HCI_DISCONNECT_COMMAND {
            return Ok(());
        }

        let action = {
            let Some(transaction) =
                self.cis_disconnect_transaction.as_mut()
            else {
                return Ok(());
            };
            transaction.on_command_status(*opcode, *status)
        };
        self.handle_cis_disconnect_action(action);
        Ok(())
    }

    fn handle_cis_disconnect_action(
        &mut self,
        action: CisDisconnectAction,
    ) {
        match action {
            CisDisconnectAction::AwaitEvent { connection_handle } => {
                self.cis_disconnect_state =
                    CisDisconnectState::AwaitingEvent {
                        connection_handle,
                    };
            }
            CisDisconnectAction::Disconnected {
                connection_handle,
                event_reason,
            } => {
                self.cis_disconnect_transaction = None;
                self.cis_disconnect_state =
                    CisDisconnectState::Disconnected {
                        connection_handle,
                        event_reason,
                    };
            }
            CisDisconnectAction::Failed(failure) => {
                self.cis_disconnect_transaction = None;
                self.cis_disconnect_state =
                    CisDisconnectState::Failed(failure);
            }
            CisDisconnectAction::Ignored => {}
        }
    }

    fn handle_disconnect_action(
        &mut self,
        action: DisconnectAction,
    ) -> Result<(), EngineFault> {
        match action {
            DisconnectAction::AwaitEvent => {
                self.disconnect_state = DisconnectState::AwaitingEvent;
            }
            DisconnectAction::Disconnected {
                connection,
                event_reason,
            } => {
                self.disconnect_transaction = None;
                self.disconnect_state = DisconnectState::Disconnected {
                    connection,
                    event_reason,
                };
            }
            DisconnectAction::Failed(failure) => {
                self.disconnect_transaction = None;
                self.disconnect_state = DisconnectState::Failed(failure);
            }
            DisconnectAction::Ignored => {}
        }
        Ok(())
    }

    fn process_fal_connection_observation(
        &mut self,
        observation: &HciObservation,
    ) -> Result<(), EngineFault> {
        let action = {
            let Some(transaction) =
                self.fal_connection_transaction.as_mut()
            else {
                return Ok(());
            };

            match observation {
                HciObservation::CommandComplete {
                    opcode,
                    status,
                    parameters_after_status,
                } => transaction.on_command_complete(
                    *opcode,
                    status.unwrap_or(0xff),
                    parameters_after_status,
                ),
                HciObservation::CommandStatus { opcode, status } => {
                    transaction.on_command_status(*opcode, *status)
                }
                HciObservation::AclSubmitted { .. }
                | HciObservation::IsoSubmitted { .. }
                | HciObservation::CompletedPackets { .. } => {
                    FalConnectAction::Ignored
                }
            }
        };

        self.handle_fal_connection_action(action)
    }

    fn handle_fal_connection_action(
        &mut self,
        action: FalConnectAction,
    ) -> Result<(), EngineFault> {
        match action {
            FalConnectAction::Send(command) => {
                let opcode = command.op_code();
                if opcode == HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND {
                    self.fal_connection_state =
                        FalConnectionState::Initiating;
                } else if opcode == HCI_LE_CREATE_CONNECTION_CANCEL_COMMAND {
                    self.fal_connection_state =
                        FalConnectionState::Cancelling;
                } else if opcode == HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND
                    && matches!(
                        self.fal_connection_state,
                        FalConnectionState::Initiating
                            | FalConnectionState::AwaitingConnection
                    )
                {
                    self.fal_connection_state =
                        FalConnectionState::Cleaning;
                }

                self.link.handle_command(0, command);
                self.check_link_fault()?;
            }
            FalConnectAction::AwaitConnection => {
                self.fal_connection_state =
                    FalConnectionState::AwaitingConnection;
            }
            FalConnectAction::CancelAccepted => {
                self.fal_connection_state =
                    FalConnectionState::CancelAccepted;
            }
            FalConnectAction::CancelRejected { status } => {
                self.fal_connection_state =
                    FalConnectionState::CancelRejected { status };
            }
            FalConnectAction::Connected(connection) => {
                self.fal_connection_transaction = None;
                self.fal_connection_state =
                    FalConnectionState::Connected(connection);
            }
            FalConnectAction::ConnectedAfterCancel(connection) => {
                self.fal_connection_transaction = None;
                self.fal_connection_state =
                    FalConnectionState::ConnectedAfterCancel(connection);
            }
            FalConnectAction::Failed(failure) => {
                self.fal_connection_transaction = None;
                self.fal_connection_state =
                    FalConnectionState::Failed(failure);
            }
            FalConnectAction::Ignored => {}
        }
        Ok(())
    }

    fn process_gatt_inbox(&mut self) -> Result<(), EngineFault> {
        let connections = self
            .connection_generations
            .iter()
            .map(|(handle, generation)| {
                (*handle, ConnectionToken {
                    connection_handle: *handle,
                    generation: *generation,
                })
            })
            .collect::<Vec<_>>();

        for (handle, connection) in connections {
            for pdu in self.device.take_inbox_on_handle(handle) {
                match pdu {
                    bumble_att::AttPdu::HandleValueNotification {
                        attribute_handle,
                        attribute_value,
                    } => {
                        self.push_gatt_report(GattReport::Value(
                            GattValueEvent::Notification {
                                connection,
                                attribute_handle,
                                value: attribute_value,
                            },
                        ))?;
                    }
                    bumble_att::AttPdu::HandleValueIndication {
                        attribute_handle,
                        attribute_value,
                    } => {
                        if !self.device.send_att_on_handle(
                            &mut self.link,
                            handle,
                            &bumble_att::AttPdu::HandleValueConfirmation,
                        ) {
                            self.check_link_fault()?;
                            return Err(EngineFault::GattSendRejected {
                                connection_handle: handle,
                            });
                        }
                        self.check_link_fault()?;
                        self.push_gatt_report(GattReport::Value(
                            GattValueEvent::Indication {
                                connection,
                                attribute_handle,
                                value: attribute_value,
                            },
                        ))?;
                    }
                    other => {
                        let opcode = other
                            .to_bytes()
                            .first()
                            .copied()
                            .unwrap_or(0);
                        let action = if let Some(transaction) =
                            self.gatt_transactions.get_mut(&handle)
                        {
                            Some(transaction.on_pdu(other))
                        } else {
                            None
                        };

                        if let Some(action) = action {
                            self.handle_gatt_action(handle, action)?;
                        } else {
                            if let Some(request_id) =
                                self.gatt_poisoned.get(&handle).copied()
                            {
                                self.push_gatt_report(
                                    GattReport::LateResponseDropped {
                                        connection,
                                        request_id,
                                        opcode,
                                    },
                                )?;
                            } else {
                                self.push_gatt_report(
                                    GattReport::UnexpectedPdu {
                                        connection,
                                        opcode,
                                    },
                                )?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn process_gatt_device_event(
        &mut self,
        event: &DeviceEvent,
    ) -> Result<bool, EngineFault> {
        let DeviceEvent::Disconnected {
            connection_handle,
            reason,
        } = event
        else {
            return Ok(false);
        };

        let action = self
            .gatt_transactions
            .get_mut(connection_handle)
            .map(|transaction| transaction.on_disconnect(*reason));
        let consumed = action.is_some();
        if let Some(action) = action {
            self.handle_gatt_action(*connection_handle, action)?;
        }

        self.gatt_mtus.remove(connection_handle);
        self.gatt_poisoned.remove(connection_handle);
        Ok(consumed)
    }

    fn handle_gatt_action(
        &mut self,
        handle: u16,
        mut action: GattAction,
    ) -> Result<(), EngineFault> {
        loop {
            match action {
                GattAction::Send(pdu) => {
                    if !self.device.send_att_on_handle(
                        &mut self.link,
                        handle,
                        &pdu,
                    ) {
                        self.check_link_fault()?;
                        self.gatt_transactions.remove(&handle);
                        return Err(EngineFault::GattSendRejected {
                            connection_handle: handle,
                        });
                    }
                    self.check_link_fault()?;
                    action = self
                        .gatt_transactions
                        .get_mut(&handle)
                        .map(GattTransaction::on_request_sent)
                        .unwrap_or(GattAction::Ignored);
                }
                GattAction::Await | GattAction::Ignored => return Ok(()),
                GattAction::Complete(result) => {
                    let transaction = self
                        .gatt_transactions
                        .remove(&handle)
                        .ok_or(EngineFault::InvalidState)?;
                    let request_id = transaction.request_id();
                    let connection = transaction.connection();
                    if let GattResult::Mtu { mtu } = &result {
                        self.gatt_mtus.insert(handle, *mtu);
                    }
                    self.push_gatt_report(GattReport::Complete {
                        request_id,
                        connection,
                        result,
                    })?;
                    return Ok(());
                }
                GattAction::Failed(failure) => {
                    self.gatt_transactions.remove(&handle);
                    self.push_gatt_report(GattReport::Failed(failure))?;
                    return Ok(());
                }
            }
        }
    }

    fn push_gatt_report(
        &mut self,
        report: GattReport,
    ) -> Result<(), EngineFault> {
        if self.gatt_reports.len() >= MAX_GATT_REPORTS {
            return Err(EngineFault::GattReportBacklogFull);
        }
        self.gatt_reports.push_back(report);
        Ok(())
    }

    fn process_device_events(&mut self) -> Result<(), EngineFault> {
        for event in self.device.take_device_events() {
            self.prepare_connection_generation(&event);
            let is_flush = matches!(&event, DeviceEvent::Flush);

            let mut consumed = false;
            consumed |= self.process_gatt_device_event(&event)?;
            consumed |= self.process_disconnect_device_event(&event)?;
            consumed |= self.process_cis_disconnect_device_event(&event)?;
            consumed |=
                self.process_stored_encryption_device_event(&event)?;
            consumed |= self.process_fal_connection_device_event(&event)?;

            let retire_handle = match &event {
                DeviceEvent::Disconnected {
                    connection_handle,
                    ..
                } => Some(*connection_handle),
                _ => None,
            };

            if !consumed {
                match event {
                    DeviceEvent::Flush
                    | DeviceEvent::Advertisement(_)
                    | DeviceEvent::AdvertisingReport(_)
                    | DeviceEvent::ExtendedAdvertisingReport(_) => {}
                    other => {
                        if self.deferred_device_events.len()
                            >= MAX_DEFERRED_DEVICE_EVENTS
                        {
                            return Err(EngineFault::DeviceEventBacklogFull);
                        }
                        self.deferred_device_events.push_back(other);
                    }
                }
            }

            if let Some(handle) = retire_handle {
                self.acl_credit_audit.flush(handle);
                self.connection_generations.remove(&handle);
            }

            if is_flush {
                self.acl_credit_audit.flush_all();
            }
        }
        Ok(())
    }

    fn prepare_connection_generation(&mut self, event: &DeviceEvent) {
        let DeviceEvent::LeConnectionEstablished(info) = event else {
            return;
        };

        if self
            .connection_generations
            .contains_key(&info.connection_handle)
        {
            return;
        }

        let generation = self.next_connection_generation;
        self.next_connection_generation =
            self.next_connection_generation.saturating_add(1);
        self.connection_generations
            .insert(info.connection_handle, generation);
    }

    fn process_stored_encryption_device_event(
        &mut self,
        event: &DeviceEvent,
    ) -> Result<bool, EngineFault> {
        let encryption_event = match event {
            DeviceEvent::EncryptionChange {
                status,
                connection_handle,
                encryption_enabled,
                encryption_key_size,
            } => Some(EncryptionEvent::Change {
                status: *status,
                connection_handle: *connection_handle,
                encryption_enabled: *encryption_enabled,
                encryption_key_size: *encryption_key_size,
            }),
            DeviceEvent::Disconnected {
                connection_handle,
                reason,
            } => Some(EncryptionEvent::Disconnected {
                connection_handle: *connection_handle,
                reason: *reason,
            }),
            _ => None,
        };

        let Some(encryption_event) = encryption_event else {
            return Ok(false);
        };

        let action = {
            let Some(transaction) =
                self.stored_encryption_transaction.as_mut()
            else {
                return Ok(false);
            };
            transaction.on_event(encryption_event)
        };
        if matches!(action, StoredEncryptionAction::Ignored) {
            return Ok(false);
        }

        self.handle_stored_encryption_action(action)?;
        Ok(true)
    }

    fn cis_state_contains_established_handle(
        &self,
        connection_handle: u16,
    ) -> bool {
        match &self.cis_create_state {
            CisCreateState::Ready { established, .. } => established
                .iter()
                .any(|cis| cis.link.connection_handle == connection_handle),
            CisCreateState::Failed(failure) => failure
                .established
                .iter()
                .any(|cis| cis.link.connection_handle == connection_handle),
            _ => false,
        }
    }

    fn retire_established_cis(&mut self, connection_handle: u16) -> bool {
        let mut become_idle = false;
        let known = match &mut self.cis_create_state {
            CisCreateState::Ready { established, .. } => {
                let before = established.len();
                established.retain(|cis| {
                    cis.link.connection_handle != connection_handle
                });
                let known = established.len() != before;
                become_idle = known && established.is_empty();
                known
            }
            CisCreateState::Failed(failure) => {
                let before = failure.established.len();
                failure.established.retain(|cis| {
                    cis.link.connection_handle != connection_handle
                });
                failure.established.len() != before
            }
            _ => false,
        };

        if become_idle {
            self.cis_create_state = CisCreateState::Idle;
        }
        known
    }

    fn process_cis_disconnect_device_event(
        &mut self,
        event: &DeviceEvent,
    ) -> Result<bool, EngineFault> {
        let cis_event = match event {
            DeviceEvent::Disconnected {
                connection_handle,
                reason,
            } => Some(CisDisconnectEvent::Disconnected {
                connection_handle: *connection_handle,
                reason: *reason,
            }),
            DeviceEvent::DisconnectionFailed {
                connection_handle,
                status,
            } => Some(CisDisconnectEvent::Failed {
                connection_handle: *connection_handle,
                status: *status,
            }),
            _ => None,
        };
        let Some(cis_event) = cis_event else {
            return Ok(false);
        };

        let successful_disconnect = match &cis_event {
            CisDisconnectEvent::Disconnected {
                connection_handle,
                reason,
            } => Some((*connection_handle, *reason)),
            CisDisconnectEvent::Failed { .. } => None,
        };

        let action = self
            .cis_disconnect_transaction
            .as_mut()
            .map(|transaction| transaction.on_event(cis_event));

        if let Some(action) = action {
            if !matches!(action, CisDisconnectAction::Ignored) {
                if let Some((connection_handle, _)) = successful_disconnect {
                    if !self.retire_established_cis(connection_handle) {
                        return Err(EngineFault::InvalidState);
                    }
                }
                self.handle_cis_disconnect_action(action);
                return Ok(true);
            }
        }

        if let Some((connection_handle, reason)) = successful_disconnect {
            if self.retire_established_cis(connection_handle) {
                self.cis_disconnect_state =
                    CisDisconnectState::Disconnected {
                        connection_handle,
                        event_reason: reason,
                    };
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn process_disconnect_device_event(
        &mut self,
        event: &DeviceEvent,
    ) -> Result<bool, EngineFault> {
        let disconnect_event = match event {
            DeviceEvent::Disconnected {
                connection_handle,
                reason,
            } => Some(DisconnectEvent::Disconnected {
                connection_handle: *connection_handle,
                reason: *reason,
            }),
            DeviceEvent::DisconnectionFailed {
                connection_handle,
                status,
            } => Some(DisconnectEvent::Failed {
                connection_handle: *connection_handle,
                status: *status,
            }),
            _ => None,
        };

        let Some(disconnect_event) = disconnect_event else {
            return Ok(false);
        };

        let action = {
            let Some(transaction) = self.disconnect_transaction.as_mut() else {
                return Ok(false);
            };
            transaction.on_event(disconnect_event)
        };
        if matches!(action, DisconnectAction::Ignored) {
            return Ok(false);
        }

        self.handle_disconnect_action(action)?;
        Ok(true)
    }

    fn process_fal_connection_device_event(
        &mut self,
        event: &DeviceEvent,
    ) -> Result<bool, EngineFault> {
        let connection_event = match event {
            DeviceEvent::LeConnectionEstablished(info) => {
                let generation = *self
                    .connection_generations
                    .get(&info.connection_handle)
                    .expect("established LE connection missing generation");
                Some(ConnectionEvent::Established(
                    snapshot_le_connection(info, generation),
                ))
            }
            DeviceEvent::ConnectionFailed {
                transport: DeviceConnectionTransport::Le,
                status,
                ..
            } => Some(ConnectionEvent::Failed { status: *status }),
            _ => None,
        };

        let Some(connection_event) = connection_event else {
            return Ok(false);
        };

        let action = {
            let Some(transaction) =
                self.fal_connection_transaction.as_mut()
            else {
                return Ok(false);
            };
            transaction.on_connection_event(connection_event)
        };

        self.handle_fal_connection_action(action)?;
        Ok(true)
    }

    fn process_scan_observation(
        &mut self,
        observation: &HciObservation,
    ) -> Result<(), EngineFault> {
        let (opcode, status) = match observation {
            HciObservation::CommandComplete {
                opcode,
                status,
                ..
            } => (*opcode, status.unwrap_or(0xff)),
            HciObservation::CommandStatus { opcode, status } => {
                (*opcode, *status)
            }
            HciObservation::AclSubmitted { .. }
            | HciObservation::IsoSubmitted { .. }
            | HciObservation::CompletedPackets { .. } => return Ok(()),
        };

        if self.scan_pending_opcode != Some(opcode) {
            return Ok(());
        }

        if status != 0 {
            self.scan_pending_opcode = None;
            self.scan_state =
                PassiveScanState::Failed { opcode, status };
            return Ok(());
        }

        match self.scan_state {
            PassiveScanState::Configuring => {
                let command = Command::LeSetExtendedScanEnable {
                    enable: 1,
                    filter_duplicates: 1,
                    duration: 0,
                    period: 0,
                };
                self.scan_pending_opcode = Some(command.op_code());
                self.link.handle_command(0, command);
                self.check_link_fault()?;
                self.scan_state = PassiveScanState::Enabling;
            }
            PassiveScanState::Enabling => {
                self.scan_pending_opcode = None;
                self.scan_state = PassiveScanState::Scanning;
            }
            PassiveScanState::Disabling => {
                self.scan_pending_opcode = None;
                self.completed_scan = Some(self.scan_accumulator.summary());
                self.scan_state = PassiveScanState::Idle;
            }
            PassiveScanState::Idle
            | PassiveScanState::Scanning
            | PassiveScanState::Failed { .. } => {}
        }
        Ok(())
    }

    fn refresh_attach_state(&mut self) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Discovering {
            return Ok(());
        }

        if let Some(status) = self.device.local_supported_commands_status() {
            if status != 0 {
                self.attach_state = OperationalAttachState::Failed(
                    OperationalAttachFailure::SupportedCommandsStatus(status),
                );
                return Ok(());
            }
        }

        if self.device.host_initialization_complete() {
            if self.device.host_initialization_succeeded() {
                self.acl_credit_audit
                    .configure(self.device.le_acl_max_in_flight())
                    .map_err(EngineFault::AclCredit)?;
                self.attach_state = OperationalAttachState::Ready;
            } else {
                self.attach_state = OperationalAttachState::Failed(
                    OperationalAttachFailure::HostInitializationFailed,
                );
            }
        }
        Ok(())
    }

    pub(crate) fn device(&self) -> &Device {
        &self.device
    }

    pub(crate) fn device_mut(&mut self) -> &mut Device {
        &mut self.device
    }

    pub(crate) fn link_mut(&mut self) -> &mut QueueHostTransport {
        &mut self.link
    }
}

fn snapshot_le_connection(
    info: &bumble_host::LeConnectionInfo,
    generation: u64,
) -> LeConnectionSnapshot {
    LeConnectionSnapshot {
        connection_handle: info.connection_handle,
        generation,
        role: info.role,
        peer_address_le: *info.peer_address.address_bytes(),
        peer_address_type: info.peer_address.address_type().0,
        connection_interval: info.parameters.connection_interval,
        peripheral_latency: info.parameters.peripheral_latency,
        supervision_timeout: info.parameters.supervision_timeout,
        encryption_enabled: info.encryption_enabled,
        encryption_key_size: info.encryption_key_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        runtime::mailbox::RecvError,
        transport::packet::HostPacket,
    };

    fn engine(
        capacity: usize,
    ) -> (
        HciEngine,
        crate::runtime::mailbox::BoundedReceiver<HostPacket>,
    ) {
        let (tx, rx) = TransportTx::test_channel(capacity);
        (HciEngine::new(tx), rx)
    }

    #[test]
    fn operational_attach_starts_with_supported_commands_and_never_reset() {
        let (mut engine, rx) = engine(8);

        engine.begin_operational_attach().unwrap();

        assert_eq!(
            rx.try_recv().unwrap(),
            HostPacket::Command(vec![0x02, 0x10, 0x00])
        );
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
        assert_eq!(
            engine.attach_state(),
            OperationalAttachState::Discovering
        );
        assert!(engine.device().controller_ready());
        assert_eq!(engine.device().reset_status(), None);
    }

    #[test]
    fn minimal_operational_attach_reaches_ready_without_reset() {
        let (mut engine, rx) = engine(8);
        engine.begin_operational_attach().unwrap();

        // Discard initial Read Local Supported Commands.
        assert!(matches!(
            rx.try_recv().unwrap(),
            HostPacket::Command(_)
        ));

        // Supported Commands success with an all-zero bitmap. That deliberately
        // disables every optional capability branch; Device should proceed only
        // with the mandatory Event Mask + LE Event Mask host initialization.
        let mut supported = vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        engine
            .ingest(ControllerPacket::Event(supported))
            .unwrap();
        assert_eq!(engine.poll_until_quiescent(), Ok(1));

        let first = rx.try_recv().unwrap();
        let second = rx.try_recv().unwrap();
        assert!(matches!(first, HostPacket::Command(_)));
        assert!(matches!(second, HostPacket::Command(_)));
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));

        // Complete the two mandatory host-initialization commands as
        // independent controller events. Device::poll() drains every event
        // currently queued, so each injected completion needs only one
        // quiescent drive; a second poll is expected to observe no work.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        assert_eq!(engine.poll_until_quiescent(), Ok(1));

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(engine.poll_until_quiescent(), Ok(1));
        assert_eq!(engine.poll_until_quiescent(), Ok(0));
        assert_eq!(engine.attach_state(), OperationalAttachState::Ready);
        assert_eq!(engine.device().reset_status(), None);

        let capabilities = engine.capabilities().unwrap();
        assert_eq!(capabilities.supported_commands, [0u8; 64]);
        assert_eq!(capabilities.hci_version, None);
        assert_eq!(capabilities.le_acl_pool, None);
        assert_eq!(capabilities.iso_pool, None);
    }

    fn make_minimal_attach_ready(
        engine: &mut HciEngine,
        rx: &crate::runtime::mailbox::BoundedReceiver<HostPacket>,
    ) {
        engine.begin_operational_attach().unwrap();
        let _ = rx.try_recv().unwrap();

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        engine
            .ingest(ControllerPacket::Event(supported))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        let _ = rx.try_recv().unwrap();
        let _ = rx.try_recv().unwrap();

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(engine.attach_state(), OperationalAttachState::Ready);
    }

    fn command_opcode(packet: HostPacket) -> u16 {
        let HostPacket::Command(bytes) = packet else {
            panic!("expected HCI command");
        };
        u16::from_le_bytes([bytes[0], bytes[1]])
    }

    fn complete(opcode: u16, status: u8, extra: &[u8]) -> ControllerPacket {
        let mut event = vec![
            0x0e,
            (4 + extra.len()) as u8,
            0x01,
            opcode as u8,
            (opcode >> 8) as u8,
            status,
        ];
        event.extend_from_slice(extra);
        ControllerPacket::Event(event)
    }

    fn command_status(opcode: u16, status: u8) -> ControllerPacket {
        ControllerPacket::Event(vec![
            0x0f,
            0x04,
            status,
            0x01,
            opcode as u8,
            (opcode >> 8) as u8,
        ])
    }

    fn le_connection_complete(
        handle: u16,
        peer_address_le: [u8; 6],
    ) -> ControllerPacket {
        let [handle_lo, handle_hi] = handle.to_le_bytes();
        let mut event = vec![
            0x3e, 0x13, 0x01, 0x00,
            handle_lo, handle_hi,
            0x00,
            0x01,
        ];
        event.extend_from_slice(&peer_address_le);
        event.extend_from_slice(&[
            0x18, 0x00,
            0x00, 0x00,
            0x2a, 0x00,
            0x00,
        ]);
        ControllerPacket::Event(event)
    }

    fn cis_established(
        handle: u16,
        status: u8,
    ) -> ControllerPacket {
        let [handle_lo, handle_hi] = handle.to_le_bytes();
        ControllerPacket::Event(vec![
            0x3e, 0x1d, 0x19,
            status,
            handle_lo, handle_hi,
            0x40, 0x9c, 0x00,
            0x20, 0x4e, 0x00,
            0xb8, 0x88, 0x00,
            0x00, 0x00, 0x00,
            0x02,
            0x01,
            0x02,
            0x01,
            0x00,
            0x02,
            0x00,
            0x64, 0x00,
            0x00, 0x00,
            0x08, 0x00,
        ])
    }

    fn cig_config() -> super::super::iso::CigConfig {
        let cis = |cis_id| super::super::iso::CisConfig {
            cis_id,
            max_sdu_c_to_p: 100,
            max_sdu_p_to_c: 0,
            phy_c_to_p: 2,
            phy_p_to_c: 1,
            rtn_c_to_p: 13,
            rtn_p_to_c: 0,
        };
        super::super::iso::CigConfig {
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

    fn single_cig_config() -> super::super::iso::CigConfig {
        let mut config = cig_config();
        config.cis.truncate(1);
        config
    }

    #[test]
    fn engine_drives_cig_configuration_from_completion_journal() {
        let (mut engine, rx) = engine(16);
        make_minimal_attach_ready(&mut engine, &rx);

        engine.begin_cig_configuration(cig_config()).unwrap();
        assert_eq!(command_opcode(rx.try_recv().unwrap()), 0x2062);
        assert_eq!(
            engine.cig_configure_state(),
            &super::super::iso::CigConfigureState::Configuring
        );

        engine
            .ingest(complete(
                0x2062,
                0,
                &[1, 2, 0x40, 0x00, 0x41, 0x00],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            engine.cig_configure_state(),
            &super::super::iso::CigConfigureState::Ready(
                super::super::iso::CigAllocation {
                    cig_id: 1,
                    cis: vec![
                        super::super::iso::CisAllocation {
                            cis_id: 1,
                            connection_handle: 0x0040,
                        },
                        super::super::iso::CisAllocation {
                            cis_id: 2,
                            connection_handle: 0x0041,
                        },
                    ],
                }
            )
        );
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
    }

    #[test]
    fn engine_removes_cig_only_after_controller_confirms_matching_id() {
        let (mut engine, rx) = engine(16);
        make_minimal_attach_ready(&mut engine, &rx);

        engine.begin_cig_configuration(cig_config()).unwrap();
        assert_eq!(command_opcode(rx.try_recv().unwrap()), 0x2062);
        engine
            .ingest(complete(
                0x2062,
                0,
                &[1, 2, 0x40, 0x00, 0x41, 0x00],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        engine.begin_cig_removal(1).unwrap();
        assert_eq!(command_opcode(rx.try_recv().unwrap()), 0x2065);
        assert_eq!(
            engine.cig_remove_state(),
            &super::super::iso_lifecycle::CigRemoveState::Removing {
                cig_id: 1,
            }
        );

        engine.ingest(complete(0x2065, 0, &[1])).unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            engine.cig_remove_state(),
            &super::super::iso_lifecycle::CigRemoveState::Removed {
                cig_id: 1,
            }
        );
        assert_eq!(
            engine.cig_configure_state(),
            &super::super::iso::CigConfigureState::Idle
        );
        assert_eq!(
            engine.cis_create_state(),
            &super::super::iso::CisCreateState::Idle
        );
    }

    #[test]
    fn engine_creates_two_cis_from_typed_bumble_control_journal() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine
            .ingest(le_connection_complete(
                0x0800,
                [1, 2, 3, 4, 5, 6],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        engine
            .ingest(le_connection_complete(
                0x0801,
                [7, 8, 9, 10, 11, 12],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        let snapshot = engine.connection_set_snapshot();
        let first = ConnectionToken {
            connection_handle: snapshot.connections[0].connection_handle,
            generation: snapshot.connections[0].generation,
        };
        let second = ConnectionToken {
            connection_handle: snapshot.connections[1].connection_handle,
            generation: snapshot.connections[1].generation,
        };

        engine.begin_cig_configuration(cig_config()).unwrap();
        assert_eq!(command_opcode(rx.try_recv().unwrap()), 0x2062);
        engine
            .ingest(complete(
                0x2062,
                0,
                &[1, 2, 0x00, 0x09, 0x01, 0x09],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        engine
            .begin_cis_creation(
                super::super::iso::CisCreateRequest {
                    cig_id: 1,
                    bindings: vec![
                        super::super::iso::CisAclBinding {
                            cis_id: 1,
                            acl: first,
                        },
                        super::super::iso::CisAclBinding {
                            cis_id: 2,
                            acl: second,
                        },
                    ],
                },
            )
            .unwrap();

        let create_opcode = command_opcode(rx.try_recv().unwrap());
        assert_eq!(
            create_opcode,
            bumble_hci::codes::HCI_LE_CREATE_CIS_COMMAND
        );
        assert_eq!(
            engine.cis_create_state(),
            &super::super::iso::CisCreateState::Submitting { cig_id: 1 }
        );

        engine.ingest(command_status(create_opcode, 0)).unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.cis_create_state(),
            &super::super::iso::CisCreateState::AwaitingEstablished {
                cig_id: 1,
                established: vec![],
            }
        );

        engine.ingest(cis_established(0x0901, 0)).unwrap();
        engine.poll_until_quiescent().unwrap();
        assert!(matches!(
            engine.cis_create_state(),
            super::super::iso::CisCreateState::AwaitingEstablished {
                cig_id: 1,
                established,
            } if established.len() == 1
                && established[0].cis_id == 2
        ));

        engine.ingest(cis_established(0x0900, 0)).unwrap();
        engine.poll_until_quiescent().unwrap();

        let super::super::iso::CisCreateState::Ready {
            cig_id,
            established,
        } = engine.cis_create_state()
        else {
            panic!("expected Ready CIS set");
        };
        assert_eq!(*cig_id, 1);
        assert_eq!(
            established.iter().map(|cis| cis.cis_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(established[0].acl, first);
        assert_eq!(established[1].acl, second);
        assert!(engine.device().cis_link(0x0900).is_some());
        assert!(engine.device().cis_link(0x0901).is_some());

        // A remote/controller-initiated CIS teardown must update Review's
        // live projection even when no local Disconnect transaction exists.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x05, 0x04, 0x00, 0x01, 0x09, 0x16,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert!(engine.device().cis_link(0x0901).is_none());
        assert!(engine.device().cis_link(0x0900).is_some());
        assert!(matches!(
            engine.cis_create_state(),
            super::super::iso::CisCreateState::Ready {
                cig_id: 1,
                established,
            } if established.len() == 1
                && established[0].link.connection_handle == 0x0900
        ));
        assert_eq!(
            engine.begin_cig_removal(1),
            Err(EngineFault::InvalidState)
        );

        engine
            .ingest(ControllerPacket::Event(vec![
                0x05, 0x04, 0x00, 0x00, 0x09, 0x16,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert!(engine.device().cis_link(0x0900).is_none());
        assert_eq!(
            engine.cis_create_state(),
            &super::super::iso::CisCreateState::Idle
        );

        engine.begin_cig_removal(1).unwrap();
        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            bumble_hci::codes::HCI_LE_REMOVE_CIG_COMMAND
        );
    }

    #[test]
    fn engine_sets_up_and_removes_hci_input_path_through_bumble_projection() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine
            .ingest(le_connection_complete(
                0x0800,
                [1, 2, 3, 4, 5, 6],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        let acl = engine.connection_set_snapshot().connections[0].token();

        engine.begin_cig_configuration(single_cig_config()).unwrap();
        assert_eq!(command_opcode(rx.try_recv().unwrap()), 0x2062);
        engine
            .ingest(complete(
                0x2062,
                0,
                &[1, 1, 0x00, 0x09],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        engine
            .begin_cis_creation(
                super::super::iso::CisCreateRequest {
                    cig_id: 1,
                    bindings: vec![super::super::iso::CisAclBinding {
                        cis_id: 1,
                        acl,
                    }],
                },
            )
            .unwrap();
        let create_opcode = command_opcode(rx.try_recv().unwrap());
        assert_eq!(
            create_opcode,
            bumble_hci::codes::HCI_LE_CREATE_CIS_COMMAND
        );
        engine.ingest(command_status(create_opcode, 0)).unwrap();
        engine.poll_until_quiescent().unwrap();
        engine.ingest(cis_established(0x0900, 0)).unwrap();
        engine.poll_until_quiescent().unwrap();
        assert!(matches!(
            engine.cis_create_state(),
            super::super::iso::CisCreateState::Ready { established, .. }
                if established.len() == 1
                    && established[0].link.connection_handle == 0x0900
        ));

        let setup =
            super::super::iso_path::IsoDataPathConfig::hci_input(0x0900);
        engine.begin_iso_path_setup(setup.clone()).unwrap();
        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            bumble_hci::codes::HCI_LE_SETUP_ISO_DATA_PATH_COMMAND
        );
        assert_eq!(
            engine.iso_path_state(),
            &super::super::iso_path::IsoPathOperationState::SettingUp(
                setup.clone()
            )
        );

        engine
            .ingest(complete(
                bumble_hci::codes::HCI_LE_SETUP_ISO_DATA_PATH_COMMAND,
                0,
                &[0x00, 0x09],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            engine.iso_path_state(),
            &super::super::iso_path::IsoPathOperationState::SetupComplete {
                connection_handle: 0x0900,
                direction:
                    super::super::iso_path::IsoDataPathDirection::Input,
            }
        );
        assert_eq!(
            engine.device().iso_data_path(0x0900, 0),
            Some(&setup.to_bumble_parameters())
        );

        engine.begin_iso_path_remove(0x0900, 0x01).unwrap();
        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            bumble_hci::codes::HCI_LE_REMOVE_ISO_DATA_PATH_COMMAND
        );
        assert_eq!(
            engine.iso_path_state(),
            &super::super::iso_path::IsoPathOperationState::Removing {
                connection_handle: 0x0900,
                directions: 0x01,
            }
        );

        engine
            .ingest(complete(
                bumble_hci::codes::HCI_LE_REMOVE_ISO_DATA_PATH_COMMAND,
                0,
                &[0x00, 0x09],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            engine.iso_path_state(),
            &super::super::iso_path::IsoPathOperationState::RemoveComplete {
                connection_handle: 0x0900,
                directions: 0x01,
            }
        );
        assert!(engine.device().iso_data_path(0x0900, 0).is_none());

        engine.begin_cis_disconnect(0x0900).unwrap();
        let HostPacket::Command(disconnect) = rx.try_recv().unwrap() else {
            panic!("expected CIS Disconnect command");
        };
        assert_eq!(
            u16::from_le_bytes([disconnect[0], disconnect[1]]),
            HCI_DISCONNECT_COMMAND
        );
        assert_eq!(
            disconnect.last().copied(),
            Some(super::super::iso_lifecycle::
                CIS_DISCONNECT_REASON_REMOTE_USER_TERMINATED)
        );

        engine
            .ingest(command_status(HCI_DISCONNECT_COMMAND, 0))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.cis_disconnect_state(),
            &super::super::iso_lifecycle::CisDisconnectState::AwaitingEvent {
                connection_handle: 0x0900,
            }
        );

        engine
            .ingest(ControllerPacket::Event(vec![
                0x05, 0x04, 0x00, 0x00, 0x09, 0x16,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.cis_disconnect_state(),
            &super::super::iso_lifecycle::CisDisconnectState::Disconnected {
                connection_handle: 0x0900,
                event_reason: 0x16,
            }
        );
        assert!(engine.device().cis_link(0x0900).is_none());

        engine.begin_cig_removal(1).unwrap();
        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            bumble_hci::codes::HCI_LE_REMOVE_CIG_COMMAND
        );
        engine
            .ingest(complete(
                bumble_hci::codes::HCI_LE_REMOVE_CIG_COMMAND,
                0,
                &[1],
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.cig_remove_state(),
            &super::super::iso_lifecycle::CigRemoveState::Removed {
                cig_id: 1,
            }
        );
        assert_eq!(
            engine.cig_configure_state(),
            &super::super::iso::CigConfigureState::Idle
        );
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
    }

    #[test]
    fn engine_drives_resolving_list_transaction_from_completion_journal() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine
            .begin_controller_privacy(ResolvingListConfig {
                peers: vec![super::super::privacy::ResolvingPeer {
                    identity_address_le: [0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8],
                    identity_address_type: 0,
                    peer_irk: [0x11; 16],
                }],
                local_irk: [0x22; 16],
                rpa_timeout_seconds:
                    super::super::privacy::DEFAULT_RPA_TIMEOUT_SECONDS,
            })
            .unwrap();

        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            0x202d
        );
        assert!(matches!(
            engine.controller_privacy_state(),
            ControllerPrivacyState::Configuring
        ));

        for (opcode, extra, next_opcode) in [
            (0x202d, &[][..], 0x2029),
            (0x2029, &[][..], 0x202a),
            (0x202a, &[1][..], 0x2027),
            (0x2027, &[][..], 0x202e),
            (0x202e, &[][..], 0x202d),
        ] {
            engine.ingest(complete(opcode, 0, extra)).unwrap();
            engine.poll_until_quiescent().unwrap();
            assert_eq!(
                command_opcode(rx.try_recv().unwrap()),
                next_opcode
            );
        }

        engine.ingest(complete(0x202d, 0, &[])).unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.controller_privacy_state(),
            &ControllerPrivacyState::Ready
        );
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
    }

    #[test]
    fn controller_identity_probe_is_journal_only_and_has_no_bumble_side_effect() {
        let (mut engine, rx) = engine(16);
        make_minimal_attach_ready(&mut engine, &rx);
        assert_eq!(engine.device().public_address(), None);

        engine.begin_controller_identity_probe().unwrap();
        assert_eq!(command_opcode(rx.try_recv().unwrap()), 0x1009);

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x0a, 0x01, 0x09, 0x10, 0x00,
                0x1a, 0x94, 0x36, 0xbb, 0xa3, 0x44,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            engine.take_controller_identity_result(),
            Some(Ok(ControllerIdentity {
                public_address_le: [
                    0x1a, 0x94, 0x36, 0xbb, 0xa3, 0x44,
                ],
            }))
        );
        assert_eq!(
            engine.device().public_address(),
            None,
            "journal-only identity probe must not trigger Bumble key-store/privacy side effects"
        );
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
    }

    #[test]
    fn engine_clears_controller_privacy_as_typed_transaction() {
        let (mut engine, rx) = engine(16);
        make_minimal_attach_ready(&mut engine, &rx);

        engine.begin_controller_privacy_clear().unwrap();
        assert_eq!(command_opcode(rx.try_recv().unwrap()), 0x202d);
        assert_eq!(
            engine.controller_privacy_state(),
            &ControllerPrivacyState::Clearing
        );

        engine.ingest(complete(0x202d, 0, &[])).unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(command_opcode(rx.try_recv().unwrap()), 0x2029);

        engine.ingest(complete(0x2029, 0, &[])).unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.controller_privacy_state(),
            &ControllerPrivacyState::Idle
        );
    }

    #[test]
    fn engine_routes_bumble_connection_event_into_fal_cleanup() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine
            .begin_fal_connection(
                super::super::connection::FalConnectRequest {
                    peer: super::super::connection::LePeerIdentity {
                        address_le: [
                            0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8,
                        ],
                        address_type: 0,
                    },
                    parameters:
                        super::super::connection::LeInitiationParameters::default(),
                },
            )
            .unwrap();

        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            0x200f
        );

        for (opcode, extra, next) in [
            (0x200f, &[8][..], 0x2010),
            (0x2010, &[][..], 0x2011),
            (0x2011, &[][..], 0x2043),
        ] {
            engine.ingest(complete(opcode, 0, extra)).unwrap();
            engine.poll_until_quiescent().unwrap();
            assert_eq!(
                command_opcode(rx.try_recv().unwrap()),
                next
            );
        }

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0f, 0x04, 0x00, 0x01, 0x43, 0x20,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.fal_connection_state(),
            &FalConnectionState::AwaitingConnection
        );

        // LE Connection Complete: status 0, handle 0x0800, central,
        // random peer, interval 24, latency 0, timeout 42.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            0x2010
        );
        assert_eq!(
            engine.fal_connection_state(),
            &FalConnectionState::Cleaning
        );

        engine.ingest(complete(0x2010, 0, &[])).unwrap();
        engine.poll_until_quiescent().unwrap();

        match engine.fal_connection_state() {
            FalConnectionState::Connected(connection) => {
                assert_eq!(connection.connection_handle, 0x0800);
                assert_eq!(connection.role, 0);
                assert_eq!(connection.peer_address_type, 1);
                assert_eq!(
                    connection.peer_address_le,
                    [1, 2, 3, 4, 5, 6]
                );
                assert_eq!(connection.connection_interval, 24);
                assert_eq!(connection.supervision_timeout, 42);
            }
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[test]
    fn engine_projects_two_connection_local_acl_states() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        for event in [
            vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ],
            vec![
                0x3e, 0x13, 0x01, 0x00,
                0x01, 0x08,
                0x00,
                0x01,
                7, 8, 9, 10, 11, 12,
                0x20, 0x00,
                0x00, 0x00,
                0x30, 0x00,
                0x00,
            ],
        ] {
            engine.ingest(ControllerPacket::Event(event)).unwrap();
            engine.poll_until_quiescent().unwrap();
        }

        let snapshot = engine.connection_set_snapshot();
        assert_eq!(snapshot.connections.len(), 2);
        assert_eq!(snapshot.connections[0].connection_handle, 0x0800);
        assert_eq!(snapshot.connections[1].connection_handle, 0x0801);
        assert_eq!(snapshot.connections[0].connection_interval, 24);
        assert_eq!(snapshot.connections[1].connection_interval, 32);
        assert_eq!(snapshot.connections[0].encryption_enabled, 0);
        assert_eq!(snapshot.connections[1].encryption_enabled, 0);

        engine
            .ingest(ControllerPacket::Event(vec![
                0x08, 0x04, 0x00, 0x01, 0x08, 0x01,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        let snapshot = engine.connection_set_snapshot();
        assert_eq!(snapshot.connections[0].encryption_enabled, 0);
        assert_eq!(snapshot.connections[1].encryption_enabled, 1);

        engine
            .ingest(ControllerPacket::Event(vec![
                0x05, 0x04, 0x00, 0x00, 0x08, 0x08,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        let snapshot = engine.connection_set_snapshot();
        assert_eq!(snapshot.connections.len(), 1);
        assert_eq!(snapshot.connections[0].connection_handle, 0x0801);
        assert_eq!(snapshot.connections[0].encryption_enabled, 1);
    }

    #[test]
    fn engine_stored_ltk_encryption_tracks_bumble_encrypted_state() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine
            .ingest(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        engine
            .begin_stored_encryption(
                super::super::security::StoredEncryptionRequest {
                    connection: ConnectionToken { connection_handle: 0x0800, generation: 1 },
                    key: super::super::security::StoredLeKeyMaterial::
                        secure_connections([0x5a; 16]),
                },
            )
            .unwrap();

        let HostPacket::Command(bytes) = rx.try_recv().unwrap() else {
            panic!("expected LE Enable Encryption command");
        };
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x2019);
        assert_eq!(bytes[2], 0x1c);
        assert_eq!(&bytes[3..5], &[0x00, 0x08]);
        assert_eq!(&bytes[5..13], &[0; 8]);
        assert_eq!(&bytes[13..15], &[0; 2]);
        assert_eq!(&bytes[15..31], &[0x5a; 16]);
        assert_eq!(
            engine.stored_encryption_state(),
            &StoredEncryptionState::Submitting
        );

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0f, 0x04, 0x00, 0x01, 0x19, 0x20,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.stored_encryption_state(),
            &StoredEncryptionState::AwaitingChange
        );

        engine
            .ingest(ControllerPacket::Event(vec![
                0x08, 0x04, 0x00, 0x00, 0x08, 0x01,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert!(engine.device().is_encrypted_on_handle(0x0800));
        assert_eq!(
            engine.stored_encryption_state(),
            &StoredEncryptionState::Encrypted {
                connection: ConnectionToken { connection_handle: 0x0800, generation: 1 },
                encryption_key_size: 0,
            }
        );

        // Idempotent repeat uses Bumble's existing encrypted state and emits
        // no second LE Enable Encryption command.
        engine
            .begin_stored_encryption(
                super::super::security::StoredEncryptionRequest {
                    connection: ConnectionToken { connection_handle: 0x0800, generation: 1 },
                    key: super::super::security::StoredLeKeyMaterial::
                        secure_connections([0x5a; 16]),
                },
            )
            .unwrap();
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
    }

    #[test]
    fn engine_disconnect_uses_0x13_and_accepts_0x16_event_reason() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        // Establish an unsolicited synthetic LE ACL so Bumble owns the
        // connection-handle state before the disconnect request.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert!(engine.device().is_connected_on_handle(0x0800));

        assert_eq!(
            engine.begin_disconnect(ConnectionToken {
                connection_handle: 0x0801,
                generation: 1,
            }),
            Err(EngineFault::StaleConnection {
                connection_handle: 0x0801,
                requested_generation: 1,
                current_generation: None,
            })
        );
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));

        engine.begin_disconnect(ConnectionToken { connection_handle: 0x0800, generation: 1 }).unwrap();
        let command = rx.try_recv().unwrap();
        let HostPacket::Command(bytes) = command else {
            panic!("expected HCI Disconnect command");
        };
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x0406);
        assert_eq!(bytes, vec![0x06, 0x04, 0x03, 0x00, 0x08, 0x13]);
        assert_eq!(
            engine.disconnect_state(),
            &DisconnectState::Submitting
        );

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0f, 0x04, 0x00, 0x01, 0x06, 0x04,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.disconnect_state(),
            &DisconnectState::AwaitingEvent
        );

        engine
            .ingest(ControllerPacket::Event(vec![
                0x05, 0x04, 0x00, 0x00, 0x08, 0x16,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            engine.disconnect_state(),
            &DisconnectState::Disconnected {
                connection: ConnectionToken { connection_handle: 0x0800, generation: 1 },
                event_reason: 0x16,
            }
        );
        assert!(!engine.device().is_connected_on_handle(0x0800));
    }

    #[test]
    fn acl_credit_audit_faults_on_over_completion_per_handle() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine
            .ingest(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert!(engine.link_mut().send_acl_packet(
            0,
            bumble_hci::AclDataPacket {
                connection_handle: 0x0800,
                pb_flag: 0,
                bc_flag: 0,
                data_total_length: 4,
                data: vec![0, 0, 4, 0],
            },
        ));
        engine.poll_until_quiescent().unwrap();
        assert_eq!(engine.acl_credit_audit.in_flight_on(0x0800), 1);

        engine
            .ingest(ControllerPacket::Event(vec![
                0x13, 0x05, 0x01,
                0x00, 0x08, 0x02, 0x00,
            ]))
            .unwrap();
        assert_eq!(
            engine.poll_until_quiescent(),
            Err(EngineFault::AclCredit(
                AclCreditFault::CompletionOverflow {
                    connection_handle: 0x0800,
                    completed: 2,
                    in_flight: 1,
                }
            ))
        );
    }

    #[test]
    fn reused_handle_rejects_previous_generation_token() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        let established = || {
            ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ])
        };

        engine.ingest(established()).unwrap();
        engine.poll_until_quiescent().unwrap();
        let first = engine.connection_set_snapshot().connections[0].token();
        assert_eq!(first.generation, 1);

        engine
            .ingest(ControllerPacket::Event(vec![
                0x05, 0x04, 0x00, 0x00, 0x08, 0x08,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert!(engine.connection_set_snapshot().connections.is_empty());

        engine.ingest(established()).unwrap();
        engine.poll_until_quiescent().unwrap();
        let second = engine.connection_set_snapshot().connections[0].token();
        assert_eq!(second.connection_handle, first.connection_handle);
        assert_eq!(second.generation, 2);

        assert_eq!(
            engine.begin_disconnect(first),
            Err(EngineFault::StaleConnection {
                connection_handle: 0x0800,
                requested_generation: 1,
                current_generation: Some(2),
            })
        );

        assert_eq!(
            engine.begin_stored_encryption(
                super::super::security::StoredEncryptionRequest {
                    connection: first,
                    key: super::super::security::StoredLeKeyMaterial::
                        secure_connections([0x44; 16]),
                },
            ),
            Err(EngineFault::StaleConnection {
                connection_handle: 0x0800,
                requested_generation: 1,
                current_generation: Some(2),
            })
        );
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
    }

    #[test]
    fn engine_cancel_waits_for_terminal_le_connection_event() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine
            .begin_fal_connection(
                super::super::connection::FalConnectRequest {
                    peer: super::super::connection::LePeerIdentity {
                        address_le: [
                            0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8,
                        ],
                        address_type: 0,
                    },
                    parameters:
                        super::super::connection::LeInitiationParameters::default(),
                },
            )
            .unwrap();
        let _ = rx.try_recv().unwrap();

        for (opcode, extra) in [
            (0x200f, &[8][..]),
            (0x2010, &[][..]),
            (0x2011, &[][..]),
        ] {
            engine.ingest(complete(opcode, 0, extra)).unwrap();
            engine.poll_until_quiescent().unwrap();
            let _ = rx.try_recv().unwrap();
        }

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0f, 0x04, 0x00, 0x01, 0x43, 0x20,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.fal_connection_state(),
            &FalConnectionState::AwaitingConnection
        );

        engine.cancel_fal_connection().unwrap();
        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            0x200e
        );
        assert_eq!(
            engine.fal_connection_state(),
            &FalConnectionState::Cancelling
        );

        engine.ingest(complete(0x200e, 0, &[])).unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.fal_connection_state(),
            &FalConnectionState::CancelAccepted
        );

        // Terminal LE Connection Complete failure after accepted cancel.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x02,
                0x00, 0x00,
                0x00,
                0x00,
                0, 0, 0, 0, 0, 0,
                0x00, 0x00,
                0x00, 0x00,
                0x00, 0x00,
                0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            command_opcode(rx.try_recv().unwrap()),
            0x2010
        );

        engine.ingest(complete(0x2010, 0, &[])).unwrap();
        engine.poll_until_quiescent().unwrap();

        match engine.fal_connection_state() {
            FalConnectionState::Failed(failure) => {
                assert_eq!(
                    failure.kind,
                    super::super::connection::FalConnectFailureKind::Cancelled {
                        connection_status: 0x02,
                    }
                );
            }
            other => panic!("expected cancelled terminal state, got {other:?}"),
        }
    }

    #[test]
    fn passive_scan_is_a_nonblocking_command_transaction() {
        let (mut engine, rx) = engine(16);
        make_minimal_attach_ready(&mut engine, &rx);

        engine.begin_passive_extended_scan().unwrap();
        let parameters = rx.try_recv().unwrap();
        assert!(matches!(parameters, HostPacket::Command(_)));
        assert_eq!(
            engine.passive_scan_state(),
            PassiveScanState::Configuring
        );

        // LE Set Extended Scan Parameters complete, status 0.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x41, 0x20, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        let enable = rx.try_recv().unwrap();
        assert!(matches!(enable, HostPacket::Command(_)));
        assert_eq!(
            engine.passive_scan_state(),
            PassiveScanState::Enabling
        );

        // LE Set Extended Scan Enable complete, status 0.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x42, 0x20, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.passive_scan_state(),
            PassiveScanState::Scanning
        );

        engine.stop_passive_extended_scan().unwrap();
        let disable = rx.try_recv().unwrap();
        assert!(matches!(disable, HostPacket::Command(_)));
        assert_eq!(
            engine.passive_scan_state(),
            PassiveScanState::Disabling
        );

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x42, 0x20, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.passive_scan_state(),
            PassiveScanState::Idle
        );

        let summary = engine.take_completed_scan().unwrap();
        assert_eq!(summary.reports_seen, 0);
        assert!(summary.observations.is_empty());
        assert!(!summary.truncated);
    }

    #[test]
    fn scan_summary_collects_extended_advertising_data_from_bumble() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine.begin_passive_extended_scan().unwrap();
        let _ = rx.try_recv().unwrap();

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x41, 0x20, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        let _ = rx.try_recv().unwrap();

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x42, 0x20, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(
            engine.passive_scan_state(),
            PassiveScanState::Scanning
        );

        let advertising_data = vec![
            0x06, 0x09, b'B', b'u', b'd', b's', b'3',
            0x07, 0x2e, 1, 2, 3, 4, 5, 6,
        ];

        let mut report = vec![
            0x3e, // LE Meta Event
            0x00, // parameter length, filled below
            0x0d, // LE Extended Advertising Report
            0x01, // num_reports
            0x00, 0x00, // event_type: complete extended advertisement
            0x00, // public device address
            0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
            0x01, // primary PHY: LE 1M
            0x00, // secondary PHY: none
            0x00, // SID
            0x7f, // TX power unavailable
            0xd8, // RSSI = -40 dBm
            0x00, 0x00, // periodic interval
            0xff, // direct address type: unavailable
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            advertising_data.len() as u8,
        ];
        report.extend_from_slice(&advertising_data);
        report[1] = (report.len() - 2) as u8;

        engine
            .ingest(ControllerPacket::Event(report))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        engine.stop_passive_extended_scan().unwrap();
        let _ = rx.try_recv().unwrap();
        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x42, 0x20, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        let summary = engine.take_completed_scan().unwrap();
        assert_eq!(summary.reports_seen, 1);
        assert_eq!(summary.observations.len(), 1);

        let observation = &summary.observations[0];
        assert_eq!(observation.address, "11:22:33:44:55:66/P");
        assert_eq!(observation.address_type, 0);
        assert_eq!(observation.name.as_deref(), Some("Buds3"));
        assert_eq!(observation.rsi, Some([1, 2, 3, 4, 5, 6]));
        assert_eq!(observation.rssi, -40);
        assert_eq!(observation.raw_data, advertising_data);
    }

    #[test]
    fn scan_command_failure_is_terminal_and_explicit() {
        let (mut engine, rx) = engine(16);
        make_minimal_attach_ready(&mut engine, &rx);

        engine.begin_passive_extended_scan().unwrap();
        let _ = rx.try_recv().unwrap();

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x41, 0x20, 0x0c,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(
            engine.passive_scan_state(),
            PassiveScanState::Failed {
                opcode: 0x2041,
                status: 0x0c,
            }
        );
    }

    #[test]
    fn supported_commands_failure_terminates_operational_attach() {
        let (mut engine, _rx) = engine(8);
        engine.begin_operational_attach().unwrap();

        // Command Complete: Read Local Supported Commands, status=0x0C.
        let mut event = vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x0c];
        event.resize(2 + 0x44, 0);
        engine.ingest(ControllerPacket::Event(event)).unwrap();

        assert_eq!(engine.poll_until_quiescent(), Ok(1));
        assert_eq!(
            engine.attach_state(),
            OperationalAttachState::Failed(
                OperationalAttachFailure::SupportedCommandsStatus(0x0c)
            )
        );
    }

    #[test]
    fn device_reset_commands_flow_through_our_transport_adapter() {
        let (mut engine, rx) = engine(8);

        // Test-only direct Device access is intentional here: production
        // Control will later call typed HciContext commands instead.
        let HciEngine { device, link, .. } = &mut engine;
        device.reset(link);

        assert_eq!(
            rx.try_recv().unwrap(),
            HostPacket::Command(vec![0x03, 0x0c, 0x00])
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            HostPacket::Command(vec![0x02, 0x10, 0x00])
        );
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
        assert!(!engine.device().controller_ready());
    }

    #[test]
    fn reset_complete_event_is_consumed_by_bumble_device_poll() {
        let (mut engine, _rx) = engine(8);

        let HciEngine { device, link, .. } = &mut engine;
        device.reset(link);
        assert!(!engine.device().controller_ready());

        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00,
            ]))
            .unwrap();

        assert_eq!(engine.poll_until_quiescent(), Ok(1));
        assert!(engine.device().controller_ready());
    }

    #[test]
    fn malformed_controller_packet_fails_before_device_poll() {
        let (mut engine, _rx) = engine(8);

        assert_eq!(
            engine.ingest(ControllerPacket::Event(vec![0x0e, 0x04])),
            Err(EngineFault::Link(LinkFault::InvalidControllerPacket))
        );
    }

    #[test]
    fn transport_backpressure_fault_surfaces_from_device_command_path() {
        let (mut engine, _rx) = engine(1);

        let HciEngine { device, link, .. } = &mut engine;
        device.reset(link);

        assert_eq!(
            engine.poll_until_quiescent(),
            Err(EngineFault::Link(LinkFault::TransportFull))
        );
    }

    fn establish_encrypted_test_acl(
        engine: &mut HciEngine,
        connection_handle: u16,
        address_seed: u8,
    ) -> ConnectionToken {
        engine
            .ingest(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                connection_handle as u8,
                (connection_handle >> 8) as u8,
                0x00,
                0x01,
                address_seed,
                address_seed.wrapping_add(1),
                address_seed.wrapping_add(2),
                address_seed.wrapping_add(3),
                address_seed.wrapping_add(4),
                address_seed.wrapping_add(5),
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        engine
            .ingest(ControllerPacket::Event(vec![
                0x08, 0x04, 0x00,
                connection_handle as u8,
                (connection_handle >> 8) as u8,
                0x01,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        engine
            .connection_set_snapshot()
            .connections
            .into_iter()
            .find(|connection| {
                connection.connection_handle == connection_handle
            })
            .expect("synthetic connection must exist")
            .token()
    }

    fn inbound_att(
        connection_handle: u16,
        pdu: bumble_att::AttPdu,
    ) -> ControllerPacket {
        let att = pdu.to_bytes();
        let mut l2cap = Vec::with_capacity(4 + att.len());
        l2cap.extend_from_slice(&(att.len() as u16).to_le_bytes());
        l2cap.extend_from_slice(&bumble_host::ATT_CID.to_le_bytes());
        l2cap.extend_from_slice(&att);

        let h4 = bumble_hci::AclDataPacket {
            connection_handle,
            pb_flag: 0b10,
            bc_flag: 0,
            data_total_length: l2cap.len() as u16,
            data: l2cap,
        }
        .to_bytes();
        assert_eq!(h4[0], bumble_hci::codes::HCI_ACL_DATA_PACKET);
        ControllerPacket::Acl(h4[1..].to_vec())
    }

    fn inbound_att_fragments(
        connection_handle: u16,
        pdu: bumble_att::AttPdu,
        max_fragment_size: usize,
    ) -> Vec<ControllerPacket> {
        let att = pdu.to_bytes();
        let mut l2cap = Vec::with_capacity(4 + att.len());
        l2cap.extend_from_slice(&(att.len() as u16).to_le_bytes());
        l2cap.extend_from_slice(&bumble_host::ATT_CID.to_le_bytes());
        l2cap.extend_from_slice(&att);

        bumble_hci::fragment_l2cap_pdu(
            connection_handle,
            0,
            max_fragment_size,
            &l2cap,
            true,
        )
        .unwrap()
        .into_iter()
        .map(|packet| {
            let h4 = packet.to_bytes();
            ControllerPacket::Acl(h4[1..].to_vec())
        })
        .collect()
    }

    fn expect_outbound_att(
        packet: HostPacket,
        expected_handle: u16,
        expected_opcode: u8,
    ) {
        let HostPacket::Acl(bytes) = packet else {
            panic!("expected outbound ACL packet");
        };
        assert!(bytes.len() >= 9);
        let header = u16::from_le_bytes([bytes[0], bytes[1]]);
        assert_eq!(header & 0x0fff, expected_handle);
        let acl_length = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
        assert_eq!(acl_length, bytes.len() - 4);
        let l2cap_length =
            u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        assert_eq!(l2cap_length, bytes.len() - 8);
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), bumble_host::ATT_CID);
        assert_eq!(bytes[8], expected_opcode);
    }

    #[test]
    fn gatt_requires_encrypted_current_connection_before_acl_io() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);

        engine
            .ingest(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        let token = engine.connection_set_snapshot().connections[0].token();

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 100,
                connection: token,
                operation:
                    super::super::gatt::GattOperation::DiscoverServices,
            })
            .unwrap();

        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Failed(
                super::super::gatt::GattFailure {
                    request_id: 100,
                    kind: super::super::gatt::GattFailureKind::NotEncrypted,
                    ..
                }
            )]
        ));
    }

    #[test]
    fn gatt_mtu_round_trip_uses_real_acl_l2cap_att_path() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);
        let token = establish_encrypted_test_acl(&mut engine, 0x0800, 1);

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 101,
                connection: token,
                operation:
                    super::super::gatt::GattOperation::ExchangeMtu {
                        client_rx_mtu: 247,
                    },
            })
            .unwrap();

        expect_outbound_att(rx.try_recv().unwrap(), 0x0800, 0x02);
        assert_eq!(engine.gatt_in_flight_count(), 1);

        engine
            .ingest(inbound_att(
                0x0800,
                bumble_att::AttPdu::ExchangeMtuResponse {
                    server_rx_mtu: 185,
                },
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(engine.gatt_in_flight_count(), 0);
        assert_eq!(engine.gatt_mtu(token), Some(185));
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Complete {
                request_id: 101,
                connection,
                result: super::super::gatt::GattResult::Mtu { mtu: 185 },
            }] if *connection == token
        ));
    }

    #[test]
    fn gatt_response_reassembles_across_hci_acl_fragments() {
        let (mut engine, rx) = engine(32);
        make_minimal_attach_ready(&mut engine, &rx);
        let token = establish_encrypted_test_acl(&mut engine, 0x0800, 1);

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 150,
                connection: token,
                operation:
                    super::super::gatt::GattOperation::ExchangeMtu {
                        client_rx_mtu: 247,
                    },
            })
            .unwrap();
        expect_outbound_att(rx.try_recv().unwrap(), 0x0800, 0x02);

        let fragments = inbound_att_fragments(
            0x0800,
            bumble_att::AttPdu::ExchangeMtuResponse {
                server_rx_mtu: 185,
            },
            4,
        );
        assert!(fragments.len() > 1);

        engine.ingest(fragments[0].clone()).unwrap();
        engine.poll_until_quiescent().unwrap();
        assert_eq!(engine.gatt_in_flight_count(), 1);
        assert!(engine.take_gatt_reports().is_empty());

        for fragment in fragments.into_iter().skip(1) {
            engine.ingest(fragment).unwrap();
            engine.poll_until_quiescent().unwrap();
        }

        assert_eq!(engine.gatt_mtu(token), Some(185));
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Complete {
                request_id: 150,
                result: super::super::gatt::GattResult::Mtu { mtu: 185 },
                ..
            }]
        ));
    }

    #[test]
    fn gatt_fixed_bearer_transactions_are_isolated_per_acl_handle() {
        let (mut engine, rx) = engine(64);
        make_minimal_attach_ready(&mut engine, &rx);
        let first =
            establish_encrypted_test_acl(&mut engine, 0x0800, 1);
        let second =
            establish_encrypted_test_acl(&mut engine, 0x0801, 7);

        for (id, token, attribute_handle) in [
            (201, first, 0x0040),
            (202, second, 0x0050),
        ] {
            engine
                .begin_gatt_request(super::super::gatt::GattRequest {
                    request_id: id,
                    connection: token,
                    operation:
                        super::super::gatt::GattOperation::ReadValue {
                            attribute_handle,
                        },
                })
                .unwrap();
        }

        expect_outbound_att(rx.try_recv().unwrap(), 0x0800, 0x0a);
        expect_outbound_att(rx.try_recv().unwrap(), 0x0801, 0x0a);
        assert_eq!(engine.gatt_in_flight_count(), 2);

        engine
            .ingest(inbound_att(
                0x0801,
                bumble_att::AttPdu::ReadResponse {
                    attribute_value: vec![0xB2],
                },
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        let reports = engine.take_gatt_reports();
        assert!(matches!(
            reports.as_slice(),
            [super::super::gatt::GattReport::Complete {
                request_id: 202,
                connection,
                result: super::super::gatt::GattResult::Value {
                    attribute_handle: 0x0050,
                    value,
                },
            }] if *connection == second && value == &vec![0xB2]
        ));
        assert_eq!(engine.gatt_in_flight_count(), 1);

        engine
            .ingest(inbound_att(
                0x0800,
                bumble_att::AttPdu::ReadResponse {
                    attribute_value: vec![0xA1],
                },
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Complete {
                request_id: 201,
                connection,
                result: super::super::gatt::GattResult::Value {
                    attribute_handle: 0x0040,
                    value,
                },
            }] if *connection == first && value == &vec![0xA1]
        ));
    }

    #[test]
    fn unsolicited_notification_and_indication_are_connection_local() {
        let (mut engine, rx) = engine(64);
        make_minimal_attach_ready(&mut engine, &rx);
        let first =
            establish_encrypted_test_acl(&mut engine, 0x0800, 1);
        let second =
            establish_encrypted_test_acl(&mut engine, 0x0801, 7);

        engine
            .ingest(inbound_att(
                0x0800,
                bumble_att::AttPdu::HandleValueNotification {
                    attribute_handle: 0x0046,
                    attribute_value: vec![1, 2],
                },
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Value(
                super::super::gatt::GattValueEvent::Notification {
                    connection,
                    attribute_handle: 0x0046,
                    value,
                }
            )] if *connection == first && value == &vec![1, 2]
        ));

        engine
            .ingest(inbound_att(
                0x0801,
                bumble_att::AttPdu::HandleValueIndication {
                    attribute_handle: 0x0056,
                    attribute_value: vec![3, 4],
                },
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        expect_outbound_att(rx.try_recv().unwrap(), 0x0801, 0x1e);
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Value(
                super::super::gatt::GattValueEvent::Indication {
                    connection,
                    attribute_handle: 0x0056,
                    value,
                }
            )] if *connection == second && value == &vec![3, 4]
        ));
    }

    #[test]
    fn gatt_cancel_poison_drops_late_response_until_disconnect() {
        let (mut engine, rx) = engine(64);
        make_minimal_attach_ready(&mut engine, &rx);
        let first =
            establish_encrypted_test_acl(&mut engine, 0x0800, 1);

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 301,
                connection: first,
                operation:
                    super::super::gatt::GattOperation::ReadValue {
                        attribute_handle: 0x0040,
                    },
            })
            .unwrap();
        expect_outbound_att(rx.try_recv().unwrap(), 0x0800, 0x0a);

        engine.cancel_gatt_request(first, 301).unwrap();
        let cancel_reports = engine.take_gatt_reports();
        assert!(matches!(
            cancel_reports.as_slice(),
            [
                super::super::gatt::GattReport::Failed(
                    super::super::gatt::GattFailure {
                        request_id: 301,
                        kind:
                            super::super::gatt::GattFailureKind::Cancelled,
                        ..
                    }
                ),
                super::super::gatt::GattReport::BearerPoisoned {
                    request_id: 301,
                    ..
                }
            ]
        ));

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 302,
                connection: first,
                operation:
                    super::super::gatt::GattOperation::ReadValue {
                        attribute_handle: 0x0041,
                    },
            })
            .unwrap();
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Failed(
                super::super::gatt::GattFailure {
                    request_id: 302,
                    kind:
                        super::super::gatt::GattFailureKind::BearerPoisoned {
                            previous_request_id: 301,
                        },
                    ..
                }
            )]
        ));
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));

        engine
            .ingest(inbound_att(
                0x0800,
                bumble_att::AttPdu::ReadResponse {
                    attribute_value: vec![0xFF],
                },
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::LateResponseDropped {
                request_id: 301,
                ..
            }]
        ));

        engine
            .ingest(ControllerPacket::Event(vec![
                0x05, 0x04, 0x00, 0x00, 0x08, 0x16,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        let second =
            establish_encrypted_test_acl(&mut engine, 0x0800, 9);
        assert_eq!(second.connection_handle, first.connection_handle);
        assert_ne!(second.generation, first.generation);

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 303,
                connection: second,
                operation:
                    super::super::gatt::GattOperation::ReadValue {
                        attribute_handle: 0x0042,
                    },
            })
            .unwrap();
        expect_outbound_att(rx.try_recv().unwrap(), 0x0800, 0x0a);
    }

    #[test]
    fn disconnect_fails_only_matching_gatt_transaction() {
        let (mut engine, rx) = engine(64);
        make_minimal_attach_ready(&mut engine, &rx);
        let first =
            establish_encrypted_test_acl(&mut engine, 0x0800, 1);
        let second =
            establish_encrypted_test_acl(&mut engine, 0x0801, 7);

        for (id, token) in [(401, first), (402, second)] {
            engine
                .begin_gatt_request(super::super::gatt::GattRequest {
                    request_id: id,
                    connection: token,
                    operation:
                        super::super::gatt::GattOperation::ReadValue {
                            attribute_handle: 0x0040,
                        },
                })
                .unwrap();
            let _ = rx.try_recv().unwrap();
        }

        engine
            .ingest(ControllerPacket::Event(vec![
                0x05, 0x04, 0x00, 0x00, 0x08, 0x08,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Failed(
                super::super::gatt::GattFailure {
                    request_id: 401,
                    connection,
                    kind:
                        super::super::gatt::GattFailureKind::Disconnected {
                            reason: 0x08,
                        },
                }
            )] if *connection == first
        ));
        assert_eq!(engine.gatt_in_flight_count(), 1);

        engine
            .ingest(inbound_att(
                0x0801,
                bumble_att::AttPdu::ReadResponse {
                    attribute_value: vec![0x55],
                },
            ))
            .unwrap();
        engine.poll_until_quiescent().unwrap();
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Complete {
                request_id: 402,
                connection,
                ..
            }] if *connection == second
        ));
    }


    #[test]
    fn fragmented_acl_reassembles_before_gatt_response_dispatch() {
        let (mut engine, rx) = engine(64);
        make_minimal_attach_ready(&mut engine, &rx);
        let token =
            establish_encrypted_test_acl(&mut engine, 0x0800, 1);

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 501,
                connection: token,
                operation:
                    super::super::gatt::GattOperation::ReadValue {
                        attribute_handle: 0x0042,
                    },
            })
            .unwrap();
        expect_outbound_att(rx.try_recv().unwrap(), 0x0800, 0x0a);

        let att = bumble_att::AttPdu::ReadResponse {
            attribute_value: vec![1, 2, 3, 4, 5, 6],
        }
        .to_bytes();
        let mut l2cap = Vec::with_capacity(4 + att.len());
        l2cap.extend_from_slice(&(att.len() as u16).to_le_bytes());
        l2cap.extend_from_slice(&bumble_host::ATT_CID.to_le_bytes());
        l2cap.extend_from_slice(&att);

        let split = 5usize;
        for (pb_flag, fragment) in [
            (bumble_hci::HCI_ACL_PB_FIRST_FLUSHABLE, &l2cap[..split]),
            (bumble_hci::HCI_ACL_PB_CONTINUATION, &l2cap[split..]),
        ] {
            let h4 = bumble_hci::AclDataPacket {
                connection_handle: 0x0800,
                pb_flag,
                bc_flag: 0,
                data_total_length: fragment.len() as u16,
                data: fragment.to_vec(),
            }
            .to_bytes();
            engine
                .ingest(ControllerPacket::Acl(h4[1..].to_vec()))
                .unwrap();
            engine.poll_until_quiescent().unwrap();

            if pb_flag == bumble_hci::HCI_ACL_PB_FIRST_FLUSHABLE {
                assert!(engine.take_gatt_reports().is_empty());
                assert_eq!(engine.gatt_in_flight_count(), 1);
            }
        }

        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Complete {
                request_id: 501,
                result: super::super::gatt::GattResult::Value {
                    attribute_handle: 0x0042,
                    value,
                },
                ..
            }] if value == &vec![1, 2, 3, 4, 5, 6]
        ));
    }

    #[test]
    fn gatt_rejects_busy_bearer_and_duplicate_request_id_without_acl_io() {
        let (mut engine, rx) = engine(64);
        make_minimal_attach_ready(&mut engine, &rx);
        let first =
            establish_encrypted_test_acl(&mut engine, 0x0800, 1);
        let second =
            establish_encrypted_test_acl(&mut engine, 0x0801, 7);

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 601,
                connection: first,
                operation:
                    super::super::gatt::GattOperation::ReadValue {
                        attribute_handle: 0x0040,
                    },
            })
            .unwrap();
        expect_outbound_att(rx.try_recv().unwrap(), 0x0800, 0x0a);

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 602,
                connection: first,
                operation:
                    super::super::gatt::GattOperation::ReadValue {
                        attribute_handle: 0x0041,
                    },
            })
            .unwrap();
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Failed(
                super::super::gatt::GattFailure {
                    request_id: 602,
                    kind: super::super::gatt::GattFailureKind::Busy,
                    ..
                }
            )]
        ));

        engine
            .begin_gatt_request(super::super::gatt::GattRequest {
                request_id: 601,
                connection: second,
                operation:
                    super::super::gatt::GattOperation::ReadValue {
                        attribute_handle: 0x0050,
                    },
            })
            .unwrap();
        assert!(matches!(
            engine.take_gatt_reports().as_slice(),
            [super::super::gatt::GattReport::Failed(
                super::super::gatt::GattFailure {
                    request_id: 601,
                    kind:
                        super::super::gatt::GattFailureKind::DuplicateRequestId,
                    ..
                }
            )]
        ));
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
        assert_eq!(engine.gatt_in_flight_count(), 1);
    }

}
