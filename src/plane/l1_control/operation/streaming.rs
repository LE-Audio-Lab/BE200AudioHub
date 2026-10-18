use crate::plane::l1_control::ascs::streaming::StreamingGateMachine;

/// Temporary progress for confirming one ASE has reached STREAMING.
///
/// The verified per-side streaming status is promoted into ConfirmedStatus.
/// This owner contains only the in-progress gate machine.
pub(crate) struct StreamingOperation {
    pub(crate) gate: Option<StreamingGateMachine>,
}

impl StreamingOperation {
    pub(crate) const fn new() -> Self {
        Self { gate: None }
    }
}
