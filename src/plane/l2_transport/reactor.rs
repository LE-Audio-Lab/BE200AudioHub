use crate::{
    platform::backend::{
        BackendError, DeviceBackend, RxEndpoint, TransferCompletion, TxEndpoint,
    },
};

use super::{
    packet::{
        reassembly::{PacketReassembler, ReassemblyError},
        ControllerPacketKind, HostPacket,
    },
    sink::{ControllerPacketSink, PacketSinkError},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactorState {
    Stopped,
    Running,
    Detached,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportFault {
    Backend(BackendError),
    Reassembly(ReassemblyError),
    HciInboxFull,
    HciInboxDisconnected,
    TransferFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    Idle,
    PacketsDelivered(usize),
    WriteCompleted,
    Cancelled,
    Detached,
    Faulted(TransportFault),
}

pub struct TransportReactor<B: DeviceBackend, S: ControllerPacketSink> {
    backend: B,
    state: ReactorState,
    hci_rx: S,
    event_rx: PacketReassembler,
    acl_rx: PacketReassembler,
    iso_rx: PacketReassembler,
}

impl<B: DeviceBackend, S: ControllerPacketSink> TransportReactor<B, S> {
    pub fn new(backend: B, hci_rx: S) -> Self {
        Self {
            backend,
            state: ReactorState::Stopped,
            hci_rx,
            event_rx: PacketReassembler::new(ControllerPacketKind::Event),
            acl_rx: PacketReassembler::new(ControllerPacketKind::Acl),
            iso_rx: PacketReassembler::new(ControllerPacketKind::Iso),
        }
    }

    pub fn state(&self) -> ReactorState {
        self.state
    }

    pub fn start(&mut self) -> Result<(), TransportFault> {
        if !self.backend.is_attached() {
            self.state = ReactorState::Detached;
            return Err(TransportFault::Backend(BackendError::DeviceGone));
        }

        self.arm_rx(RxEndpoint::Event)?;
        self.arm_rx(RxEndpoint::Acl)?;
        self.state = ReactorState::Running;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.backend.cancel_all();
        self.event_rx.clear();
        self.acl_rx.clear();
        self.iso_rx.clear();
        self.state = ReactorState::Stopped;
    }

    pub fn send(&mut self, packet: HostPacket) -> Result<(), TransportFault> {
        if self.state != ReactorState::Running {
            return Err(TransportFault::Backend(BackendError::Cancelled));
        }
        let (endpoint, bytes) = match packet {
            HostPacket::Command(bytes) => (TxEndpoint::Command, bytes),
            HostPacket::Acl(bytes) => (TxEndpoint::Acl, bytes),
            HostPacket::Iso(bytes) => (TxEndpoint::Iso, bytes),
        };
        self.backend
            .submit_write(endpoint, bytes)
            .map(|_| ())
            .map_err(TransportFault::Backend)
    }

    pub fn step(&mut self) -> StepOutcome {
        if self.state == ReactorState::Stopped {
            return StepOutcome::Idle;
        }

        let Some(completion) = self.backend.next_completion() else {
            return StepOutcome::Idle;
        };

        match completion {
            TransferCompletion::Read {
                endpoint,
                bytes,
                ..
            } => self.handle_read(endpoint, &bytes),
            TransferCompletion::Write { .. } => StepOutcome::WriteCompleted,
            TransferCompletion::Cancelled { .. } => StepOutcome::Cancelled,
            TransferCompletion::DeviceGone => {
                self.state = ReactorState::Detached;
                StepOutcome::Detached
            }
            TransferCompletion::Failed { .. } => self.fail(TransportFault::TransferFailed),
        }
    }

    fn handle_read(&mut self, endpoint: RxEndpoint, bytes: &[u8]) -> StepOutcome {
        let packets = {
            let reassembler = match endpoint {
                RxEndpoint::Event => &mut self.event_rx,
                RxEndpoint::Acl => &mut self.acl_rx,
                RxEndpoint::Iso => &mut self.iso_rx,
            };
            match reassembler.push(bytes) {
                Ok(packets) => packets,
                Err(error) => return self.fail(TransportFault::Reassembly(error)),
            }
        };

        for packet in &packets {
            if let Err(error) = self.hci_rx.try_send_controller(packet.clone()) {
                return self.fail(match error {
                    PacketSinkError::Full => TransportFault::HciInboxFull,
                    PacketSinkError::Disconnected => TransportFault::HciInboxDisconnected,
                });
            }
        }

        if self.state == ReactorState::Running {
            if let Err(error) = self.arm_rx(endpoint) {
                return self.fail(error);
            }
        }

        StepOutcome::PacketsDelivered(packets.len())
    }

    fn arm_rx(&mut self, endpoint: RxEndpoint) -> Result<(), TransportFault> {
        self.backend
            .arm_read(endpoint)
            .map(|_| ())
            .map_err(TransportFault::Backend)
    }

    fn fail(&mut self, fault: TransportFault) -> StepOutcome {
        self.state = ReactorState::Faulted;
        self.backend.cancel_all();
        StepOutcome::Faulted(fault)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        platform::backend::test::TestBackend,
        runtime::mailbox,
        plane::l2_transport::packet::ControllerPacket,
    };

    fn reactor(
        hci_capacity: usize,
    ) -> (
        TransportReactor<TestBackend, crate::runtime::mailbox::BoundedSender<ControllerPacket>>,
        crate::runtime::mailbox::BoundedReceiver<ControllerPacket>,
    ) {
        let (tx, rx) = mailbox::bounded(hci_capacity);
        (TransportReactor::new(TestBackend::default(), tx), rx)
    }

    #[test]
    fn start_arms_event_and_acl_concurrently() {
        let (mut reactor, _rx) = reactor(8);
        reactor.start().unwrap();
        assert!(reactor.backend.pending_read(RxEndpoint::Event));
        assert!(reactor.backend.pending_read(RxEndpoint::Acl));
    }

    #[test]
    fn completion_delivers_packet_and_rearms_only_that_endpoint() {
        let (mut reactor, rx) = reactor(8);
        reactor.start().unwrap();

        reactor
            .backend
            .inject_read(RxEndpoint::Event, vec![0x0e, 0x01, 0xaa])
            .unwrap();

        assert_eq!(reactor.step(), StepOutcome::PacketsDelivered(1));
        assert_eq!(
            rx.try_recv(),
            Ok(ControllerPacket::Event(vec![0x0e, 0x01, 0xaa]))
        );
        assert!(reactor.backend.pending_read(RxEndpoint::Event));
        assert!(reactor.backend.pending_read(RxEndpoint::Acl));
    }

    #[test]
    fn partial_completion_is_rearmed_without_emitting_packet() {
        let (mut reactor, rx) = reactor(8);
        reactor.start().unwrap();

        reactor
            .backend
            .inject_read(RxEndpoint::Event, vec![0x0e])
            .unwrap();
        assert_eq!(reactor.step(), StepOutcome::PacketsDelivered(0));
        assert!(reactor.backend.pending_read(RxEndpoint::Event));
        assert!(matches!(
            rx.try_recv(),
            Err(crate::runtime::mailbox::RecvError::Empty)
        ));

        reactor
            .backend
            .inject_read(RxEndpoint::Event, vec![0x01, 0xaa])
            .unwrap();
        assert_eq!(reactor.step(), StepOutcome::PacketsDelivered(1));
        assert_eq!(
            rx.try_recv(),
            Ok(ControllerPacket::Event(vec![0x0e, 0x01, 0xaa]))
        );
    }

    #[test]
    fn hci_inbox_overflow_is_a_transport_fault_not_packet_loss() {
        let (mut reactor, _rx) = reactor(1);
        reactor.start().unwrap();

        reactor
            .backend
            .inject_read(
                RxEndpoint::Event,
                vec![0x0e, 0x01, 0xaa, 0x0f, 0x01, 0xbb],
            )
            .unwrap();

        assert_eq!(
            reactor.step(),
            StepOutcome::Faulted(TransportFault::HciInboxFull)
        );
        assert_eq!(reactor.state(), ReactorState::Faulted);
    }

    #[test]
    fn detach_propagates_without_becoming_idle() {
        let (mut reactor, _rx) = reactor(8);
        reactor.start().unwrap();
        reactor.backend.detach();
        assert_eq!(reactor.step(), StepOutcome::Detached);
        assert_eq!(reactor.state(), ReactorState::Detached);
    }

    #[test]
    fn host_packets_map_to_separate_transport_classes() {
        let (mut reactor, _rx) = reactor(8);
        reactor.start().unwrap();

        reactor.send(HostPacket::Command(vec![1])).unwrap();
        reactor.send(HostPacket::Acl(vec![2])).unwrap();
        reactor.send(HostPacket::Iso(vec![3])).unwrap();

        let writes = reactor.backend.recorded_writes();
        assert_eq!(writes.len(), 3);
        assert!(writes.iter().any(|w| w.endpoint == TxEndpoint::Command));
        assert!(writes.iter().any(|w| w.endpoint == TxEndpoint::Acl));
        assert!(writes.iter().any(|w| w.endpoint == TxEndpoint::Iso));
    }

    #[test]
    fn interleaved_event_and_acl_completions_preserve_arrival_order_without_starvation() {
        let (mut reactor, rx) = reactor(256);
        reactor.start().unwrap();

        let mut expected = Vec::new();
        for i in 0u8..100 {
            let event = vec![0x0e, 0x01, i];
            let acl = vec![0x01, 0x20, 0x01, 0x00, i];

            if i % 2 == 0 {
                reactor
                    .backend
                    .inject_read(RxEndpoint::Acl, acl.clone())
                    .unwrap();
                reactor
                    .backend
                    .inject_read(RxEndpoint::Event, event.clone())
                    .unwrap();
                expected.push(ControllerPacket::Acl(acl));
                expected.push(ControllerPacket::Event(event));
            } else {
                reactor
                    .backend
                    .inject_read(RxEndpoint::Event, event.clone())
                    .unwrap();
                reactor
                    .backend
                    .inject_read(RxEndpoint::Acl, acl.clone())
                    .unwrap();
                expected.push(ControllerPacket::Event(event));
                expected.push(ControllerPacket::Acl(acl));
            }

            assert_eq!(reactor.step(), StepOutcome::PacketsDelivered(1));
            assert_eq!(reactor.step(), StepOutcome::PacketsDelivered(1));
            assert!(reactor.backend.pending_read(RxEndpoint::Event));
            assert!(reactor.backend.pending_read(RxEndpoint::Acl));
        }

        let mut actual = Vec::new();
        while let Ok(packet) = rx.try_recv() {
            actual.push(packet);
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn stop_cancels_pending_io_and_clears_reassembly() {
        let (mut reactor, _rx) = reactor(8);
        reactor.start().unwrap();
        reactor
            .backend
            .inject_read(RxEndpoint::Event, vec![0x0e])
            .unwrap();
        reactor.step();
        reactor.stop();
        assert_eq!(reactor.state(), ReactorState::Stopped);
        assert_eq!(reactor.event_rx.buffered_len(), 0);
    }
}
