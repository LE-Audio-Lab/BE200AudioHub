use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    platform::backend::{BackendError, DeviceBackend},
    runtime::{
        cancellation::CancellationToken,
        mailbox::{self, BoundedReceiver, BoundedSender, RecvError, SendError},
    },
};

use super::{
    packet::{ControllerPacket, HostPacket},
    reactor::{StepOutcome, TransportFault, TransportReactor},
    sink::ControllerPacketSink,
    tx_audit::{AclTransportTxSnapshot, TransportTxAudit},
};

const TX_BUDGET_PER_TURN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportWorkerExit {
    Cancelled,
    Detached,
    TxDisconnected,
    BackendOpenFailed(BackendError),
    Faulted(TransportFault),
}

#[derive(Clone, Debug)]
pub struct TransportTx {
    tx: BoundedSender<HostPacket>,
    cancel: CancellationToken,
    audit: Arc<TransportTxAudit>,
}

impl TransportTx {
    #[cfg(test)]
    pub(crate) fn test_channel(
        capacity: usize,
    ) -> (
        Self,
        BoundedReceiver<HostPacket>,
    ) {
        let (tx, rx) = mailbox::bounded(capacity);
        (
            Self {
                tx,
                cancel: CancellationToken::new(),
                audit: Arc::new(TransportTxAudit::default()),
            },
            rx,
        )
    }

    pub fn try_send(&self, packet: HostPacket) -> Result<(), SendError> {
        let is_acl = matches!(&packet, HostPacket::Acl(_));
        let result = self.tx.try_send(packet);
        if result.is_ok() && is_acl {
            self.audit.observe_mailbox_enqueued();
        }
        result
    }

    pub(crate) fn acl_tx_audit_snapshot(
        &self,
    ) -> AclTransportTxSnapshot {
        self.audit.snapshot()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

pub struct TransportWorkerJoin {
    join: JoinHandle<TransportWorkerExit>,
}

impl TransportWorkerJoin {
    pub fn join(self) -> TransportWorkerExit {
        self.join
            .join()
            .unwrap_or(TransportWorkerExit::Faulted(
                TransportFault::Backend(BackendError::Failed),
            ))
    }
}

pub fn spawn_transport_worker<B, F>(
    factory: F,
    tx_capacity: usize,
    hci_rx_capacity: usize,
) -> (
    TransportTx,
    BoundedReceiver<ControllerPacket>,
    TransportWorkerJoin,
)
where
    B: DeviceBackend + 'static,
    F: FnOnce() -> Result<B, BackendError> + Send + 'static,
{
    let (hci_tx, hci_rx) = mailbox::bounded(hci_rx_capacity);
    let (tx, join) =
        spawn_transport_worker_with_sink(factory, tx_capacity, hci_tx);
    (tx, hci_rx, join)
}

pub fn spawn_transport_worker_with_sink<B, F, S>(
    factory: F,
    tx_capacity: usize,
    hci_sink: S,
) -> (TransportTx, TransportWorkerJoin)
where
    B: DeviceBackend + 'static,
    F: FnOnce() -> Result<B, BackendError> + Send + 'static,
    S: ControllerPacketSink + Send + 'static,
{
    let (tx, tx_rx) = mailbox::bounded(tx_capacity);
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let audit = Arc::new(TransportTxAudit::default());
    let worker_audit = audit.clone();

    let join = thread::Builder::new()
        .name("UsbReactor".into())
        .spawn(move || {
            let backend = match factory() {
                Ok(backend) => backend,
                Err(error) => {
                    return TransportWorkerExit::BackendOpenFailed(error)
                }
            };

            let mut reactor =
                TransportReactor::new(backend, hci_sink, worker_audit);
            if let Err(fault) = reactor.start() {
                return TransportWorkerExit::Faulted(fault);
            }

            let exit = run_loop(&mut reactor, &tx_rx, &worker_cancel);
            reactor.stop();
            exit
        })
        .expect("failed to spawn USB reactor");

    (
        TransportTx { tx, cancel, audit },
        TransportWorkerJoin { join },
    )
}

fn run_loop<B, S>(
    reactor: &mut TransportReactor<B, S>,
    tx_rx: &BoundedReceiver<HostPacket>,
    cancel: &CancellationToken,
) -> TransportWorkerExit
where
    B: DeviceBackend,
    S: ControllerPacketSink,
{
    loop {
        if cancel.is_cancelled() {
            return TransportWorkerExit::Cancelled;
        }

        for _ in 0..TX_BUDGET_PER_TURN {
            match tx_rx.try_recv() {
                Ok(packet) => {
                    if let Err(fault) = reactor.send(packet) {
                        return TransportWorkerExit::Faulted(fault);
                    }
                }
                Err(RecvError::Empty) => break,
                Err(RecvError::Disconnected) => {
                    return TransportWorkerExit::TxDisconnected;
                }
            }
        }

        match reactor.step() {
            StepOutcome::Idle => {
                // Test/nonblocking backends can return immediately. A real
                // libusb backend blocks inside handle_events until completion
                // or its short mailbox-fairness slice expires.
                thread::sleep(Duration::from_micros(100));
            }
            StepOutcome::PacketsDelivered(_)
            | StepOutcome::WriteCompleted
            | StepOutcome::Cancelled => {}
            StepOutcome::Detached => return TransportWorkerExit::Detached,
            StepOutcome::Faulted(fault) => {
                return TransportWorkerExit::Faulted(fault)
            }
        }
    }
}

#[cfg(windows)]
pub fn spawn_be200_transport(
    tx_capacity: usize,
    hci_rx_capacity: usize,
) -> (
    TransportTx,
    BoundedReceiver<ControllerPacket>,
    TransportWorkerJoin,
) {
    spawn_transport_worker(
        crate::platform::windows::be200::open_backend,
        tx_capacity,
        hci_rx_capacity,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use crate::platform::backend::{
        RxEndpoint, TransferCompletion, TransferId, TxEndpoint,
    };

    use super::*;

    #[derive(Default)]
    struct TestState {
        next_id: u64,
        writes: Vec<(TxEndpoint, Vec<u8>)>,
        cancelled: bool,
    }

    struct RecordingBackend {
        state: Arc<Mutex<TestState>>,
        attached: bool,
    }

    impl RecordingBackend {
        fn new(state: Arc<Mutex<TestState>>) -> Self {
            Self {
                state,
                attached: true,
            }
        }

        fn id(&mut self) -> TransferId {
            let mut state = self.state.lock().unwrap();
            state.next_id += 1;
            TransferId(state.next_id)
        }
    }

    impl DeviceBackend for RecordingBackend {
        fn arm_read(
            &mut self,
            _endpoint: RxEndpoint,
        ) -> Result<TransferId, BackendError> {
            Ok(self.id())
        }

        fn submit_write(
            &mut self,
            endpoint: TxEndpoint,
            bytes: Vec<u8>,
        ) -> Result<TransferId, BackendError> {
            let id = self.id();
            self.state.lock().unwrap().writes.push((endpoint, bytes));
            Ok(id)
        }

        fn next_completion(&mut self) -> Option<TransferCompletion> {
            None
        }

        fn cancel_all(&mut self) {
            self.state.lock().unwrap().cancelled = true;
        }

        fn is_attached(&self) -> bool {
            self.attached
        }
    }

    #[derive(Clone)]
    struct CountingSink {
        count: Arc<Mutex<usize>>,
    }

    impl ControllerPacketSink for CountingSink {
        fn try_send_controller(
            &self,
            _packet: ControllerPacket,
        ) -> Result<(), crate::plane::l2_transport::sink::PacketSinkError> {
            *self.count.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[test]
    fn worker_accepts_custom_controller_packet_sink() {
        let state = Arc::new(Mutex::new(TestState::default()));
        let factory_state = state.clone();
        let count = Arc::new(Mutex::new(0usize));
        let sink = CountingSink {
            count: count.clone(),
        };

        let (tx, join) = spawn_transport_worker_with_sink(
            move || Ok(RecordingBackend::new(factory_state)),
            4,
            sink,
        );

        tx.cancel();
        assert_eq!(join.join(), TransportWorkerExit::Cancelled);
        assert_eq!(*count.lock().unwrap(), 0);
        assert!(state.lock().unwrap().cancelled);
    }

    #[test]
    fn worker_moves_host_packets_on_its_own_thread_and_cancels_cleanly() {
        let state = Arc::new(Mutex::new(TestState::default()));
        let factory_state = state.clone();
        let (tx, _hci_rx, join) = spawn_transport_worker(
            move || Ok(RecordingBackend::new(factory_state)),
            8,
            8,
        );

        tx.try_send(HostPacket::Command(vec![1, 2, 3]))
            .unwrap();
        tx.try_send(HostPacket::Iso(vec![4, 5, 6]))
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if state.lock().unwrap().writes.len() == 2 {
                break;
            }
            assert!(Instant::now() < deadline, "worker did not service TX mailbox");
            thread::yield_now();
        }

        tx.cancel();
        assert_eq!(join.join(), TransportWorkerExit::Cancelled);

        let state = state.lock().unwrap();
        assert!(state.cancelled);
        assert_eq!(state.writes[0], (TxEndpoint::Command, vec![1, 2, 3]));
        assert_eq!(state.writes[1], (TxEndpoint::Iso, vec![4, 5, 6]));
    }

    #[test]
    fn cancellation_wins_under_sustained_tx_pressure() {
        let state = Arc::new(Mutex::new(TestState::default()));
        let factory_state = state.clone();
        let (tx, _hci_rx, join) = spawn_transport_worker(
            move || Ok(RecordingBackend::new(factory_state)),
            64,
            8,
        );

        for i in 0u8..32 {
            tx.try_send(HostPacket::Iso(vec![i; 16])).unwrap();
        }
        tx.cancel();

        assert_eq!(join.join(), TransportWorkerExit::Cancelled);
        assert!(state.lock().unwrap().cancelled);
    }

    #[test]
    fn failed_backend_open_is_reported_without_starting_loop() {
        let (_tx, _hci_rx, join) =
            spawn_transport_worker::<RecordingBackend, _>(
                || Err(BackendError::DeviceGone),
                4,
                4,
            );
        assert_eq!(
            join.join(),
            TransportWorkerExit::BackendOpenFailed(BackendError::DeviceGone)
        );
    }

    #[test]
    fn transport_tx_is_bounded_and_nonblocking() {
        // A zero-speed worker is unnecessary to test the public semantics:
        // keep the receiver alive and verify bounded try_send behavior.
        let (raw_tx, _rx) = mailbox::bounded(1);
        let tx = TransportTx {
            tx: raw_tx,
            cancel: CancellationToken::new(),
            audit: Arc::new(TransportTxAudit::default()),
        };
        tx.try_send(HostPacket::Acl(vec![1])).unwrap();
        assert_eq!(
            tx.try_send(HostPacket::Acl(vec![2])),
            Err(SendError::Full)
        );
    }
}
