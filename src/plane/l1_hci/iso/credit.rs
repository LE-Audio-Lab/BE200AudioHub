use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoCreditFault {
    ZeroCapacity,
    NotConfigured,
    ReconfigureWithPending { pending: usize },
    PoolExceeded {
        limit: usize,
        in_flight: usize,
    },
    UnknownCompletion {
        cis_handle: u16,
        completed: usize,
    },
    CompletionOverflow {
        cis_handle: u16,
        completed: usize,
        in_flight: usize,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IsoCompletionAccounting {
    pub live_completed: usize,
    pub retired_completed: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IsoCreditAudit {
    limit: Option<usize>,
    total_in_flight: usize,
    per_handle: BTreeMap<u16, usize>,
    retired_completion_allowance_per_handle: BTreeMap<u16, usize>,
    total_submitted: u64,
    total_completed: u64,
    submitted_per_handle: BTreeMap<u16, u64>,
    completed_per_handle: BTreeMap<u16, u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IsoCreditSnapshot {
    pub limit: Option<usize>,
    pub in_flight: usize,
    pub total_submitted: u64,
    pub total_completed: u64,
    pub in_flight_per_handle: BTreeMap<u16, usize>,
    pub submitted_per_handle: BTreeMap<u16, u64>,
    pub completed_per_handle: BTreeMap<u16, u64>,
}

impl IsoCreditAudit {
    pub fn configure(&mut self, limit: usize) -> Result<(), IsoCreditFault> {
        if limit == 0 {
            return Err(IsoCreditFault::ZeroCapacity);
        }
        if self.total_in_flight != 0 {
            return Err(IsoCreditFault::ReconfigureWithPending {
                pending: self.total_in_flight,
            });
        }
        self.limit = Some(limit);
        self.per_handle.clear();
        self.retired_completion_allowance_per_handle.clear();
        self.total_submitted = 0;
        self.total_completed = 0;
        self.submitted_per_handle.clear();
        self.completed_per_handle.clear();
        Ok(())
    }

    pub fn submitted(&mut self, cis_handle: u16) -> Result<(), IsoCreditFault> {
        let limit = self.limit.ok_or(IsoCreditFault::NotConfigured)?;
        if self.total_in_flight >= limit {
            return Err(IsoCreditFault::PoolExceeded {
                limit,
                in_flight: self.total_in_flight,
            });
        }

        self.total_in_flight += 1;
        *self.per_handle.entry(cis_handle).or_default() += 1;
        self.total_submitted = self.total_submitted.saturating_add(1);
        *self
            .submitted_per_handle
            .entry(cis_handle)
            .or_default() += 1;
        Ok(())
    }

    pub fn completed(
        &mut self,
        cis_handle: u16,
        completed: usize,
    ) -> Result<(), IsoCreditFault> {
        self.completed_accounting(cis_handle, completed)
            .map(|_| ())
    }

    pub fn completed_accounting(
        &mut self,
        cis_handle: u16,
        completed: usize,
    ) -> Result<IsoCompletionAccounting, IsoCreditFault> {
        if completed == 0 {
            return Ok(IsoCompletionAccounting::default());
        }

        let live_available =
            self.per_handle.get(&cis_handle).copied().unwrap_or(0);
        let retired_available = self
            .retired_completion_allowance_per_handle
            .get(&cis_handle)
            .copied()
            .unwrap_or(0);
        let known_available =
            live_available.saturating_add(retired_available);

        if known_available == 0 {
            return Err(IsoCreditFault::UnknownCompletion {
                cis_handle,
                completed,
            });
        }
        if completed > known_available {
            return Err(IsoCreditFault::CompletionOverflow {
                cis_handle,
                completed,
                in_flight: known_available,
            });
        }

        let live_completed = completed.min(live_available);
        let retired_completed = completed - live_completed;

        if live_completed != 0 {
            let in_flight = self
                .per_handle
                .get_mut(&cis_handle)
                .expect("live availability requires per-handle credit");
            *in_flight -= live_completed;
            self.total_in_flight -= live_completed;
            self.total_completed = self
                .total_completed
                .saturating_add(live_completed as u64);
            *self
                .completed_per_handle
                .entry(cis_handle)
                .or_default() += live_completed as u64;
            if *in_flight == 0 {
                self.per_handle.remove(&cis_handle);
            }
        }

        if retired_completed != 0 {
            let allowance = self
                .retired_completion_allowance_per_handle
                .get_mut(&cis_handle)
                .expect("retired availability requires allowance");
            *allowance -= retired_completed;
            if *allowance == 0 {
                self.retired_completion_allowance_per_handle
                    .remove(&cis_handle);
            }
        }

        Ok(IsoCompletionAccounting {
            live_completed,
            retired_completed,
        })
    }

    /// Teardown-only accounting cleanup. Retired packets are not completion
    /// evidence and must never be reported as successfully delivered.
    pub fn flush(&mut self, cis_handle: u16) -> usize {
        let retired = self.per_handle.remove(&cis_handle).unwrap_or(0);
        self.total_in_flight -= retired;
        if retired != 0 {
            *self
                .retired_completion_allowance_per_handle
                .entry(cis_handle)
                .or_default() += retired;
        }
        retired
    }

    pub fn flush_all(&mut self) -> usize {
        let retired = self.total_in_flight;
        for (cis_handle, count) in &self.per_handle {
            *self
                .retired_completion_allowance_per_handle
                .entry(*cis_handle)
                .or_default() += *count;
        }
        self.per_handle.clear();
        self.total_in_flight = 0;
        retired
    }

    pub fn retired_completion_allowance_on(
        &self,
        cis_handle: u16,
    ) -> usize {
        self.retired_completion_allowance_per_handle
            .get(&cis_handle)
            .copied()
            .unwrap_or(0)
    }

    pub fn in_flight(&self) -> usize {
        self.total_in_flight
    }

    pub fn in_flight_on(&self, cis_handle: u16) -> usize {
        self.per_handle.get(&cis_handle).copied().unwrap_or(0)
    }

    pub fn snapshot(&self) -> IsoCreditSnapshot {
        IsoCreditSnapshot {
            limit: self.limit,
            in_flight: self.total_in_flight,
            total_submitted: self.total_submitted,
            total_completed: self.total_completed,
            in_flight_per_handle: self.per_handle.clone(),
            submitted_per_handle: self.submitted_per_handle.clone(),
            completed_per_handle: self.completed_per_handle.clone(),
        }
    }

    pub fn limit(&self) -> Option<usize> {
        self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_release_only_the_matching_cis_capacity() {
        let mut audit = IsoCreditAudit::default();
        audit.configure(3).unwrap();
        audit.submitted(0x0900).unwrap();
        audit.submitted(0x0901).unwrap();
        audit.submitted(0x0901).unwrap();

        audit.completed(0x0901, 1).unwrap();
        assert_eq!(audit.in_flight(), 2);
        assert_eq!(audit.in_flight_on(0x0900), 1);
        assert_eq!(audit.in_flight_on(0x0901), 1);
    }

    #[test]
    fn over_completion_and_unknown_cis_are_strict_faults() {
        let mut audit = IsoCreditAudit::default();
        audit.configure(2).unwrap();
        audit.submitted(0x0900).unwrap();

        assert_eq!(
            audit.completed(0x0900, 2),
            Err(IsoCreditFault::CompletionOverflow {
                cis_handle: 0x0900,
                completed: 2,
                in_flight: 1,
            })
        );
        assert_eq!(
            audit.completed(0x0901, 1),
            Err(IsoCreditFault::UnknownCompletion {
                cis_handle: 0x0901,
                completed: 1,
            })
        );
        assert_eq!(audit.in_flight(), 1);
    }

    #[test]
    fn controller_pool_limit_is_global_across_cis_handles() {
        let mut audit = IsoCreditAudit::default();
        assert_eq!(
            audit.submitted(0x0900),
            Err(IsoCreditFault::NotConfigured)
        );
        audit.configure(2).unwrap();
        audit.submitted(0x0900).unwrap();
        audit.submitted(0x0901).unwrap();
        assert_eq!(
            audit.submitted(0x0900),
            Err(IsoCreditFault::PoolExceeded {
                limit: 2,
                in_flight: 2,
            })
        );
    }

    #[test]
    fn snapshot_tracks_submitted_completed_and_not_flushed_as_completed() {
        let mut audit = IsoCreditAudit::default();
        audit.configure(4).unwrap();
        audit.submitted(0x0900).unwrap();
        audit.submitted(0x0901).unwrap();
        audit.completed(0x0900, 1).unwrap();
        audit.flush(0x0901);

        let snapshot = audit.snapshot();
        assert_eq!(snapshot.total_submitted, 2);
        assert_eq!(snapshot.total_completed, 1);
        assert_eq!(snapshot.in_flight, 0);
        assert_eq!(
            snapshot.submitted_per_handle.get(&0x0901),
            Some(&1)
        );
        assert_eq!(
            snapshot.completed_per_handle.get(&0x0901),
            None
        );
    }

    #[test]
    fn late_completion_after_retire_consumes_only_retired_allowance() {
        let mut audit = IsoCreditAudit::default();
        audit.configure(4).unwrap();
        audit.submitted(0x0900).unwrap();

        assert_eq!(audit.flush(0x0900), 1);
        assert_eq!(audit.in_flight(), 0);
        assert_eq!(audit.retired_completion_allowance_on(0x0900), 1);

        let accounting =
            audit.completed_accounting(0x0900, 1).unwrap();
        assert_eq!(
            accounting,
            IsoCompletionAccounting {
                live_completed: 0,
                retired_completed: 1,
            }
        );
        assert_eq!(audit.total_completed, 0);
        assert_eq!(audit.retired_completion_allowance_on(0x0900), 0);
        assert_eq!(
            audit.completed(0x0900, 1),
            Err(IsoCreditFault::UnknownCompletion {
                cis_handle: 0x0900,
                completed: 1,
            })
        );
    }

    #[test]
    fn disconnect_flush_is_handle_local_and_not_completion() {
        let mut audit = IsoCreditAudit::default();
        audit.configure(4).unwrap();
        audit.submitted(0x0900).unwrap();
        audit.submitted(0x0900).unwrap();
        audit.submitted(0x0901).unwrap();

        assert_eq!(audit.flush(0x0900), 2);
        assert_eq!(audit.in_flight(), 1);
        assert_eq!(audit.in_flight_on(0x0900), 0);
        assert_eq!(audit.in_flight_on(0x0901), 1);
    }
}
