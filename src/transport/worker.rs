use std::{
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
}

impl TransportTx {
    pub fn try_send(&self, packet: HostPacket) -> Result<(), SendError> {
        self.tx.try_send(packet)
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
    let (tx, tx_rx) = mailbox::bounded(tx_capacity);
    let (hci_tx, hci_rx) = mailbox::bounded(hci_rx_capacity);
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();

    let join = thread::Builder::new()
        .name("UsbReactor".into())
        .spawn(move || {
            let backend = match factory() {
                Ok(backend) => backend,
                Err(error) => return TransportWorkerExit::BackendOpenFailed(error),
            };

            let mut reactor = TransportReactor::new(backend, hci_tx);
            if let Err(fault) = reactor.start() {
                return TransportWorkerExit::Faulted(fault);
            }

            let exit = run_loop(&mut reactor, &tx_rx, &worker_cancel);
            reactor.stop();
            exit
        })
        .expect("failed to spawn USB reactor");

    (
        TransportTx { tx, cancel },
        hci_rx,
        TransportWorkerJoin { join },
    )
}

fn run_loop<B: DeviceBackend>(
    reactor: &mut TransportReactor<B>,
    tx_rx: &BoundedReceiver<HostPacket>,
    cancel: &CancellationToken,
) -> TransportWorkerExit {
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
        crate::platform::windows_be200::open_backend,
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
        };
        tx.try_send(HostPacket::Acl(vec![1])).unwrap();
        assert_eq!(
            tx.try_send(HostPacket::Acl(vec![2])),
            Err(SendError::Full)
        );
    }
}
