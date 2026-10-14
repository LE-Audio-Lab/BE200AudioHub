use super::packet::{ControllerPacket, ControllerPacketKind};

const DEFAULT_MAX_PACKET_BYTES: usize = 65_539;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReassemblyError {
    PacketTooLarge { declared: usize, max: usize },
}

pub struct PacketReassembler {
    kind: ControllerPacketKind,
    buffered: Vec<u8>,
    max_packet_bytes: usize,
}

impl PacketReassembler {
    pub fn new(kind: ControllerPacketKind) -> Self {
        Self::with_max_packet_bytes(kind, DEFAULT_MAX_PACKET_BYTES)
    }

    pub fn with_max_packet_bytes(
        kind: ControllerPacketKind,
        max_packet_bytes: usize,
    ) -> Self {
        Self {
            kind,
            buffered: Vec::new(),
            max_packet_bytes,
        }
    }

    pub fn buffered_len(&self) -> usize {
        self.buffered.len()
    }

    pub fn clear(&mut self) {
        self.buffered.clear();
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<ControllerPacket>, ReassemblyError> {
        self.buffered.extend_from_slice(bytes);
        let mut packets = Vec::new();

        loop {
            let Some(total_len) = declared_packet_len(self.kind, &self.buffered) else {
                break;
            };

            if total_len > self.max_packet_bytes {
                self.buffered.clear();
                return Err(ReassemblyError::PacketTooLarge {
                    declared: total_len,
                    max: self.max_packet_bytes,
                });
            }

            if self.buffered.len() < total_len {
                break;
            }

            let rest = self.buffered.split_off(total_len);
            let packet_bytes = std::mem::replace(&mut self.buffered, rest);
            packets.push(match self.kind {
                ControllerPacketKind::Event => ControllerPacket::Event(packet_bytes),
                ControllerPacketKind::Acl => ControllerPacket::Acl(packet_bytes),
                ControllerPacketKind::Iso => ControllerPacket::Iso(packet_bytes),
            });
        }

        Ok(packets)
    }
}

fn declared_packet_len(kind: ControllerPacketKind, bytes: &[u8]) -> Option<usize> {
    match kind {
        ControllerPacketKind::Event => {
            if bytes.len() < 2 {
                None
            } else {
                Some(2 + usize::from(bytes[1]))
            }
        }
        ControllerPacketKind::Acl | ControllerPacketKind::Iso => {
            if bytes.len() < 4 {
                None
            } else {
                Some(4 + usize::from(u16::from_le_bytes([bytes[2], bytes[3]])))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_partial_reads_are_preserved_until_complete() {
        let mut rx = PacketReassembler::new(ControllerPacketKind::Event);
        assert!(rx.push(&[0x0e]).unwrap().is_empty());
        assert_eq!(rx.buffered_len(), 1);
        assert!(rx.push(&[0x03, 0x01]).unwrap().is_empty());
        let packets = rx.push(&[0x02, 0x03]).unwrap();
        assert_eq!(
            packets,
            vec![ControllerPacket::Event(vec![0x0e, 0x03, 0x01, 0x02, 0x03])]
        );
        assert_eq!(rx.buffered_len(), 0);
    }

    #[test]
    fn acl_partial_header_and_payload_are_reassembled() {
        let mut rx = PacketReassembler::new(ControllerPacketKind::Acl);
        assert!(rx.push(&[0x01, 0x20, 0x03]).unwrap().is_empty());
        assert!(rx.push(&[0x00, 0xaa]).unwrap().is_empty());
        let packets = rx.push(&[0xbb, 0xcc]).unwrap();
        assert_eq!(
            packets,
            vec![ControllerPacket::Acl(vec![
                0x01, 0x20, 0x03, 0x00, 0xaa, 0xbb, 0xcc
            ])]
        );
    }

    #[test]
    fn one_transfer_may_contain_multiple_complete_packets() {
        let mut rx = PacketReassembler::new(ControllerPacketKind::Event);
        let packets = rx
            .push(&[
                0x0e, 0x01, 0xaa,
                0x0f, 0x02, 0xbb, 0xcc,
            ])
            .unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].bytes(), &[0x0e, 0x01, 0xaa]);
        assert_eq!(packets[1].bytes(), &[0x0f, 0x02, 0xbb, 0xcc]);
    }

    #[test]
    fn impossible_declared_size_is_rejected_and_buffer_reset() {
        let mut rx =
            PacketReassembler::with_max_packet_bytes(ControllerPacketKind::Acl, 8);
        assert_eq!(
            rx.push(&[0x01, 0x20, 0x10, 0x00]),
            Err(ReassemblyError::PacketTooLarge {
                declared: 20,
                max: 8,
            })
        );
        assert_eq!(rx.buffered_len(), 0);
    }
}
