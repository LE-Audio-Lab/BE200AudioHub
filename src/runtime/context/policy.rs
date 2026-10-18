//! Runtime execution-domain policy.
//!
//! These are execution-context constraints, not product-domain state.

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
    Forbidden,
    DeadlineOnly,
    SourceEventOnly,
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
            Self::Core => ExecutionPolicy { domain: self, blocking: BlockingRule::Forbidden, owns_mutable_state: true, hot_path_must_be_bounded: true },
            Self::HciHost => ExecutionPolicy { domain: self, blocking: BlockingRule::Forbidden, owns_mutable_state: true, hot_path_must_be_bounded: true },
            Self::MediaRt => ExecutionPolicy { domain: self, blocking: BlockingRule::DeadlineOnly, owns_mutable_state: true, hot_path_must_be_bounded: true },
            Self::WasapiCapture => ExecutionPolicy { domain: self, blocking: BlockingRule::SourceEventOnly, owns_mutable_state: true, hot_path_must_be_bounded: true },
            Self::UsbReactor => ExecutionPolicy { domain: self, blocking: BlockingRule::IoCompletionOnly, owns_mutable_state: true, hot_path_must_be_bounded: true },
        }
    }
}
