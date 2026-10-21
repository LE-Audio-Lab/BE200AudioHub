use crate::{
    core::condition::interface::snapshot::TransportConditionView,
    plane::l2_transport::interface::condition::{
        TransportConditionState, TransportConditionUpdate,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::core::condition) struct TransportBlackboard {
    value: TransportConditionView,
}

impl TransportBlackboard {
    pub(in crate::core::condition) const fn initial() -> Self {
        Self {
            value: TransportConditionView {
                state: TransportConditionState::initial(),
                telemetry: None,
            },
        }
    }

    pub(in crate::core::condition) const fn value(self) -> TransportConditionView {
        self.value
    }

    pub(in crate::core::condition) fn apply(
        &mut self,
        update: TransportConditionUpdate,
    ) -> bool {
        let next = match update {
            TransportConditionUpdate::State(state) => TransportConditionView {
                state,
                telemetry: self.value.telemetry,
            },
            TransportConditionUpdate::Telemetry(telemetry) => TransportConditionView {
                state: self.value.state,
                telemetry: Some(telemetry),
            },
            TransportConditionUpdate::Final { state, telemetry } => {
                TransportConditionView {
                    state,
                    telemetry: Some(telemetry),
                }
            }
        };
        if self.value == next {
            return false;
        }
        self.value = next;
        true
    }

    pub(in crate::core::condition) fn reset(&mut self) -> bool {
        let next = Self::initial();
        if *self == next {
            return false;
        }
        *self = next;
        true
    }
}
