use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AclCreditFault {
    ZeroCapacity,
    NotConfigured,
    ReconfigureWithPending { pending: usize },
    PoolExceeded {
        limit: usize,
        in_flight: usize,
    },
    UnknownCompletion {
        connection_handle: u16,
        completed: usize,
    },
    CompletionOverflow {
        connection_handle: u16,
        completed: usize,
        in_flight: usize,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AclAttWriteSignature {
    pub connection_handle: u16,
    pub attribute_handle: u16,
    pub value_len: u16,
    pub value_prefix_len: u8,
    pub value_prefix: [u8; 8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AclCreditSnapshot {
    pub bumble_pending: usize,
    pub audit_in_flight: usize,
    pub audit_limit: Option<usize>,
    /// Controller->Host HCI ACL packets accepted by QueueHostTransport.
    pub controller_acl_rx_packets: u64,
    /// ATT PDUs drained from Bumble's per-connection inbox.
    pub att_pdus_drained: u64,
    /// ATT Handle Value Notification/Indication PDUs surfaced as GATT values.
    pub gatt_value_events: u64,
    pub audit_total_submitted: u64,
    pub audit_total_completed: u64,
    pub audit_total_retired: u64,
    pub transport_mailbox_enqueued: u64,
    pub transport_usb_submitted: u64,
    pub transport_usb_completed: u64,
    pub recent_att_writes: [Option<AclAttWriteSignature>; 4],
}

impl AclCreditSnapshot {
    pub fn waiting_estimate(self) -> usize {
        self.bumble_pending.saturating_sub(self.audit_in_flight)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AclCreditAudit {
    limit: Option<usize>,
    total_in_flight: usize,
    per_handle: BTreeMap<u16, usize>,
    total_submitted: u64,
    total_completed: u64,
    total_retired: u64,
}

impl AclCreditAudit {
    pub fn configure(&mut self, limit: usize) -> Result<(), AclCreditFault> {
        if limit == 0 {
            return Err(AclCreditFault::ZeroCapacity);
        }
        if self.total_in_flight != 0 {
            return Err(AclCreditFault::ReconfigureWithPending {
                pending: self.total_in_flight,
            });
        }
        self.limit = Some(limit);
        self.per_handle.clear();
        Ok(())
    }

    pub fn submitted(
        &mut self,
        connection_handle: u16,
    ) -> Result<(), AclCreditFault> {
        let limit = self.limit.ok_or(AclCreditFault::NotConfigured)?;
        if self.total_in_flight >= limit {
            return Err(AclCreditFault::PoolExceeded {
                limit,
                in_flight: self.total_in_flight,
            });
        }

        self.total_in_flight += 1;
        *self.per_handle.entry(connection_handle).or_default() += 1;
        self.total_submitted = self.total_submitted.saturating_add(1);
        Ok(())
    }

    pub fn completed(
        &mut self,
        connection_handle: u16,
        completed: usize,
    ) -> Result<(), AclCreditFault> {
        if completed == 0 {
            return Ok(());
        }

        let Some(in_flight) = self.per_handle.get_mut(&connection_handle) else {
            return Err(AclCreditFault::UnknownCompletion {
                connection_handle,
                completed,
            });
        };

        if completed > *in_flight {
            return Err(AclCreditFault::CompletionOverflow {
                connection_handle,
                completed,
                in_flight: *in_flight,
            });
        }

        *in_flight -= completed;
        self.total_in_flight -= completed;
        self.total_completed = self
            .total_completed
            .saturating_add(completed as u64);
        if *in_flight == 0 {
            self.per_handle.remove(&connection_handle);
        }
        Ok(())
    }

    /// Mirrors Bumble DataPacketQueue::flush(handle): queued/in-flight work for
    /// a disconnected handle is retired without borrowing capacity from peers.
    pub fn flush(&mut self, connection_handle: u16) -> usize {
        let retired = self.per_handle.remove(&connection_handle).unwrap_or(0);
        self.total_in_flight -= retired;
        self.total_retired = self
            .total_retired
            .saturating_add(retired as u64);
        retired
    }

    pub fn flush_all(&mut self) -> usize {
        let retired = self.total_in_flight;
        self.per_handle.clear();
        self.total_in_flight = 0;
        self.total_retired = self
            .total_retired
            .saturating_add(retired as u64);
        retired
    }

    pub fn in_flight(&self) -> usize {
        self.total_in_flight
    }

    pub fn in_flight_on(&self, connection_handle: u16) -> usize {
        self.per_handle
            .get(&connection_handle)
            .copied()
            .unwrap_or(0)
    }

    pub fn limit(&self) -> Option<usize> {
        self.limit
    }

    pub fn total_submitted(&self) -> u64 {
        self.total_submitted
    }

    pub fn total_completed(&self) -> u64 {
        self.total_completed
    }

    pub fn total_retired(&self) -> u64 {
        self.total_retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_handle_completions_release_only_the_matching_capacity() {
        let mut audit = AclCreditAudit::default();
        audit.configure(3).unwrap();
        audit.submitted(0x0800).unwrap();
        audit.submitted(0x0801).unwrap();
        audit.submitted(0x0801).unwrap();

        audit.completed(0x0801, 1).unwrap();
        assert_eq!(audit.in_flight(), 2);
        assert_eq!(audit.in_flight_on(0x0800), 1);
        assert_eq!(audit.in_flight_on(0x0801), 1);

        audit.completed(0x0800, 1).unwrap();
        assert_eq!(audit.in_flight(), 1);
        assert_eq!(audit.in_flight_on(0x0800), 0);
    }

    #[test]
    fn over_completion_and_unknown_completion_are_faults_without_mutation() {
        let mut audit = AclCreditAudit::default();
        audit.configure(2).unwrap();
        audit.submitted(0x0800).unwrap();

        assert_eq!(
            audit.completed(0x0800, 2),
            Err(AclCreditFault::CompletionOverflow {
                connection_handle: 0x0800,
                completed: 2,
                in_flight: 1,
            })
        );
        assert_eq!(audit.in_flight_on(0x0800), 1);

        assert_eq!(
            audit.completed(0x0801, 1),
            Err(AclCreditFault::UnknownCompletion {
                connection_handle: 0x0801,
                completed: 1,
            })
        );
        assert_eq!(audit.in_flight(), 1);
    }

    #[test]
    fn pool_limit_and_reconfigure_are_explicit() {
        let mut audit = AclCreditAudit::default();
        assert_eq!(
            audit.submitted(0x0800),
            Err(AclCreditFault::NotConfigured)
        );
        audit.configure(1).unwrap();
        audit.submitted(0x0800).unwrap();

        assert_eq!(
            audit.submitted(0x0801),
            Err(AclCreditFault::PoolExceeded {
                limit: 1,
                in_flight: 1,
            })
        );
        assert_eq!(
            audit.configure(2),
            Err(AclCreditFault::ReconfigureWithPending { pending: 1 })
        );
    }

    #[test]
    fn disconnect_flush_is_handle_local() {
        let mut audit = AclCreditAudit::default();
        audit.configure(4).unwrap();
        audit.submitted(0x0800).unwrap();
        audit.submitted(0x0800).unwrap();
        audit.submitted(0x0801).unwrap();

        assert_eq!(audit.flush(0x0800), 2);
        assert_eq!(audit.in_flight(), 1);
        assert_eq!(audit.in_flight_on(0x0800), 0);
        assert_eq!(audit.in_flight_on(0x0801), 1);
    }
}
