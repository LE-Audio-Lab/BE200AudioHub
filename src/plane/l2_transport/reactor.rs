use std::{
    collections::HashMap,
    sync::Arc,
};

use crate::{
    platform::backend::{
        BackendError, DeviceBackend, RxEndpoint, TransferCompletion, TransferId,
        TxEndpoint,
    },
};

use super::{
    packet::{
        reassembly::{PacketReassembler, ReassemblyError},
        ControllerPacketKind, HostPacket,
    },
    interface::condition::{
        TransportConditionUpdate, TransportState,
    },
    sink::{ControllerPacketSink, PacketSinkError},
    tx_audit::TransportTxAudit,
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
    rx_packets: u64,
    tx_packets: u64,
    faults: u64,
    tx_audit: Arc<TransportTxAudit>,
    pending_read_ids: HashMap<TransferId, RxEndpoint>,
    acl_rx_read_depth: usize,
}

impl<B: DeviceBackend, S: ControllerPacketSink> TransportReactor<B, S> {
    pub fn new(
        backend: B,
        hci_rx: S,
        tx_audit: Arc<TransportTxAudit>,
    ) -> Self {
        Self::new_with_acl_rx_depth(backend, hci_rx, tx_audit, 1)
    }

    pub fn new_with_acl_rx_depth(
        backend: B,
        hci_rx: S,
        tx_audit: Arc<TransportTxAudit>,
        acl_rx_read_depth: usize,
    ) -> Self {
        assert!(acl_rx_read_depth > 0, "ACL RX read depth must be nonzero");
        Self {
            backend,
            state: ReactorState::Stopped,
            hci_rx,
            event_rx: PacketReassembler::new(ControllerPacketKind::Event),
            acl_rx: PacketReassembler::new(ControllerPacketKind::Acl),
            iso_rx: PacketReassembler::new(ControllerPacketKind::Iso),
            rx_packets: 0,
            tx_packets: 0,
            faults: 0,
            tx_audit,
            pending_read_ids: HashMap::new(),
            acl_rx_read_depth,
        }
    }

    pub fn state(&self) -> ReactorState {
        self.state
    }

    pub fn condition_update(&self) -> TransportConditionUpdate {
        let state = match self.state {
            ReactorState::Stopped | ReactorState::Detached => {
                TransportState::Detached
            }
            ReactorState::Running => TransportState::Ready,
            ReactorState::Faulted => TransportState::Faulted,
        };
        TransportConditionUpdate {
            state,
            rx_packets: self.rx_packets,
            tx_packets: self.tx_packets,
            faults: self.faults,
        }
    }

    pub fn start(&mut self) -> Result<(), TransportFault> {
        if !self.backend.is_attached() {
            self.state = ReactorState::Detached;
            return Err(TransportFault::Backend(BackendError::DeviceGone));
        }

        self.arm_rx(RxEndpoint::Event)?;
        for _ in 0..self.acl_rx_read_depth {
            self.arm_rx(RxEndpoint::Acl)?;
        }
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
        match self.backend.submit_write(endpoint, bytes) {
            Ok(_) => {
                self.tx_packets = self.tx_packets.saturating_add(1);
                if endpoint == TxEndpoint::Acl {
                    self.tx_audit.observe_usb_submitted();
                }
                Ok(())
            }
            Err(error) => Err(TransportFault::Backend(error)),
        }
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
                id,
                endpoint,
                bytes,
            } => self.handle_read(id, endpoint, &bytes),
            TransferCompletion::Write { endpoint, .. } => {
                if endpoint == TxEndpoint::Acl {
                    self.tx_audit.observe_usb_completed();
                }
                StepOutcome::WriteCompleted
            },
            TransferCompletion::Cancelled { id } => {
                if let Some(endpoint) = self.pending_read_ids.remove(&id) {
                    self.tx_audit.observe_read_cancelled(endpoint);
                }
                StepOutcome::Cancelled
            },
            TransferCompletion::DeviceGone => {
                self.state = ReactorState::Detached;
                StepOutcome::Detached
            }
            TransferCompletion::Failed { .. } => self.fail(TransportFault::TransferFailed),
        }
    }

    fn handle_read(
        &mut self,
        id: TransferId,
        endpoint: RxEndpoint,
        bytes: &[u8],
    ) -> StepOutcome {
        let tracked = self.pending_read_ids.remove(&id);
        debug_assert_eq!(tracked, Some(endpoint));
        self.tx_audit.observe_read_completion(endpoint);

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
            self.rx_packets = self.rx_packets.saturating_add(1);
        }

        if self.state == ReactorState::Running {
            if let Err(error) = self.arm_rx(endpoint) {
                return self.fail(error);
            }
        }

        StepOutcome::PacketsDelivered(packets.len())
    }

    fn arm_rx(&mut self, endpoint: RxEndpoint) -> Result<(), TransportFault> {
        let id = self
            .backend
            .arm_read(endpoint)
            .map_err(TransportFault::Backend)?;
        let old = self.pending_read_ids.insert(id, endpoint);
        debug_assert!(old.is_none(), "duplicate read TransferId");
        self.tx_audit.observe_read_arm(endpoint);
        Ok(())
    }

    fn fail(&mut self, fault: TransportFault) -> StepOutcome {
        self.state = ReactorState::Faulted;
        self.faults = self.faults.saturating_add(1);
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
        (
            TransportReactor::new(
                TestBackend::default(),
                tx,
                Arc::new(TransportTxAudit::default()),
            ),
            rx,
        )
    }

    #[test]
    fn start_arms_event_and_acl_concurrently() {
        let (mut reactor, _rx) = reactor(8);
        reactor.start().unwrap();
        assert!(reactor.backend.pending_read(RxEndpoint::Event));
        assert!(reactor.backend.pending_read(RxEndpoint::Acl));
    }

    #[test]
    fn acl_rx_depth_two_keeps_two_reads_outstanding() {
        let (tx, _rx) = mailbox::bounded(8);
        let audit = Arc::new(TransportTxAudit::default());
        let mut reactor = TransportReactor::new_with_acl_rx_depth(
            TestBackend::default(),
            tx,
            audit.clone(),
            2,
        );
        reactor.start().unwrap();

        assert_eq!(
            reactor.backend.pending_read_count(RxEndpoint::Event),
            1
        );
        assert_eq!(
            reactor.backend.pending_read_count(RxEndpoint::Acl),
            2
        );
        let snapshot = audit.snapshot();
        assert_eq!(snapshot.acl_read_arms, 2);
        assert_eq!(snapshot.acl_read_completions, 0);

        reactor
            .backend
            .inject_read(
                RxEndpoint::Acl,
                vec![0x00, 0x08, 0x00, 0x00],
            )
            .unwrap();
        assert_eq!(reactor.step(), StepOutcome::PacketsDelivered(1));
        assert_eq!(
            reactor.backend.pending_read_count(RxEndpoint::Acl),
            2
        );
        let snapshot = audit.snapshot();
        assert_eq!(snapshot.acl_read_arms, 3);
        assert_eq!(snapshot.acl_read_completions, 1);
    }

    #[test]
    fn cancelled_acl_read_is_visible_and_not_mistaken_for_pending() {
        let (mut reactor, _rx) = reactor(8);
        reactor.start().unwrap();

        reactor.backend.cancel_read(RxEndpoint::Acl).unwrap();
        assert_eq!(reactor.step(), StepOutcome::Cancelled);

        let snapshot = reactor.tx_audit.snapshot();
        assert_eq!(snapshot.acl_read_arms, 1);
        assert_eq!(snapshot.acl_read_completions, 0);
        assert_eq!(snapshot.acl_read_cancelled, 1);
        assert!(reactor.backend.pending_read(RxEndpoint::Event));
        assert!(!reactor.backend.pending_read(RxEndpoint::Acl));
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
        assert_eq!(
            reactor.condition_update(),
            TransportConditionUpdate {
                state: TransportState::Faulted,
                rx_packets: 1,
                tx_packets: 0,
                faults: 1,
            }
        );
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
        assert_eq!(reactor.condition_update().tx_packets, 3);
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
