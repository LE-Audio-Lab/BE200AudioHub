use bumble_hci::Command;
use bumble_host::{Device, HostTransport};

use crate::transport::{
    packet::ControllerPacket,
    worker::TransportTx,
};

use super::link::{HciObservation, LinkFault, QueueHostTransport};

const DEFAULT_POLL_BUDGET: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineFault {
    Link(LinkFault),
    PollBudgetExceeded,
    InvalidState,
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
}

impl HciEngine {
    pub fn new(tx: TransportTx) -> Self {
        Self {
            device: Device::new(0),
            link: QueueHostTransport::new(tx),
            attach_state: OperationalAttachState::Idle,
            scan_state: PassiveScanState::Idle,
            scan_pending_opcode: None,
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

    pub fn begin_passive_extended_scan(&mut self) -> Result<(), EngineFault> {
        if self.attach_state != OperationalAttachState::Ready
            || self.scan_state != PassiveScanState::Idle
        {
            return Err(EngineFault::InvalidState);
        }

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
                self.process_observations()?;
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

    fn process_observations(&mut self) -> Result<(), EngineFault> {
        for observation in self.link.take_observations() {
            let (opcode, status) = match observation {
                HciObservation::CommandComplete { opcode, status } => {
                    (opcode, status.unwrap_or(0xff))
                }
                HciObservation::CommandStatus { opcode, status } => {
                    (opcode, status)
                }
            };

            if self.scan_pending_opcode != Some(opcode) {
                continue;
            }

            if status != 0 {
                self.scan_pending_opcode = None;
                self.scan_state =
                    PassiveScanState::Failed { opcode, status };
                continue;
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
                    self.scan_state = PassiveScanState::Idle;
                }
                PassiveScanState::Idle
                | PassiveScanState::Scanning
                | PassiveScanState::Failed { .. } => {}
            }
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

        // Set Event Mask complete, status 0.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        // LE Set Event Mask complete, status 0.
        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();

        assert_eq!(engine.poll_until_quiescent(), Ok(1));
        assert_eq!(engine.poll_until_quiescent(), Ok(1));
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
        engine
            .ingest(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();
        engine.poll_until_quiescent().unwrap();

        assert_eq!(engine.attach_state(), OperationalAttachState::Ready);
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
        let HciEngine { device, link } = &mut engine;
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

        let HciEngine { device, link } = &mut engine;
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

        let HciEngine { device, link } = &mut engine;
        device.reset(link);

        assert_eq!(
            engine.poll_until_quiescent(),
            Err(EngineFault::Link(LinkFault::TransportFull))
        );
    }
}
