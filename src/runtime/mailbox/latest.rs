//! Single-producer coalesced-latest mailbox.
//!
//! Capacity is exactly one logical snapshot. Publishing never blocks. When a
//! newer value arrives while one is pending, the producer evicts the pending
//! value and stores the newer one. This is telemetry delivery, not a reliable
//! semantic queue.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crossbeam_channel::{
    bounded, Receiver, Sender, TryRecvError, TrySendError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatestSendError {
    Disconnected,
}

pub struct LatestSender<T> {
    tx: Sender<T>,
    evict_rx: Receiver<T>,
    closed: Arc<AtomicBool>,
}

pub struct LatestReceiver<T> {
    rx: Receiver<T>,
    closed: Arc<AtomicBool>,
}

pub fn latest<T>() -> (LatestSender<T>, LatestReceiver<T>) {
    let (tx, rx) = bounded(1);
    let closed = Arc::new(AtomicBool::new(false));
    (
        LatestSender {
            tx,
            evict_rx: rx.clone(),
            closed: Arc::clone(&closed),
        },
        LatestReceiver { rx, closed },
    )
}

impl<T> LatestSender<T> {
    pub fn try_publish(&self, value: T) -> Result<(), LatestSendError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(LatestSendError::Disconnected);
        }

        match self.tx.try_send(value) {
            Ok(()) => Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                Err(LatestSendError::Disconnected)
            }
            Err(TrySendError::Full(value)) => {
                // One producer and one consumer. Either we remove the stale
                // pending value, or the consumer raced us and already removed
                // it. Both cases make room for the latest snapshot.
                match self.evict_rx.try_recv() {
                    Ok(_) | Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        return Err(LatestSendError::Disconnected)
                    }
                }

                match self.tx.try_send(value) {
                    Ok(()) => Ok(()),
                    Err(TrySendError::Disconnected(_)) => {
                        Err(LatestSendError::Disconnected)
                    }
                    Err(TrySendError::Full(value)) => {
                        // Defensive second replacement. With the intended SPSC
                        // ownership this can only be a race with the consumer,
                        // never an unbounded retry loop.
                        let _ = self.evict_rx.try_recv();
                        self.tx
                            .try_send(value)
                            .map_err(|_| LatestSendError::Disconnected)
                    }
                }
            }
        }
    }
}

impl<T> LatestReceiver<T> {
    pub fn try_recv(&self) -> Result<T, super::RecvError> {
        self.rx.try_recv().map_err(|error| match error {
            TryRecvError::Empty => super::RecvError::Empty,
            TryRecvError::Disconnected => super::RecvError::Disconnected,
        })
    }
}

impl<T> Drop for LatestReceiver<T> {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_snapshot_replaces_pending_snapshot_without_blocking() {
        let (tx, rx) = latest();
        tx.try_publish(1).unwrap();
        tx.try_publish(2).unwrap();
        tx.try_publish(3).unwrap();

        assert_eq!(rx.try_recv(), Ok(3));
        assert_eq!(rx.try_recv(), Err(super::super::RecvError::Empty));
    }

    #[test]
    fn receiver_drop_is_visible_to_producer() {
        let (tx, rx) = latest::<u8>();
        drop(rx);
        assert_eq!(
            tx.try_publish(1),
            Err(LatestSendError::Disconnected)
        );
    }
}
