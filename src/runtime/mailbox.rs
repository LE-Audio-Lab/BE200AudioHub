use std::sync::mpsc::{
    self, Receiver, SyncSender, TryRecvError, TrySendError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    Full,
    Disconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecvError {
    Empty,
    Disconnected,
}

#[derive(Debug)]
pub struct BoundedSender<T> {
    inner: SyncSender<T>,
}

#[derive(Debug)]
pub struct BoundedReceiver<T> {
    inner: Receiver<T>,
}

pub fn bounded<T>(capacity: usize) -> (BoundedSender<T>, BoundedReceiver<T>) {
    assert!(capacity > 0, "mailbox capacity must be nonzero");
    let (tx, rx) = mpsc::sync_channel(capacity);
    (
        BoundedSender { inner: tx },
        BoundedReceiver { inner: rx },
    )
}

impl<T> Clone for BoundedSender<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> BoundedSender<T> {
    pub fn try_send(&self, value: T) -> Result<(), SendError> {
        self.inner.try_send(value).map_err(|error| match error {
            TrySendError::Full(_) => SendError::Full,
            TrySendError::Disconnected(_) => SendError::Disconnected,
        })
    }
}

impl<T> BoundedReceiver<T> {
    pub fn try_recv(&self) -> Result<T, RecvError> {
        self.inner.try_recv().map_err(|error| match error {
            TryRecvError::Empty => RecvError::Empty,
            TryRecvError::Disconnected => RecvError::Disconnected,
        })
    }

    pub(crate) fn recv(&self) -> Result<T, RecvError> {
        self.inner.recv().map_err(|_| RecvError::Disconnected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_mailbox_is_fifo() {
        let (tx, rx) = bounded(2);
        tx.try_send(10).unwrap();
        tx.try_send(20).unwrap();
        assert_eq!(rx.try_recv(), Ok(10));
        assert_eq!(rx.try_recv(), Ok(20));
        assert_eq!(rx.try_recv(), Err(RecvError::Empty));
    }

    #[test]
    fn producer_never_blocks_when_mailbox_is_full() {
        let (tx, rx) = bounded(1);
        tx.try_send(1).unwrap();
        assert_eq!(tx.try_send(2), Err(SendError::Full));
        assert_eq!(rx.try_recv(), Ok(1));
        tx.try_send(3).unwrap();
        assert_eq!(rx.try_recv(), Ok(3));
    }

    #[test]
    fn disconnect_is_explicit() {
        let (tx, rx) = bounded::<u8>(1);
        drop(rx);
        assert_eq!(tx.try_send(1), Err(SendError::Disconnected));
    }
}
