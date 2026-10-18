//! Generic bounded telemetry/raw-trace ring.
//!
//! This is diagnostics infrastructure only. It must never carry reliable
//! state-changing protocol events. Overflow is explicitly lossy and never
//! blocks the producer or writes to persistent storage.

use std::collections::{vec_deque::Iter, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelemetryOverflowPolicy {
    /// Preserve the existing buffered history and discard the arriving record.
    DropNewest,
    /// Preserve the most recent history by evicting exactly one oldest record.
    OverwriteOldest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelemetryPushOutcome {
    Stored,
    DroppedNewest,
    OverwroteOldest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelemetryRingError {
    ZeroCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TelemetryRingStats {
    pub capacity: usize,
    pub len: usize,
    /// Number of records retained by a push at the time it was accepted.
    pub stored: u64,
    /// Number of arriving records discarded by DropNewest.
    pub dropped_newest: u64,
    /// Number of old records evicted by OverwriteOldest.
    pub overwritten_oldest: u64,
}

pub struct TelemetryRing<T> {
    capacity: usize,
    policy: TelemetryOverflowPolicy,
    records: VecDeque<T>,
    stored: u64,
    dropped_newest: u64,
    overwritten_oldest: u64,
}

impl<T> TelemetryRing<T> {
    pub fn new(
        capacity: usize,
        policy: TelemetryOverflowPolicy,
    ) -> Result<Self, TelemetryRingError> {
        if capacity == 0 {
            return Err(TelemetryRingError::ZeroCapacity);
        }
        Ok(Self {
            capacity,
            policy,
            records: VecDeque::with_capacity(capacity),
            stored: 0,
            dropped_newest: 0,
            overwritten_oldest: 0,
        })
    }

    pub fn push(&mut self, record: T) -> TelemetryPushOutcome {
        if self.records.len() < self.capacity {
            self.records.push_back(record);
            self.stored = self.stored.saturating_add(1);
            return TelemetryPushOutcome::Stored;
        }

        match self.policy {
            TelemetryOverflowPolicy::DropNewest => {
                self.dropped_newest =
                    self.dropped_newest.saturating_add(1);
                TelemetryPushOutcome::DroppedNewest
            }
            TelemetryOverflowPolicy::OverwriteOldest => {
                let _ = self.records.pop_front();
                self.records.push_back(record);
                self.stored = self.stored.saturating_add(1);
                self.overwritten_oldest =
                    self.overwritten_oldest.saturating_add(1);
                TelemetryPushOutcome::OverwroteOldest
            }
        }
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn iter(&self) -> Iter<'_, T> {
        self.records.iter()
    }

    pub fn front(&self) -> Option<&T> {
        self.records.front()
    }

    pub fn back(&self) -> Option<&T> {
        self.records.back()
    }

    /// Drop all buffered records while preserving lifetime loss counters.
    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn stats(&self) -> TelemetryRingStats {
        TelemetryRingStats {
            capacity: self.capacity,
            len: self.records.len(),
            stored: self.stored,
            dropped_newest: self.dropped_newest,
            overwritten_oldest: self.overwritten_oldest,
        }
    }

    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.records.drain(..)
    }
}

impl<T: Clone> TelemetryRing<T> {
    /// Non-destructive value snapshot for debug/UI consumers.
    pub fn snapshot(&self) -> Vec<T> {
        self.records.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_capacity_is_rejected() {
        assert!(matches!(
            TelemetryRing::<u32>::new(
                0,
                TelemetryOverflowPolicy::DropNewest,
            ),
            Err(TelemetryRingError::ZeroCapacity)
        ));
    }

    #[test]
    fn drop_newest_preserves_existing_history_and_counts_loss() {
        let mut ring = TelemetryRing::new(
            2,
            TelemetryOverflowPolicy::DropNewest,
        )
        .unwrap();

        assert_eq!(ring.push(10), TelemetryPushOutcome::Stored);
        assert_eq!(ring.push(20), TelemetryPushOutcome::Stored);
        assert_eq!(
            ring.push(30),
            TelemetryPushOutcome::DroppedNewest
        );
        assert_eq!(ring.snapshot(), vec![10, 20]);
        assert_eq!(
            ring.stats(),
            TelemetryRingStats {
                capacity: 2,
                len: 2,
                stored: 2,
                dropped_newest: 1,
                overwritten_oldest: 0,
            }
        );
    }

    #[test]
    fn overwrite_oldest_keeps_recent_history_and_counts_eviction() {
        let mut ring = TelemetryRing::new(
            2,
            TelemetryOverflowPolicy::OverwriteOldest,
        )
        .unwrap();

        ring.push(10);
        ring.push(20);
        assert_eq!(
            ring.push(30),
            TelemetryPushOutcome::OverwroteOldest
        );
        assert_eq!(ring.snapshot(), vec![20, 30]);
        assert_eq!(ring.front(), Some(&20));
        assert_eq!(ring.back(), Some(&30));
        assert_eq!(
            ring.stats(),
            TelemetryRingStats {
                capacity: 2,
                len: 2,
                stored: 3,
                dropped_newest: 0,
                overwritten_oldest: 1,
            }
        );
    }

    #[test]
    fn clear_drops_buffered_records_but_preserves_loss_history() {
        let mut ring = TelemetryRing::new(
            1,
            TelemetryOverflowPolicy::DropNewest,
        )
        .unwrap();
        ring.push(1);
        ring.push(2);
        ring.clear();

        assert!(ring.is_empty());
        assert_eq!(ring.stats().dropped_newest, 1);
        assert_eq!(ring.stats().stored, 1);
    }

    #[test]
    fn drain_preserves_fifo_order() {
        let mut ring = TelemetryRing::new(
            3,
            TelemetryOverflowPolicy::OverwriteOldest,
        )
        .unwrap();
        ring.push(1);
        ring.push(2);
        ring.push(3);

        assert_eq!(ring.drain().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(ring.is_empty());
    }
}
