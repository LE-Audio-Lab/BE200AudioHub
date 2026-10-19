use std::sync::atomic::{AtomicU64, Ordering};

use crate::platform::backend::RxEndpoint;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AclTransportTxSnapshot {
    pub(crate) mailbox_enqueued: u64,
    pub(crate) usb_submitted: u64,
    pub(crate) usb_completed: u64,
    pub(crate) event_read_arms: u64,
    pub(crate) event_read_completions: u64,
    pub(crate) event_read_cancelled: u64,
    pub(crate) acl_read_arms: u64,
    pub(crate) acl_read_completions: u64,
    pub(crate) acl_read_cancelled: u64,
}

#[derive(Debug, Default)]
pub(crate) struct TransportTxAudit {
    acl_mailbox_enqueued: AtomicU64,
    acl_usb_submitted: AtomicU64,
    acl_usb_completed: AtomicU64,
    event_read_arms: AtomicU64,
    event_read_completions: AtomicU64,
    event_read_cancelled: AtomicU64,
    acl_read_arms: AtomicU64,
    acl_read_completions: AtomicU64,
    acl_read_cancelled: AtomicU64,
}

impl TransportTxAudit {
    pub(crate) fn observe_mailbox_enqueued(&self) {
        self.acl_mailbox_enqueued.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn observe_usb_submitted(&self) {
        self.acl_usb_submitted.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn observe_usb_completed(&self) {
        self.acl_usb_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn observe_read_arm(&self, endpoint: RxEndpoint) {
        match endpoint {
            RxEndpoint::Event => {
                self.event_read_arms.fetch_add(1, Ordering::Relaxed);
            }
            RxEndpoint::Acl => {
                self.acl_read_arms.fetch_add(1, Ordering::Relaxed);
            }
            RxEndpoint::Iso => {}
        }
    }

    pub(crate) fn observe_read_completion(&self, endpoint: RxEndpoint) {
        match endpoint {
            RxEndpoint::Event => {
                self.event_read_completions.fetch_add(1, Ordering::Relaxed);
            }
            RxEndpoint::Acl => {
                self.acl_read_completions.fetch_add(1, Ordering::Relaxed);
            }
            RxEndpoint::Iso => {}
        }
    }

    pub(crate) fn observe_read_cancelled(&self, endpoint: RxEndpoint) {
        match endpoint {
            RxEndpoint::Event => {
                self.event_read_cancelled.fetch_add(1, Ordering::Relaxed);
            }
            RxEndpoint::Acl => {
                self.acl_read_cancelled.fetch_add(1, Ordering::Relaxed);
            }
            RxEndpoint::Iso => {}
        }
    }

    pub(crate) fn snapshot(&self) -> AclTransportTxSnapshot {
        AclTransportTxSnapshot {
            mailbox_enqueued: self.acl_mailbox_enqueued.load(Ordering::Relaxed),
            usb_submitted: self.acl_usb_submitted.load(Ordering::Relaxed),
            usb_completed: self.acl_usb_completed.load(Ordering::Relaxed),
            event_read_arms: self.event_read_arms.load(Ordering::Relaxed),
            event_read_completions: self
                .event_read_completions
                .load(Ordering::Relaxed),
            event_read_cancelled: self
                .event_read_cancelled
                .load(Ordering::Relaxed),
            acl_read_arms: self.acl_read_arms.load(Ordering::Relaxed),
            acl_read_completions: self
                .acl_read_completions
                .load(Ordering::Relaxed),
            acl_read_cancelled: self
                .acl_read_cancelled
                .load(Ordering::Relaxed),
        }
    }
}
