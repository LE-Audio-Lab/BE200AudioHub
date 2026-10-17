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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IsoCreditAudit {
    limit: Option<usize>,
    total_in_flight: usize,
    per_handle: BTreeMap<u16, usize>,
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
        Ok(())
    }

    pub fn completed(
        &mut self,
        cis_handle: u16,
        completed: usize,
    ) -> Result<(), IsoCreditFault> {
        if completed == 0 {
            return Ok(());
        }

        let Some(in_flight) = self.per_handle.get_mut(&cis_handle) else {
            return Err(IsoCreditFault::UnknownCompletion {
                cis_handle,
                completed,
            });
        };
        if completed > *in_flight {
            return Err(IsoCreditFault::CompletionOverflow {
                cis_handle,
                completed,
                in_flight: *in_flight,
            });
        }

        *in_flight -= completed;
        self.total_in_flight -= completed;
        if *in_flight == 0 {
            self.per_handle.remove(&cis_handle);
        }
        Ok(())
    }

    /// Teardown-only accounting cleanup. Retired packets are not completion
    /// evidence and must never be reported as successfully delivered.
    pub fn flush(&mut self, cis_handle: u16) -> usize {
        let retired = self.per_handle.remove(&cis_handle).unwrap_or(0);
        self.total_in_flight -= retired;
        retired
    }

    pub fn flush_all(&mut self) -> usize {
        let retired = self.total_in_flight;
        self.per_handle.clear();
        self.total_in_flight = 0;
        retired
    }

    pub fn in_flight(&self) -> usize {
        self.total_in_flight
    }

    pub fn in_flight_on(&self, cis_handle: u16) -> usize {
        self.per_handle.get(&cis_handle).copied().unwrap_or(0)
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
