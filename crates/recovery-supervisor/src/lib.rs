//! Product-level recovery policy. Transport/profile mechanisms live elsewhere.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorState {
    Waiting,
    Connecting,
    Bonding,
    Discovering,
    Configuring,
    Streaming,
    Recovering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureScope {
    CisOnly,
    StreamConfiguration,
    Acl,
    Controller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    RecreateCis,
    ReconfigureStream,
    ReconnectAcl,
    ReattachController,
}

pub fn escalate(scope: FailureScope) -> RecoveryAction {
    match scope {
        FailureScope::CisOnly => RecoveryAction::RecreateCis,
        FailureScope::StreamConfiguration => RecoveryAction::ReconfigureStream,
        FailureScope::Acl => RecoveryAction::ReconnectAcl,
        FailureScope::Controller => RecoveryAction::ReattachController,
    }
}
