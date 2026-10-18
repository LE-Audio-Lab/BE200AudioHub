use crate::{
    core::condition::interface::snapshot::MediaConditionView,
    plane::l1_media::interface::condition::{
        MediaConditionState, MediaConditionUpdate,
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::core::condition) struct MediaBlackboard {
    value: MediaConditionView,
}

impl MediaBlackboard {
    pub(in crate::core::condition) const fn initial() -> Self {
        Self {
            value: MediaConditionView {
                state: MediaConditionState::initial(),
                bridge_telemetry: None,
            },
        }
    }

    pub(in crate::core::condition) const fn value(self) -> MediaConditionView {
        self.value
    }

    pub(in crate::core::condition) fn apply(
        &mut self,
        update: MediaConditionUpdate,
    ) -> bool {
        let next = match update {
            MediaConditionUpdate::State(state) => MediaConditionView {
                state,
                bridge_telemetry: self.value.bridge_telemetry,
            },
            MediaConditionUpdate::Telemetry(bridge_telemetry) => MediaConditionView {
                state: self.value.state,
                bridge_telemetry: Some(bridge_telemetry),
            },
            MediaConditionUpdate::Final {
                state,
                bridge_telemetry,
            } => MediaConditionView {
                state,
                bridge_telemetry: Some(bridge_telemetry),
            },
        };
        if self.value == next { return false; }
        self.value = next;
        true
    }

    pub(in crate::core::condition) fn reset(&mut self) -> bool {
        let next = Self::initial();
        if *self == next { return false; }
        *self = next;
        true
    }
}
