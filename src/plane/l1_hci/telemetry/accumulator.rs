#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HciIsoTelemetryAccumulator {
    controller_limit: Option<usize>,
    submitted_packets: u64,
    completed_packets: u64,
    retired_packets: u64,
    in_flight_packets: usize,
    peak_in_flight_packets: usize,
    dirty: bool,
}

impl HciIsoTelemetryAccumulator {
    pub fn observe_configured(&mut self, limit: usize, in_flight: usize) {
        self.controller_limit = Some(limit);
        self.in_flight_packets = in_flight;
        self.peak_in_flight_packets = self.peak_in_flight_packets.max(in_flight);
        self.dirty = true;
    }

    pub fn observe_submitted(&mut self, in_flight: usize) {
        self.submitted_packets = self.submitted_packets.saturating_add(1);
        self.in_flight_packets = in_flight;
        self.peak_in_flight_packets = self.peak_in_flight_packets.max(in_flight);
        self.dirty = true;
    }

    pub fn observe_completed(&mut self, count: usize, in_flight: usize) {
        if count == 0 {
            return;
        }
        self.completed_packets = self
            .completed_packets
            .saturating_add(count as u64);
        self.in_flight_packets = in_flight;
        self.dirty = true;
    }

    pub fn observe_retired(&mut self, count: usize, in_flight: usize) {
        if count == 0 {
            return;
        }
        self.retired_packets = self
            .retired_packets
            .saturating_add(count as u64);
        self.in_flight_packets = in_flight;
        self.dirty = true;
    }

    pub const fn controller_limit(&self) -> Option<usize> {
        self.controller_limit
    }

    pub const fn submitted_packets(&self) -> u64 {
        self.submitted_packets
    }

    pub const fn completed_packets(&self) -> u64 {
        self.completed_packets
    }

    pub const fn retired_packets(&self) -> u64 {
        self.retired_packets
    }

    pub const fn in_flight_packets(&self) -> usize {
        self.in_flight_packets
    }

    pub const fn peak_in_flight_packets(&self) -> usize {
        self.peak_in_flight_packets
    }

    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Publication bookkeeping only. This does not reset any accumulated
    /// metric and therefore cannot affect HCI/controller behavior.
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
    fn completion_and_teardown_retirement_are_distinct() {
        let mut value = HciIsoTelemetryAccumulator::default();
        value.observe_configured(8, 0);
        value.observe_submitted(1);
        value.observe_submitted(2);
        value.observe_completed(1, 1);
        value.observe_retired(1, 0);

        assert_eq!(value.controller_limit(), Some(8));
        assert_eq!(value.submitted_packets(), 2);
        assert_eq!(value.completed_packets(), 1);
        assert_eq!(value.retired_packets(), 1);
        assert_eq!(value.in_flight_packets(), 0);
        assert_eq!(value.peak_in_flight_packets(), 2);
    }

    #[test]
    fn dirty_is_coalesced_without_resetting_counters() {
        let mut value = HciIsoTelemetryAccumulator::default();
        assert!(!value.take_dirty());

        value.observe_submitted(1);
        value.observe_submitted(2);
        assert!(value.take_dirty());
        assert!(!value.take_dirty());
        assert_eq!(value.submitted_packets(), 2);
        assert_eq!(value.in_flight_packets(), 2);
    }
}
