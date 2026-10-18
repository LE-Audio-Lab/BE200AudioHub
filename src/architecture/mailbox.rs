//! Transitional product-scoped message envelope semantics.
//!
//! Generic delivery/backpressure policy lives in runtime::mailbox::contract.
//! MessageScope remains here temporarily because it carries product lifecycle
//! identities (Generation/SessionId/StreamId) and has no active runtime owner.

use super::types::{Generation, SessionId, StreamId};
use crate::runtime::mailbox::contract::{
    DeliveryClass, MailboxContract, OverflowAction,
};

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
