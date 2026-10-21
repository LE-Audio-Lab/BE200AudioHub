//! Producer-local aggregate counters for L2 Transport.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportTelemetryAccumulator {
    rx_packets: u64,
    tx_packets: u64,
    faults: u64,
    dirty: bool,
}

impl TransportTelemetryAccumulator {
    pub fn observe_rx_packets(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.rx_packets = self.rx_packets.saturating_add(count as u64);
        self.dirty = true;
    }

    pub fn observe_tx_packet(&mut self) {
        self.tx_packets = self.tx_packets.saturating_add(1);
        self.dirty = true;
    }

    pub fn observe_fault(&mut self) {
        self.faults = self.faults.saturating_add(1);
        self.dirty = true;
    }

    pub const fn rx_packets(&self) -> u64 { self.rx_packets }
    pub const fn tx_packets(&self) -> u64 { self.tx_packets }
    pub const fn faults(&self) -> u64 { self.faults }

    pub fn take_dirty(&mut self) -> bool {
        let dirty = self.dirty;
        self.dirty = false;
        dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_coalesce_without_resetting_transport_metrics() {
        let mut value = TransportTelemetryAccumulator::default();
        value.observe_rx_packets(2);
        value.observe_tx_packet();
        value.observe_fault();

        assert_eq!(value.rx_packets(), 2);
        assert_eq!(value.tx_packets(), 1);
        assert_eq!(value.faults(), 1);
        assert!(value.take_dirty());
        assert!(!value.take_dirty());
        assert_eq!(value.rx_packets(), 2);
    }
}
