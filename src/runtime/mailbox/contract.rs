//! Generic runtime delivery and overflow semantics.
//!
//! Product/domain-specific message scope and media policies do not belong here.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryClass {
    ReliableBounded,
    CoalescedLatest,
    RealtimeBounded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowAction {
    Fault,
    DropOldestAndMarkDiscontinuity,
    DropStaleAndMarkMiss,
    CoalesceLatest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MailboxContract {
    pub delivery: DeliveryClass,
    pub overflow: OverflowAction,
    pub producer_must_not_block: bool,
}

pub const RELIABLE_MAILBOX: MailboxContract = MailboxContract {
    delivery: DeliveryClass::ReliableBounded,
    overflow: OverflowAction::Fault,
    producer_must_not_block: true,
};

pub const TELEMETRY_MAILBOX: MailboxContract = MailboxContract {
    delivery: DeliveryClass::CoalescedLatest,
    overflow: OverflowAction::CoalesceLatest,
    producer_must_not_block: true,
};
