use crate::runtime::telemetry::{
    telemetry_ring, TelemetryRing, TelemetryRingStats,
};

pub const DEFAULT_HCI_ISO_TRACE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciIsoTraceRecord {
    Configured {
        limit: usize,
    },
    Submitted {
        cis_handle: u16,
        in_flight: usize,
    },
    Completed {
        cis_handle: u16,
        count: usize,
        in_flight: usize,
    },
    Retired {
        cis_handle: Option<u16>,
        count: usize,
        in_flight: usize,
    },
}

pub struct HciIsoTrace {
    ring: TelemetryRing<HciIsoTraceRecord>,
}

impl Default for HciIsoTrace {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_HCI_ISO_TRACE_CAPACITY)
    }
}

impl HciIsoTrace {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            ring: telemetry_ring(capacity)
                .expect("HCI ISO trace capacity must be non-zero"),
        }
    }

    pub fn push(&mut self, record: HciIsoTraceRecord) {
        let _ = self.ring.push(record);
    }

    pub fn stats(&self) -> TelemetryRingStats {
        self.ring.stats()
    }

    pub fn snapshot(&self) -> Vec<HciIsoTraceRecord> {
        self.ring.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_trace_is_bounded_and_preserves_recent_history() {
        let mut trace = HciIsoTrace::with_capacity(2);
        trace.push(HciIsoTraceRecord::Configured { limit: 8 });
        trace.push(HciIsoTraceRecord::Submitted {
            cis_handle: 0x0900,
            in_flight: 1,
        });
        trace.push(HciIsoTraceRecord::Completed {
            cis_handle: 0x0900,
            count: 1,
            in_flight: 0,
        });

        assert_eq!(trace.snapshot().len(), 2);
        assert!(matches!(
            trace.snapshot()[0],
            HciIsoTraceRecord::Submitted { .. }
        ));
        assert_eq!(trace.stats().overwritten_oldest, 1);
    }
}
