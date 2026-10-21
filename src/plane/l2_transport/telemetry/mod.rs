//! Producer-local L2-Transport observability.
//!
//! Aggregate counters are coalesced locally. Raw USB/backend history remains
//! in a bounded recent-history ring and is never protocol truth.

pub mod accumulator;
pub mod observation;
pub mod trace;

use accumulator::TransportTelemetryAccumulator;
pub use observation::TransportTelemetrySnapshot;
pub use trace::{TransportTraceFault, TransportTraceRecord};
use trace::TransportTrace;

use crate::platform::backend::{
    RxEndpoint, TransferId, TxEndpoint,
};

#[derive(Default)]
pub struct TransportTelemetry {
    aggregate: TransportTelemetryAccumulator,
    trace: TransportTrace,
}

impl TransportTelemetry {
    pub fn observe_tx_submitted(
        &mut self,
        id: TransferId,
        endpoint: TxEndpoint,
        bytes: usize,
    ) {
        self.aggregate.observe_tx_packet();
        self.trace.push(TransportTraceRecord::TxSubmitted {
            id,
            endpoint,
            bytes,
        });
    }

    pub fn observe_read_completed(
        &mut self,
        id: TransferId,
        endpoint: RxEndpoint,
        bytes: usize,
        packets: usize,
    ) {
        self.trace.push(TransportTraceRecord::ReadCompleted {
            id,
            endpoint,
            bytes,
            packets,
        });
    }

    pub fn observe_rx_delivered(&mut self, count: usize) {
        self.aggregate.observe_rx_packets(count);
    }

    pub fn observe_write_completed(
        &mut self,
        id: TransferId,
        endpoint: TxEndpoint,
        bytes: usize,
    ) {
        self.trace.push(TransportTraceRecord::WriteCompleted {
            id,
            endpoint,
            bytes,
        });
    }

    pub fn observe_cancelled(&mut self, id: TransferId) {
        self.trace.push(TransportTraceRecord::Cancelled { id });
    }

    pub fn observe_device_gone(&mut self) {
        self.trace.push(TransportTraceRecord::DeviceGone);
    }

    pub fn observe_transfer_failed_completion(
        &mut self,
        id: Option<TransferId>,
    ) {
        self.trace.push(TransportTraceRecord::TransferFailed { id });
    }

    pub fn observe_fault(&mut self, kind: TransportTraceFault) {
        self.aggregate.observe_fault();
        self.trace.push(TransportTraceRecord::Fault { kind });
    }

    pub fn snapshot(&self) -> TransportTelemetrySnapshot {
        TransportTelemetrySnapshot {
            rx_packets: self.aggregate.rx_packets(),
            tx_packets: self.aggregate.tx_packets(),
            faults: self.aggregate.faults(),
            trace: self.trace.stats(),
        }
    }

    pub fn trace_snapshot(&self) -> Vec<TransportTraceRecord> {
        self.trace.snapshot()
    }

    pub fn take_dirty(&mut self) -> bool {
        self.aggregate.take_dirty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::backend::{BackendError, TransferId};

    #[test]
    fn aggregate_and_raw_lanes_remain_distinct() {
        let mut telemetry = TransportTelemetry::default();

        telemetry.observe_tx_submitted(
            TransferId(1),
            TxEndpoint::Iso,
            100,
        );
        telemetry.observe_read_completed(
            TransferId(2),
            RxEndpoint::Event,
            7,
            2,
        );
        telemetry.observe_rx_delivered(2);
        telemetry.observe_fault(
            TransportTraceFault::Backend(BackendError::Failed),
        );

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.tx_packets, 1);
        assert_eq!(snapshot.rx_packets, 2);
        assert_eq!(snapshot.faults, 1);
        assert_eq!(snapshot.trace.len, 3);
        assert!(telemetry.take_dirty());
        assert!(!telemetry.take_dirty());
    }
}
