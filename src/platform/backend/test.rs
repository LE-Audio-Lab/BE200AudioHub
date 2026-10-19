use std::collections::{HashMap, VecDeque};

use super::{
    BackendError, CompletionQueue, DeviceBackend, RxEndpoint, TransferCompletion,
    TransferId, TxEndpoint,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedWrite {
    pub id: TransferId,
    pub endpoint: TxEndpoint,
    pub bytes: Vec<u8>,
}

pub struct TestBackend {
    next_id: u64,
    attached: bool,
    pending_reads: HashMap<RxEndpoint, TransferId>,
    pending_writes: HashMap<TransferId, RecordedWrite>,
    completions: CompletionQueue,
    write_log: VecDeque<RecordedWrite>,
}

impl Default for TestBackend {
    fn default() -> Self {
        Self {
            next_id: 1,
            attached: true,
            pending_reads: HashMap::new(),
            pending_writes: HashMap::new(),
            completions: CompletionQueue::default(),
            write_log: VecDeque::new(),
        }
    }
}

impl TestBackend {
    pub fn pending_read(&self, endpoint: RxEndpoint) -> bool {
        self.pending_reads.contains_key(&endpoint)
    }

    pub fn inject_read(
        &mut self,
        endpoint: RxEndpoint,
        bytes: Vec<u8>,
    ) -> Result<(), BackendError> {
        let id = self
            .pending_reads
            .remove(&endpoint)
            .ok_or(BackendError::NotPending(endpoint))?;
        self.completions.push(TransferCompletion::Read {
            id,
            endpoint,
            bytes,
        });
        Ok(())
    }

    pub fn cancel_read(
        &mut self,
        endpoint: RxEndpoint,
    ) -> Result<(), BackendError> {
        let id = self
            .pending_reads
            .remove(&endpoint)
            .ok_or(BackendError::NotPending(endpoint))?;
        self.completions
            .push(TransferCompletion::Cancelled { id });
        Ok(())
    }

    pub fn complete_next_write(&mut self) -> bool {
        let Some(write) = self.write_log.pop_front() else {
            return false;
        };
        self.pending_writes.remove(&write.id);
        self.completions.push(TransferCompletion::Write {
            id: write.id,
            endpoint: write.endpoint,
            bytes: write.bytes.len(),
        });
        true
    }

    pub fn recorded_writes(&self) -> Vec<RecordedWrite> {
        self.pending_writes.values().cloned().collect()
    }

    pub fn detach(&mut self) {
        self.attached = false;
        self.pending_reads.clear();
        self.pending_writes.clear();
        self.completions.push(TransferCompletion::DeviceGone);
    }

    fn allocate_id(&mut self) -> TransferId {
        let id = TransferId(self.next_id);
        self.next_id += 1;
        id
    }
}

impl DeviceBackend for TestBackend {
    fn arm_read(&mut self, endpoint: RxEndpoint) -> Result<TransferId, BackendError> {
        if !self.attached {
            return Err(BackendError::DeviceGone);
        }
        if self.pending_reads.contains_key(&endpoint) {
            return Err(BackendError::AlreadyPending(endpoint));
        }
        let id = self.allocate_id();
        self.pending_reads.insert(endpoint, id);
        Ok(id)
    }

    fn submit_write(
        &mut self,
        endpoint: TxEndpoint,
        bytes: Vec<u8>,
    ) -> Result<TransferId, BackendError> {
        if !self.attached {
            return Err(BackendError::DeviceGone);
        }
        let id = self.allocate_id();
        let write = RecordedWrite {
            id,
            endpoint,
            bytes,
        };
        self.pending_writes.insert(id, write.clone());
        self.write_log.push_back(write);
        Ok(id)
    }

    fn next_completion(&mut self) -> Option<TransferCompletion> {
        self.completions.pop()
    }

    fn cancel_all(&mut self) {
        for (_, id) in self.pending_reads.drain() {
            self.completions
                .push(TransferCompletion::Cancelled { id });
        }
        for (id, _) in self.pending_writes.drain() {
            self.completions
                .push(TransferCompletion::Cancelled { id });
        }
        self.write_log.clear();
    }

    fn is_attached(&self) -> bool {
        self.attached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_rx_endpoints_can_be_pending_at_the_same_time() {
        let mut backend = TestBackend::default();
        backend.arm_read(RxEndpoint::Event).unwrap();
        backend.arm_read(RxEndpoint::Acl).unwrap();
        assert!(backend.pending_read(RxEndpoint::Event));
        assert!(backend.pending_read(RxEndpoint::Acl));

        backend
            .inject_read(RxEndpoint::Acl, vec![1, 2, 3])
            .unwrap();
        assert!(backend.pending_read(RxEndpoint::Event));
        assert!(!backend.pending_read(RxEndpoint::Acl));

        assert!(matches!(
            backend.next_completion(),
            Some(TransferCompletion::Read {
                endpoint: RxEndpoint::Acl,
                ..
            })
        ));
    }

    #[test]
    fn read_completion_order_is_injection_order_not_endpoint_order() {
        let mut backend = TestBackend::default();
        backend.arm_read(RxEndpoint::Event).unwrap();
        backend.arm_read(RxEndpoint::Acl).unwrap();
        backend.inject_read(RxEndpoint::Acl, vec![0xaa]).unwrap();
        backend.inject_read(RxEndpoint::Event, vec![0xbb]).unwrap();

        assert!(matches!(
            backend.next_completion(),
            Some(TransferCompletion::Read {
                endpoint: RxEndpoint::Acl,
                ..
            })
        ));
        assert!(matches!(
            backend.next_completion(),
            Some(TransferCompletion::Read {
                endpoint: RxEndpoint::Event,
                ..
            })
        ));
    }

    #[test]
    fn cancel_all_generates_explicit_completions() {
        let mut backend = TestBackend::default();
        backend.arm_read(RxEndpoint::Event).unwrap();
        backend
            .submit_write(TxEndpoint::Iso, vec![1, 2, 3])
            .unwrap();
        backend.cancel_all();

        let first = backend.next_completion().unwrap();
        let second = backend.next_completion().unwrap();
        assert!(matches!(first, TransferCompletion::Cancelled { .. }));
        assert!(matches!(second, TransferCompletion::Cancelled { .. }));
        assert!(backend.next_completion().is_none());
    }

    #[test]
    fn detach_is_explicit_and_rejects_new_io() {
        let mut backend = TestBackend::default();
        backend.arm_read(RxEndpoint::Event).unwrap();
        backend.detach();
        assert_eq!(
            backend.arm_read(RxEndpoint::Acl),
            Err(BackendError::DeviceGone)
        );
        assert_eq!(
            backend.next_completion(),
            Some(TransferCompletion::DeviceGone)
        );
    }
}
