use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransferId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RxEndpoint {
    Event,
    Acl,
    Iso,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TxEndpoint {
    Command,
    Acl,
    Iso,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendError {
    AlreadyPending(RxEndpoint),
    NotPending(RxEndpoint),
    DeviceGone,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferCompletion {
    Read {
        id: TransferId,
        endpoint: RxEndpoint,
        bytes: Vec<u8>,
    },
    Write {
        id: TransferId,
        endpoint: TxEndpoint,
        bytes: usize,
    },
    Cancelled {
        id: TransferId,
    },
    DeviceGone,
    Failed {
        id: Option<TransferId>,
    },
}

pub trait DeviceBackend {
    fn arm_read(&mut self, endpoint: RxEndpoint) -> Result<TransferId, BackendError>;

    fn submit_write(
        &mut self,
        endpoint: TxEndpoint,
        bytes: Vec<u8>,
    ) -> Result<TransferId, BackendError>;

    /// Wait/poll for one OS/backend completion. This is called only by
    /// TransportContext; higher layers never invoke backend I/O directly.
    fn next_completion(&mut self) -> Option<TransferCompletion>;

    fn cancel_all(&mut self);

    fn is_attached(&self) -> bool;
}

#[derive(Default)]
pub(crate) struct CompletionQueue {
    queue: VecDeque<TransferCompletion>,
}

impl CompletionQueue {
    pub(crate) fn push(&mut self, completion: TransferCompletion) {
        self.queue.push_back(completion);
    }

    pub(crate) fn pop(&mut self) -> Option<TransferCompletion> {
        self.queue.pop_front()
    }
}
