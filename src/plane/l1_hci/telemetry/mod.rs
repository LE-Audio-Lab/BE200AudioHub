//! Producer-local HCI observability.
//!
//! These diagnostics are not protocol truth. Controller credit/accounting
//! remains authoritative in the ISO/ACL credit owners. This module mirrors
//! successful mutations into bounded counters and an in-memory raw trace.

pub mod accumulator;
pub mod observation;
pub mod trace;

use accumulator::HciIsoTelemetryAccumulator;
pub use observation::HciIsoTelemetrySnapshot;
pub use trace::HciIsoTraceRecord;
use trace::HciIsoTrace;

#[derive(Default)]
pub struct HciTelemetry {
    iso: HciIsoTelemetryAccumulator,
    trace: HciIsoTrace,
}

impl HciTelemetry {
    pub fn observe_iso_configured(&mut self, limit: usize, in_flight: usize) {
        self.iso.observe_configured(limit, in_flight);
        self.trace.push(HciIsoTraceRecord::Configured { limit });
    }

    pub fn observe_iso_submitted(&mut self, cis_handle: u16, in_flight: usize) {
        self.iso.observe_submitted(in_flight);
        self.trace.push(HciIsoTraceRecord::Submitted {
            cis_handle,
            in_flight,
        });
    }

    pub fn observe_iso_completed(
        &mut self,
        cis_handle: u16,
        count: usize,
        in_flight: usize,
    ) {
        if count == 0 {
            return;
        }
        self.iso.observe_completed(count, in_flight);
        self.trace.push(HciIsoTraceRecord::Completed {
            cis_handle,
            count,
            in_flight,
        });
    }

    pub fn observe_iso_retired(
        &mut self,
        cis_handle: Option<u16>,
        count: usize,
        in_flight: usize,
    ) {
        if count == 0 {
            return;
        }
        self.iso.observe_retired(count, in_flight);
        self.trace.push(HciIsoTraceRecord::Retired {
            cis_handle,
            count,
            in_flight,
        });
    }

    pub fn iso_snapshot(&self) -> HciIsoTelemetrySnapshot {
        HciIsoTelemetrySnapshot {
            controller_limit: self.iso.controller_limit(),
            submitted_packets: self.iso.submitted_packets(),
            completed_packets: self.iso.completed_packets(),
            retired_packets: self.iso.retired_packets(),
            in_flight_packets: self.iso.in_flight_packets(),
            peak_in_flight_packets: self.iso.peak_in_flight_packets(),
            trace: self.trace.stats(),
        }
    }

    pub fn iso_trace_snapshot(&self) -> Vec<HciIsoTraceRecord> {
        self.trace.snapshot()
    }

    pub fn take_iso_dirty(&mut self) -> bool {
        self.iso.take_dirty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_aggregate_only_and_trace_loss_is_explicit() {
        let mut telemetry = HciTelemetry {
            iso: HciIsoTelemetryAccumulator::default(),
            trace: HciIsoTrace::with_capacity(1),
        };
        telemetry.observe_iso_configured(8, 0);
        telemetry.observe_iso_submitted(0x0900, 1);

        let snapshot = telemetry.iso_snapshot();
        assert_eq!(snapshot.controller_limit, Some(8));
        assert_eq!(snapshot.submitted_packets, 1);
        assert_eq!(snapshot.in_flight_packets, 1);
        assert_eq!(snapshot.trace.len, 1);
        assert_eq!(snapshot.trace.overwritten_oldest, 1);
    }
}
