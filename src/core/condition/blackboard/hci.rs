use crate::{
    core::condition::interface::snapshot::HciConditionView,
    plane::l1_hci::interface::condition::{
        HciConditionState, HciConditionUpdate,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::core::condition) struct HciBlackboard {
    value: HciConditionView,
}

impl HciBlackboard {
    pub(in crate::core::condition) const fn initial() -> Self {
        Self {
            value: HciConditionView {
                state: HciConditionState::initial(),
                iso_telemetry: None,
            },
        }
    }

    pub(in crate::core::condition) const fn value(self) -> HciConditionView {
        self.value
    }

    pub(in crate::core::condition) fn apply(
        &mut self,
        update: HciConditionUpdate,
    ) -> bool {
        let next = match update {
            HciConditionUpdate::State(state) => HciConditionView {
                state,
                iso_telemetry: self.value.iso_telemetry,
            },
            HciConditionUpdate::Telemetry(iso_telemetry) => HciConditionView {
                state: self.value.state,
                iso_telemetry: Some(iso_telemetry),
            },
            HciConditionUpdate::Final {
                state,
                iso_telemetry,
            } => HciConditionView {
                state,
                iso_telemetry: Some(iso_telemetry),
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

    pub(in crate::core::condition) fn clear_stream(&mut self) -> bool {
        if self.value.state.stream_id.is_none() { return false; }
        self.value.state.stream_id = None;
        true
    }
}
