use crate::runtime::context::policy::ExecutionDomain;

use super::mailbox::{self, BoundedReceiver, BoundedSender, RecvError, SendError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextFaultKind {
    HandlerPanicked,
    InboxDisconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextFault {
    pub domain: ExecutionDomain,
    pub kind: ContextFaultKind,
}

#[derive(Clone, Debug)]
pub struct FaultSink {
    tx: BoundedSender<ContextFault>,
}

#[derive(Debug)]
pub struct FaultInbox {
    rx: BoundedReceiver<ContextFault>,
}

pub fn fault_channel(capacity: usize) -> (FaultSink, FaultInbox) {
    let (tx, rx) = mailbox::bounded(capacity);
    (FaultSink { tx }, FaultInbox { rx })
}

impl FaultSink {
    pub fn report(&self, fault: ContextFault) -> Result<(), SendError> {
        self.tx.try_send(fault)
    }
}

impl FaultInbox {
    pub fn try_recv(&self) -> Result<ContextFault, RecvError> {
        self.rx.try_recv()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_channel_is_bounded_and_nonblocking() {
        let (sink, inbox) = fault_channel(1);
        let fault = ContextFault {
            domain: ExecutionDomain::Core,
            kind: ContextFaultKind::HandlerPanicked,
        };
        sink.report(fault).unwrap();
        assert_eq!(sink.report(fault), Err(SendError::Full));
        assert_eq!(inbox.try_recv(), Ok(fault));
    }
}
