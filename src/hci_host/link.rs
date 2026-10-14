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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciObservation {
    CommandComplete {
        opcode: u16,
        status: Option<u8>,
    },
    CommandStatus {
        opcode: u16,
        status: u8,
    },
}

pub struct QueueHostTransport {
    tx: TransportTx,
    inbound: VecDeque<HciPacket>,
    observations: VecDeque<HciObservation>,
    fault: Option<LinkFault>,
}

impl QueueHostTransport {
    pub fn new(tx: TransportTx) -> Self {
        Self {
            tx,
            inbound: VecDeque::new(),
            observations: VecDeque::new(),
            fault: None,
        }
    }

    pub fn ingest(&mut self, packet: ControllerPacket) -> Result<(), LinkFault> {
        let (packet_type, bytes) = match packet {
            ControllerPacket::Event(bytes) => {
                if let Some(observation) = observe_native_event(&bytes) {
                    self.observations.push_back(observation);
                }
                (HCI_EVENT_PACKET, bytes)
            }
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

    pub fn take_observations(&mut self) -> Vec<HciObservation> {
        self.observations.drain(..).collect()
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

fn observe_native_event(bytes: &[u8]) -> Option<HciObservation> {
    match bytes.first().copied()? {
        // HCI Command Complete:
        // event, param_len, num_cmd_packets, opcode_le[2], return_parameters...
        0x0e if bytes.len() >= 5 => Some(HciObservation::CommandComplete {
            opcode: u16::from_le_bytes([bytes[3], bytes[4]]),
            status: bytes.get(5).copied(),
        }),

        // HCI Command Status:
        // event, param_len, status, num_cmd_packets, opcode_le[2]
        0x0f if bytes.len() >= 6 => Some(HciObservation::CommandStatus {
            opcode: u16::from_le_bytes([bytes[4], bytes[5]]),
            status: bytes[2],
        }),

        _ => None,
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
    use crate::transport::worker::TransportTx;

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
    fn command_complete_is_journaled_without_stealing_device_event() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        link.ingest(ControllerPacket::Event(vec![
            0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00,
        ]))
        .unwrap();

        assert_eq!(
            link.take_observations(),
            vec![HciObservation::CommandComplete {
                opcode: 0x0c03,
                status: Some(0),
            }]
        );
        assert_eq!(link.drain_host_events(0).len(), 1);
    }

    #[test]
    fn command_status_is_journaled() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        link.ingest(ControllerPacket::Event(vec![
            0x0f, 0x04, 0x0c, 0x01, 0x0d, 0x20,
        ]))
        .unwrap();

        assert_eq!(
            link.take_observations(),
            vec![HciObservation::CommandStatus {
                opcode: 0x200d,
                status: 0x0c,
            }]
        );
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
