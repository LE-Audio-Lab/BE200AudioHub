use bumble_host::Device;

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

pub struct HciEngine {
    device: Device,
    link: QueueHostTransport,
}

impl HciEngine {
    pub fn new(tx: TransportTx) -> Self {
        Self {
            device: Device::new(0),
            link: QueueHostTransport::new(tx),
        }
    }

    pub fn ingest(&mut self, packet: ControllerPacket) -> Result<(), EngineFault> {
        self.link.ingest(packet).map_err(EngineFault::Link)
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
                return Ok(turns);
            }

            turns += 1;
        }

        Err(EngineFault::PollBudgetExceeded)
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
