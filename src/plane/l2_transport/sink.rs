use crate::runtime::mailbox::{BoundedSender, SendError};

use super::packet::ControllerPacket;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketSinkError {
    Full,
    Disconnected,
}

pub trait ControllerPacketSink {
    fn try_send_controller(
        &self,
        packet: ControllerPacket,
    ) -> Result<(), PacketSinkError>;
}

impl ControllerPacketSink for BoundedSender<ControllerPacket> {
    fn try_send_controller(
        &self,
        packet: ControllerPacket,
    ) -> Result<(), PacketSinkError> {
        self.try_send(packet).map_err(|error| match error {
            SendError::Full => PacketSinkError::Full,
            SendError::Disconnected => PacketSinkError::Disconnected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::mailbox;

    #[test]
    fn bounded_sender_adapter_preserves_full_and_disconnect_semantics() {
        let (tx, rx) = mailbox::bounded(1);

        tx.try_send_controller(ControllerPacket::Event(vec![1]))
            .unwrap();
        assert_eq!(
            tx.try_send_controller(ControllerPacket::Event(vec![2])),
            Err(PacketSinkError::Full)
        );

        drop(rx);
        assert_eq!(
            tx.try_send_controller(ControllerPacket::Event(vec![3])),
            Err(PacketSinkError::Disconnected)
        );
    }
}
