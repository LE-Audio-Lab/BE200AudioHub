use std::collections::VecDeque;

use bumble_hci::{
    codes::{
        HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        HCI_LE_CREATE_CONNECTION_CANCEL_COMMAND,
        HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
    },
    Command,
};
use bumble_host::{
    Device, DeviceConnectionTransport, DeviceEvent, HostTransport,
};

use crate::transport::{
    packet::ControllerPacket,
    worker::TransportTx,
};

use super::{
    connection::{
        ConnectionEvent, FalConnectAction, FalConnectConfigError,
        FalConnectRequest, FalConnectTransaction, FalConnectionState,
        LeConnectionSnapshot,
    },
    link::{HciObservation, LinkFault, QueueHostTransport},
    privacy::{
        ControllerPrivacyState, PrivacyAction, PrivacyConfigError,
        PrivacyTransaction, ResolvingListConfig,
    },
    scan::{ScanAccumulator, ScanSummary},
};

const DEFAULT_POLL_BUDGET: usize = 64;
const MAX_DEFERRED_DEVICE_EVENTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineFault {
    Link(LinkFault),
    PollBudgetExceeded,
    InvalidState,
    PrivacyConfig(PrivacyConfigError),
    FalConnectConfig(FalConnectConfigError),
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

pub struct HciEngine {
    device: Device,
    link: QueueHostTransport,
    attach_state: OperationalAttachState,
    scan_state: PassiveScanState,
    scan_pending_opcode: Option<u16>,
    scan_accumulator: ScanAccumulator,
    completed_scan: Option<ScanSummary>,
    privacy_state: ControllerPrivacyState,
    privacy_transaction: Option<PrivacyTransaction>,
    fal_connection_state: FalConnectionState,
    fal_connection_transaction: Option<FalConnectTransaction>,
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
            privacy_state: ControllerPrivacyState::Idle,
            privacy_transaction: None,
            fal_connection_state: FalConnectionState::Idle,
            fal_connection_transaction: None,
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

    pub fn controller_privacy_state(&self) -> &ControllerPrivacyState {
        &self.privacy_state
    }

    pub fn fal_connection_state(&self) -> &FalConnectionState {
        &self.fal_connection_state
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
                self.process_device_events()?;
                self.check_link_fault()?;
                self.refresh_attach_state();
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
            self.process_privacy_observation(&observation)?;
            self.process_fal_connection_observation(&observation)?;
            self.process_scan_observation(&observation)?;
        }
        Ok(())
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

    fn process_device_events(&mut self) -> Result<(), EngineFault> {
        for event in self.device.take_device_events() {
            if self.process_fal_connection_device_event(&event)? {
                continue;
            }

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
        Ok(())
    }

    fn process_fal_connection_device_event(
        &mut self,
        event: &DeviceEvent,
    ) -> Result<bool, EngineFault> {
        let connection_event = match event {
            DeviceEvent::LeConnectionEstablished(info) => {
                Some(ConnectionEvent::Established(
                    LeConnectionSnapshot {
                        connection_handle: info.connection_handle,
                        role: info.role,
                        peer_address_le: *info.peer_address.address_bytes(),
                        peer_address_type:
                            info.peer_address.address_type().0,
                        connection_interval:
                            info.parameters.connection_interval,
                        peripheral_latency:
                            info.parameters.peripheral_latency,
                        supervision_timeout:
                            info.parameters.supervision_timeout,
                        encryption_enabled: info.encryption_enabled,
                    },
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

    fn refresh_attach_state(&mut self) {
        if self.attach_state != OperationalAttachState::Discovering {
            return;
        }

        if let Some(status) = self.device.local_supported_commands_status() {
            if status != 0 {
                self.attach_state = OperationalAttachState::Failed(
                    OperationalAttachFailure::SupportedCommandsStatus(status),
                );
                return;
            }
        }

        if self.device.host_initialization_complete() {
            self.attach_state = if self.device.host_initialization_succeeded() {
                OperationalAttachState::Ready
            } else {
                OperationalAttachState::Failed(
                    OperationalAttachFailure::HostInitializationFailed,
                )
            };
        }
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
}
