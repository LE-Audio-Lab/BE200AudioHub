//! Execution-domain contracts.
//!
//! Architecture code depends on serial execution contexts rather than concrete
//! OS thread identities. Runtime composition may remap contexts to threads after
//! profiling without changing subsystem ownership.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExecutionDomain {
    Core,
    HciHost,
    MediaRt,
    WasapiCapture,
    UsbReactor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockingRule {
    /// No blocking waits, sleeps, synchronous cross-context calls or hardware I/O.
    Forbidden,
    /// The only intentional wait is the scheduler sleeping until an absolute
    /// StreamClock deadline or a cancellation/stop signal.
    DeadlineOnly,
    /// Blocking is restricted to the platform source event plus cancellation.
    SourceEventOnly,
    /// Blocking is restricted to the OS/libusb completion reactor. Endpoint
    /// operations themselves remain concurrently pending.
    IoCompletionOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionPolicy {
    pub domain: ExecutionDomain,
    pub blocking: BlockingRule,
    pub owns_mutable_state: bool,
    pub hot_path_must_be_bounded: bool,
}

impl ExecutionDomain {
    pub const fn policy(self) -> ExecutionPolicy {
        match self {
            Self::Core => ExecutionPolicy {
                domain: self,
                blocking: BlockingRule::Forbidden,
                owns_mutable_state: true,
                hot_path_must_be_bounded: true,
            },
            Self::HciHost => ExecutionPolicy {
                domain: self,
                blocking: BlockingRule::Forbidden,
                owns_mutable_state: true,
                hot_path_must_be_bounded: true,
            },
            Self::MediaRt => ExecutionPolicy {
                domain: self,
                blocking: BlockingRule::DeadlineOnly,
                owns_mutable_state: true,
                hot_path_must_be_bounded: true,
            },
            Self::WasapiCapture => ExecutionPolicy {
                domain: self,
                blocking: BlockingRule::SourceEventOnly,
                owns_mutable_state: true,
                hot_path_must_be_bounded: true,
            },
            Self::UsbReactor => ExecutionPolicy {
                domain: self,
                blocking: BlockingRule::IoCompletionOnly,
                owns_mutable_state: true,
                hot_path_must_be_bounded: true,
            },
        }
    }
}
