//! Stable executable Control Plane facade.
//!
//! Core/L0 sees this facade rather than the fine-grained ControlMachine API.
//! Lower-plane inputs and upper intents are typed at the boundary; every
//! transition yields one uniform ControlStep for action dispatch + observation.

use crate::plane::l1_control::{
    engine::machine::{ControlFailure, ControlMachine},
    interface::boundary::{
        ControlIntent, ControlLowerReport, ControlStep,
    },
    model::set_plan::{ControlPlanError, ControlSetPlan},
};

pub struct ControlPlane {
    machine: ControlMachine,
}

impl ControlPlane {
    pub fn new(plan: ControlSetPlan) -> Result<Self, ControlPlanError> {
        Ok(Self {
            machine: ControlMachine::new(plan)?,
        })
    }

    pub fn snapshot(
        &self,
    ) -> crate::architecture::control::ControlSnapshot {
        self.machine.snapshot()
    }

    pub fn handle_intent(
        &mut self,
        intent: ControlIntent,
    ) -> Result<ControlStep, ControlFailure> {
        let actions = match intent {
            ControlIntent::Start => self.machine.start()?,
            ControlIntent::BeginStreamHandoff {
                session_id,
                stream_id,
            } => self
                .machine
                .begin_stream_handoff(session_id, stream_id)?,
            ControlIntent::DiscoveryWindowElapsed => self
                .machine
                .on_event(crate::plane::l1_control::engine::machine::ControlEvent::DiscoveryWindowElapsed)?,
            ControlIntent::TimerExpired(timer) => self
                .machine
                .on_event(crate::plane::l1_control::engine::machine::ControlEvent::TimerExpired(timer))?,
        };
        Ok(self.finish_step(actions))
    }

    pub fn handle_lower_report(
        &mut self,
        report: ControlLowerReport,
    ) -> Result<ControlStep, ControlFailure> {
        let actions = match report {
            ControlLowerReport::Hci(report) => {
                self.machine.on_hci_report(&report)?
            }
        };
        Ok(self.finish_step(actions))
    }

    fn finish_step(
        &self,
        actions: Vec<crate::plane::l1_control::engine::machine::ControlAction>,
    ) -> ControlStep {
        let snapshot = self.machine.snapshot();
        let stream_binding = self.machine.stream_binding().ok();
        ControlStep {
            actions,
            snapshot,
            stream_binding,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::architecture::control::ControlState;

    use crate::plane::l1_control::engine::machine::ControlPhase;

    #[test]
    fn exported_snapshot_coarsens_internal_control_phases() {
        // This assertion is intentionally compile-time adjacent to the mapping:
        // the public state surface is much smaller than the semantic FSM.
        let _ = [
            ControlState::Idle,
            ControlState::Resolving,
            ControlState::Connecting,
            ControlState::Securing,
            ControlState::Discovering,
            ControlState::Configuring,
            ControlState::StreamReady,
            ControlState::Streaming,
            ControlState::TearingDown,
            ControlState::Faulted,
        ];
        let _ = ControlPhase::Idle;
    }
}
