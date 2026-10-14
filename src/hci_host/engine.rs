use bumble_hci::Command;
use bumble_host::{Device, HostTransport};

use crate::transport::{
    packet::ControllerPacket,
    worker::TransportTx,
};

use super::link::{LinkFault, QueueHostTransport};

const DEFAULT_POLL_BUDGET: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineFault {
    Link(LinkFault),
    PollBudgetExceeded,
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

pub struct HciEngine {
    device: Device,
    link: QueueHostTransport,
    attach_state: OperationalAttachState,
}

impl HciEngine {
    pub fn new(tx: TransportTx) -> Self {
        Self {
            device: Device::new(0),
            link: QueueHostTransport::new(tx),
            attach_state: OperationalAttachState::Idle,
        }
    }

    pub fn ingest(&mut self, packet: ControllerPacket) -> Result<(), EngineFault> {
        self.link.ingest(packet).map_err(EngineFault::Link)
    }

    pub fn attach_state(&self) -> OperationalAttachState {
        self.attach_state
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
                if let Some(fault) = self.link.take_fault() {
                    return Err(EngineFault::Link(fault));
                }
                self.refresh_attach_state();
                return Ok(turns);
            }

            turns += 1;
        }

        Err(EngineFault::PollBudgetExceeded)
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
