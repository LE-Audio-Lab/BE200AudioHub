use std::collections::{BTreeSet, VecDeque};

use bumble_hci::{
    codes::{
        HCI_ACL_DATA_PACKET, HCI_COMMAND_PACKET, HCI_EVENT_PACKET, HCI_ISO_DATA_PACKET,
        HCI_LE_SET_EVENT_MASK_COMMAND, HCI_LE_WRITE_SUGGESTED_DEFAULT_DATA_LENGTH_COMMAND,
        HCI_SET_EVENT_MASK_COMMAND, HCI_SET_EVENT_MASK_PAGE_2_COMMAND,
    },
    AclDataPacket, Command, Event, HciPacket, IsoDataPacket, ReturnParameters,
};
use bumble_host::HostTransport;

use crate::{
    runtime::mailbox::SendError,
    plane::l2_transport::{
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HciObservation {
    CommandComplete {
        opcode: u16,
        status: Option<u8>,
        parameters_after_status: Vec<u8>,
    },
    CommandStatus {
        opcode: u16,
        status: u8,
    },
    AclSubmitted {
        connection_handle: u16,
    },
    IsoSubmitted {
        connection_handle: u16,
    },
    CompletedPackets {
        completions: Vec<(u16, u16)>,
    },
}

pub struct QueueHostTransport {
    tx: TransportTx,
    inbound: VecDeque<HciPacket>,
    observations: VecDeque<HciObservation>,
    suppress_command_complete_once: BTreeSet<u16>,
    fault: Option<LinkFault>,
    acl_rx_packets: u64,
}

impl QueueHostTransport {
    pub fn new(tx: TransportTx) -> Self {
        Self {
            tx,
            inbound: VecDeque::new(),
            observations: VecDeque::new(),
            suppress_command_complete_once: BTreeSet::new(),
            fault: None,
            acl_rx_packets: 0,
        }
    }

    pub fn ingest(&mut self, packet: ControllerPacket) -> Result<(), LinkFault> {
        let (packet_type, bytes) = match packet {
            ControllerPacket::Event(bytes) => {
                if let Some(observation) = observe_native_event(&bytes) {
                    self.observations.push_back(observation);
                }
                if let Some(completions) =
                    observe_number_of_completed_packets(&bytes)
                {
                    self.observations.push_back(
                        HciObservation::CompletedPackets { completions },
                    );
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

        if packet_type == HCI_ACL_DATA_PACKET {
            self.acl_rx_packets = self.acl_rx_packets.saturating_add(1);
        }

        let suppress = matches!(
            self.observations.back(),
            Some(HciObservation::CommandComplete { opcode, .. })
                if self.suppress_command_complete_once.remove(opcode)
        );
        if !suppress {
            self.inbound
                .push_back(normalize_status_only_command_complete(parsed));
        }
        Ok(())
    }

    pub fn take_fault(&mut self) -> Option<LinkFault> {
        self.fault.take()
    }

    pub fn acl_rx_packets(&self) -> u64 {
        self.acl_rx_packets
    }

    pub fn take_observations(&mut self) -> Vec<HciObservation> {
        self.observations.drain(..).collect()
    }

    pub fn suppress_next_command_complete(&mut self, opcode: u16) {
        self.suppress_command_complete_once.insert(opcode);
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

fn normalize_status_only_command_complete(packet: HciPacket) -> HciPacket {
    let HciPacket::Event(Event::CommandComplete {
        num_hci_command_packets,
        command_opcode,
        return_parameters: ReturnParameters::Raw { data },
    }) = packet
    else {
        return packet;
    };

    let status_only = matches!(
        command_opcode,
        HCI_SET_EVENT_MASK_COMMAND
            | HCI_SET_EVENT_MASK_PAGE_2_COMMAND
            | HCI_LE_SET_EVENT_MASK_COMMAND
            | HCI_LE_WRITE_SUGGESTED_DEFAULT_DATA_LENGTH_COMMAND
    );

    if status_only && data.len() == 1 {
        HciPacket::Event(Event::CommandComplete {
            num_hci_command_packets,
            command_opcode,
            return_parameters: ReturnParameters::Status { status: data[0] },
        })
    } else {
        HciPacket::Event(Event::CommandComplete {
            num_hci_command_packets,
            command_opcode,
            return_parameters: ReturnParameters::Raw { data },
        })
    }
}

fn observe_number_of_completed_packets(
    bytes: &[u8],
) -> Option<Vec<(u16, u16)>> {
    if bytes.first().copied()? != 0x13 || bytes.len() < 3 {
        return None;
    }

    let handle_count = usize::from(bytes[2]);
    if bytes.len() != 3 + handle_count * 4 {
        return None;
    }

    let mut completions = Vec::with_capacity(handle_count);
    let mut offset = 3usize;
    for _ in 0..handle_count {
        let handle =
            u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let completed =
            u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        completions.push((handle, completed));
        offset += 4;
    }
    Some(completions)
}

fn observe_native_event(bytes: &[u8]) -> Option<HciObservation> {
    match bytes.first().copied()? {
        // HCI Command Complete:
        // event, param_len, num_cmd_packets, opcode_le[2], return_parameters...
        0x0e if bytes.len() >= 5 => Some(HciObservation::CommandComplete {
            opcode: u16::from_le_bytes([bytes[3], bytes[4]]),
            status: bytes.get(5).copied(),
            parameters_after_status: bytes.get(6..).unwrap_or(&[]).to_vec(),
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
        let connection_handle = packet.connection_handle;
        let sent =
            self.send_native(HCI_ACL_DATA_PACKET, packet.to_bytes());
        if sent {
            self.observations.push_back(HciObservation::AclSubmitted {
                connection_handle,
            });
        }
        sent
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
        let connection_handle = packet.connection_handle;
        let sent =
            self.send_native(HCI_ISO_DATA_PACKET, packet.to_bytes());
        if sent {
            self.observations.push_back(HciObservation::IsoSubmitted {
                connection_handle,
            });
        }
        sent
    }

    fn drain_host_events(&mut self, _controller_id: usize) -> Vec<HciPacket> {
        self.inbound.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plane::l2_transport::worker::TransportTx;

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
    fn status_only_host_init_completion_is_normalized_for_bumble_device() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        link.ingest(ControllerPacket::Event(vec![
            0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
        ]))
        .unwrap();

        assert_eq!(
            link.drain_host_events(0),
            vec![HciPacket::Event(Event::CommandComplete {
                num_hci_command_packets: 1,
                command_opcode: HCI_SET_EVENT_MASK_COMMAND,
                return_parameters: ReturnParameters::Status { status: 0 },
            })]
        );
    }

    #[test]
    fn unknown_raw_command_complete_is_not_reinterpreted_as_status() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        link.ingest(ControllerPacket::Event(vec![
            0x0e, 0x04, 0x01, 0xff, 0x3f, 0x7a,
        ]))
        .unwrap();

        assert_eq!(
            link.drain_host_events(0),
            vec![HciPacket::Event(Event::CommandComplete {
                num_hci_command_packets: 1,
                command_opcode: 0x3fff,
                return_parameters: ReturnParameters::Raw { data: vec![0x7a] },
            })]
        );
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
                parameters_after_status: Vec::new(),
            }]
        );
        assert_eq!(link.drain_host_events(0).len(), 1);
    }

    #[test]
    fn command_complete_journals_payload_after_status() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        // LE Read Resolving List Size, status=0, capacity=0x10.
        link.ingest(ControllerPacket::Event(vec![
            0x0e, 0x05, 0x01, 0x2a, 0x20, 0x00, 0x10,
        ]))
        .unwrap();

        assert_eq!(
            link.take_observations(),
            vec![HciObservation::CommandComplete {
                opcode: 0x202a,
                status: Some(0),
                parameters_after_status: vec![0x10],
            }]
        );
    }

    #[test]
    fn one_shot_suppressed_completion_is_journaled_but_not_delivered() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);
        link.suppress_next_command_complete(0x1009);

        link.ingest(ControllerPacket::Event(vec![
            0x0e, 0x0a, 0x01, 0x09, 0x10, 0x00,
            1, 2, 3, 4, 5, 6,
        ]))
        .unwrap();

        assert_eq!(
            link.take_observations(),
            vec![HciObservation::CommandComplete {
                opcode: 0x1009,
                status: Some(0),
                parameters_after_status: vec![1, 2, 3, 4, 5, 6],
            }]
        );
        assert!(link.drain_host_events(0).is_empty());

        // Suppression is one-shot: the next completion reaches Bumble.
        link.ingest(ControllerPacket::Event(vec![
            0x0e, 0x0a, 0x01, 0x09, 0x10, 0x00,
            6, 5, 4, 3, 2, 1,
        ]))
        .unwrap();
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
    fn valid_controller_acl_ingest_counts_rx_after_parse() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        let packet = AclDataPacket {
            connection_handle: 0x0800,
            pb_flag: 0b10,
            bc_flag: 0,
            data_total_length: 4,
            data: vec![0, 0, 4, 0],
        }
        .to_bytes();

        link.ingest(ControllerPacket::Acl(packet[1..].to_vec()))
            .unwrap();

        assert_eq!(link.acl_rx_packets(), 1);
        assert_eq!(link.drain_host_events(0).len(), 1);
    }

    #[test]
    fn successful_acl_submission_is_journaled_but_failed_enqueue_is_not() {
        let (tx, rx) = tx(1);
        let mut link = QueueHostTransport::new(tx);

        let packet = AclDataPacket {
            connection_handle: 0x0800,
            pb_flag: 0,
            bc_flag: 0,
            data_total_length: 4,
            data: vec![0, 0, 4, 0],
        };
        assert!(link.send_acl_packet(0, packet.clone()));
        assert_eq!(
            link.take_observations(),
            vec![HciObservation::AclSubmitted {
                connection_handle: 0x0800,
            }]
        );

        // Keep the bounded transport lane full; the second ACL must fail and
        // must not be accounted as submitted.
        assert!(!link.send_acl_packet(0, packet));
        assert!(link.take_observations().is_empty());
        let _ = rx.try_recv().unwrap();
    }

    #[test]
    fn successful_iso_submission_is_journaled_but_failed_enqueue_is_not() {
        let (tx, rx) = tx(1);
        let mut link = QueueHostTransport::new(tx);

        let packet = IsoDataPacket {
            connection_handle: 0x0900,
            pb_flag: 0b10,
            ts_flag: 0,
            data_total_length: 7,
            time_stamp: None,
            packet_sequence_number: Some(1),
            iso_sdu_length: Some(3),
            packet_status_flag: Some(0),
            iso_sdu_fragment: vec![1, 2, 3],
        };

        assert!(link.send_iso_packet(0, packet.clone()));
        assert_eq!(
            link.take_observations(),
            vec![HciObservation::IsoSubmitted {
                connection_handle: 0x0900,
            }]
        );

        assert!(!link.send_iso_packet(0, packet));
        assert!(link.take_observations().is_empty());
        let _ = rx.try_recv().unwrap();
    }

    #[test]
    fn number_of_completed_packets_is_journaled_per_handle() {
        let (tx, _rx) = tx(4);
        let mut link = QueueHostTransport::new(tx);

        link.ingest(ControllerPacket::Event(vec![
            0x13, 0x09, 0x02,
            0x00, 0x08, 0x01, 0x00,
            0x01, 0x08, 0x02, 0x00,
        ]))
        .unwrap();

        assert_eq!(
            link.take_observations(),
            vec![HciObservation::CompletedPackets {
                completions: vec![(0x0800, 1), (0x0801, 2)],
            }]
        );
        assert_eq!(link.drain_host_events(0).len(), 1);
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
