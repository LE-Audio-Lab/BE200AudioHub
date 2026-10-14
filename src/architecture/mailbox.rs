//! Cross-context mailbox semantics.
//!
//! This module deliberately does not select a Rust channel crate. It freezes the
//! delivery/backpressure semantics that an implementation must preserve.

use super::types::{Generation, SessionId, StreamId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageScope {
    pub generation: Generation,
    pub session_id: Option<SessionId>,
    pub stream_id: Option<StreamId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope<T> {
    pub scope: MessageScope,
    pub payload: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryClass {
    /// State-changing commands/reports and HCI packets. Loss is a fault.
    ReliableBounded,
    /// High-rate observation where a newer value supersedes an older one.
    CoalescedLatest,
    /// Realtime media data. Producer never blocks; overflow follows a defined
    /// latency-bounding policy and records a discontinuity/miss.
    RealtimeBounded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowAction {
    /// Queue exhaustion means subsystem state can no longer be trusted.
    Fault,
    /// Keep latency bounded by discarding the oldest source-domain data and
    /// explicitly recording a discontinuity.
    DropOldestAndMarkDiscontinuity,
    /// Drop an already-stale target-domain frame rather than increasing latency.
    DropStaleAndMarkMiss,
    /// Replace an older observation with the latest snapshot.
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

pub const SOURCE_PCM_RING: MailboxContract = MailboxContract {
    delivery: DeliveryClass::RealtimeBounded,
    overflow: OverflowAction::DropOldestAndMarkDiscontinuity,
    producer_must_not_block: true,
};

pub const TARGET_FRAME_STORE: MailboxContract = MailboxContract {
    delivery: DeliveryClass::RealtimeBounded,
    overflow: OverflowAction::DropStaleAndMarkMiss,
    producer_must_not_block: true,
};

pub const TELEMETRY_MAILBOX: MailboxContract = MailboxContract {
    delivery: DeliveryClass::CoalescedLatest,
    overflow: OverflowAction::CoalesceLatest,
    producer_must_not_block: true,
};
