use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AclTransportTxSnapshot {
    pub(crate) mailbox_enqueued: u64,
    pub(crate) usb_submitted: u64,
    pub(crate) usb_completed: u64,
}

#[derive(Debug, Default)]
pub(crate) struct TransportTxAudit {
    acl_mailbox_enqueued: AtomicU64,
    acl_usb_submitted: AtomicU64,
    acl_usb_completed: AtomicU64,
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

    pub(crate) fn snapshot(&self) -> AclTransportTxSnapshot {
        AclTransportTxSnapshot {
            mailbox_enqueued: self.acl_mailbox_enqueued.load(Ordering::Relaxed),
            usb_submitted: self.acl_usb_submitted.load(Ordering::Relaxed),
            usb_completed: self.acl_usb_completed.load(Ordering::Relaxed),
        }
    }
}
