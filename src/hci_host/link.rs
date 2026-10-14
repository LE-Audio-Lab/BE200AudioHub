use std::collections::VecDeque;

use bumble_hci::{
    codes::{HCI_ACL_DATA_PACKET, HCI_COMMAND_PACKET, HCI_EVENT_PACKET, HCI_ISO_DATA_PACKET},
    AclDataPacket, Command, HciPacket, IsoDataPacket,
};
use bumble_host::HostTransport;

use crate::{
    runtime::mailbox::SendError,
    transport::{
        packet::{ControllerPacket, HostPacket},
        worker::TransportTx,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkFault {
    TransportFull,
    TransportDisconnected,
    InvalidControllerPacket,
}

pub struct QueueHostTransport {
    tx: TransportTx,
    inbound: VecDeque<HciPacket>,
    fault: Option<LinkFault>,
}

impl QueueHostTransport {
    pub fn new(tx: TransportTx) -> Self {
        Self {
            tx,
            inbound: VecDeque::new(),
            fault: None,
        }
    }

    pub fn ingest(&mut self, packet: ControllerPacket) -> Result<(), LinkFault> {
        let (packet_type, bytes) = match packet {
            ControllerPacket::Event(bytes) => (HCI_EVENT_PACKET, bytes),
            ControllerPacket::Acl(bytes) => (HCI_ACL_DATA_PACKET, bytes),
            ControllerPacket::Iso(bytes) => (HCI_ISO_DATA_PACKET, bytes),
        };

        let mut h4 = Vec::with_capacity(bytes.len() + 1);
        h4.push(packet_type);
        h4.extend_from_slice(&bytes);

        let parsed = HciPacket::from_bytes(&h4)
            .map_err(|_| LinkFault::InvalidControllerPacket)?;
        self.inbound.push_back(parsed);
        Ok(())
    }

    pub fn take_fault(&mut self) -> Option<LinkFault> {
        self.fault.take()
    }

    fn send_native(&mut self, packet_type: u8, bytes: Vec<u8>) -> bool {
        if bytes.first().copied() != Some(packet_type) {
            self.fault = Some(LinkFault::InvalidControllerPacket);
            return false;
        }

        let native = bytes[1..].to_vec();
        let packet = match packet_type {
            HCI_COMMAND_PACKET => HostPacket::Command(native),
            HCI_ACL_DATA_PACKET => HostPacket::Acl(native),
            HCI_ISO_DATA_PACKET => HostPacket::Iso(native),
            _ => {
                self.fault = Some(LinkFault::InvalidControllerPacket);
                return false;
            }
        };

        match self.tx.try_send(packet) {
            Ok(()) => true,
            Err(SendError::Full) => {
                self.fault = Some(LinkFault::TransportFull);
                false
            }
            Err(SendError::Disconnected) => {
                self.fault = Some(LinkFault::TransportDisconnected);
                false
            }
        }
    }
}

impl HostTransport for QueueHostTransport {
    fn handle_command(&mut self, _controller_id: usize, command: Command) {
        let _ = self.send_native(HCI_COMMAND_PACKET, command.to_bytes());
    }

    fn send_acl_packet(
        &mut self,
        _controller_id: usize,
        packet: AclDataPacket,
    ) -> bool {
        self.send_native(HCI_ACL_DATA_PACKET, packet.to_bytes())
    }

    fn send_synchronous_data(
        &mut self,
        _controller_id: usize,
        _connection_handle: u16,
        _packet_status: u8,
        _data: &[u8],
    ) -> bool {
        false
    }

    fn send_iso_packet(
        &mut self,
        _controller_id: usize,
        packet: IsoDataPacket,
    ) -> bool {
        self.send_native(HCI_ISO_DATA_PACKET, packet.to_bytes())
    }

    fn drain_host_events(&mut self, _controller_id: usize) -> Vec<HciPacket> {
        self.inbound.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        runtime::mailbox,
        transport::worker::TransportTx,
    };

    fn tx(capacity: usize) -> (
        TransportTx,
        crate::runtime::mailbox::BoundedReceiver<HostPacket>,
    ) {
        TransportTx::test_channel(capacity)
    }

    #[test]
    fn native_event_is_wrapped_as_h4_before_bumble_parse() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        // Command Complete, opcode 0x0C03 (Reset), status 0.
        link.ingest(ControllerPacket::Event(vec![
            0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00,
        ]))
        .unwrap();

        let events = link.drain_host_events(0);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], HciPacket::Event(_)));
    }

    #[test]
    fn outbound_command_strips_h4_before_transport() {
        let (tx, rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        link.handle_command(0, Command::Reset);

        assert_eq!(
            rx.try_recv().unwrap(),
            HostPacket::Command(vec![0x03, 0x0c, 0x00])
        );
        assert_eq!(link.take_fault(), None);
    }

    #[test]
    fn malformed_native_packet_is_rejected() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        assert_eq!(
            link.ingest(ControllerPacket::Event(vec![0x0e, 0x04, 0x01])),
            Err(LinkFault::InvalidControllerPacket)
        );
    }

    #[test]
    fn transport_backpressure_is_recorded_not_silently_dropped() {
        let (tx, _rx) = tx(1);
        let mut link = QueueHostTransport::new(tx);

        link.handle_command(0, Command::Reset);
        link.handle_command(0, Command::Reset);

        assert_eq!(link.take_fault(), Some(LinkFault::TransportFull));
    }
}
