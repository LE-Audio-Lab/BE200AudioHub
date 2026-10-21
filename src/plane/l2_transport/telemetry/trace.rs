use crate::{
    platform::backend::{
        BackendError, RxEndpoint, TransferId, TxEndpoint,
    },
    plane::l2_transport::packet::reassembly::ReassemblyError,
    runtime::telemetry::{
        telemetry_ring, TelemetryRing, TelemetryRingStats,
    },
};

pub const DEFAULT_TRANSPORT_TRACE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportTraceFault {
    Backend(BackendError),
    Reassembly(ReassemblyError),
    HciInboxFull,
    HciInboxDisconnected,
    TransferFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportTraceRecord {
    TxSubmitted {
        id: TransferId,
        endpoint: TxEndpoint,
        bytes: usize,
    },
    ReadCompleted {
        id: TransferId,
        endpoint: RxEndpoint,
        bytes: usize,
        packets: usize,
    },
    WriteCompleted {
        id: TransferId,
        endpoint: TxEndpoint,
        bytes: usize,
    },
    Cancelled {
        id: TransferId,
    },
    DeviceGone,
    TransferFailed {
        id: Option<TransferId>,
    },
    Fault {
        kind: TransportTraceFault,
    },
}

pub struct TransportTrace {
    ring: TelemetryRing<TransportTraceRecord>,
}

impl Default for TransportTrace {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_TRANSPORT_TRACE_CAPACITY)
    }
}

impl TransportTrace {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            ring: telemetry_ring(capacity)
                .expect("Transport trace capacity must be non-zero"),
        }
    }

    pub fn push(&mut self, record: TransportTraceRecord) {
        let _ = self.ring.push(record);
    }

    pub fn stats(&self) -> TelemetryRingStats {
        self.ring.stats()
    }

    pub fn snapshot(&self) -> Vec<TransportTraceRecord> {
        self.ring.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_trace_is_bounded_and_preserves_recent_history() {
        let mut trace = TransportTrace::with_capacity(2);
        trace.push(TransportTraceRecord::DeviceGone);
        trace.push(TransportTraceRecord::Cancelled {
            id: TransferId(1),
        });
        trace.push(TransportTraceRecord::TransferFailed {
            id: Some(TransferId(2)),
        });

        assert_eq!(trace.snapshot().len(), 2);
        assert!(matches!(
            trace.snapshot()[0],
            TransportTraceRecord::Cancelled { .. }
        ));
        assert_eq!(trace.stats().overwritten_oldest, 1);
    }

    #[test]
    fn default_trace_capacity_matches_plane_policy() {
        let trace = TransportTrace::default();
        assert_eq!(
            trace.stats().capacity,
            DEFAULT_TRANSPORT_TRACE_CAPACITY
        );
    }
}
