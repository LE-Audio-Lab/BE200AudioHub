use crate::plane::l1_control::interface::stream::StreamBinding;

/// Temporary logical stream handoff state while HCI route bind is awaiting ACK.
///
/// A binding becomes durable only after a successful Bound ACK, at which point
/// the engine promotes it into ConfirmedStatus::active_stream_binding.
pub(crate) struct StreamRouteOperation {
    pub(crate) pending_binding: Option<StreamBinding>,
}

impl StreamRouteOperation {
    pub(crate) const fn new() -> Self {
        Self {
            pending_binding: None,
        }
    }
}
