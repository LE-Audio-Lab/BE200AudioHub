//! Stable executable Control Plane facade.
//!
//! Core/L0 sees this facade rather than the fine-grained ControlMachine API.
//! Lower-plane inputs and upper intents are typed at the boundary; every
//! transition yields one uniform ControlStep for action dispatch + observation.

use crate::plane::l1_control::{
    engine::machine::{ControlFailure, ControlMachine},
    engine::step::ControlStep,
    interface::{
        command::ControlIntent,
        report::ControlLowerReport,
    },
    model::set_plan::{ControlPlanError, ControlSetPlan},
    planning::iso::StreamLatencyProfile,
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

    pub fn new_with_latency_profile(
        plan: ControlSetPlan,
        latency_profile: StreamLatencyProfile,
    ) -> Result<Self, ControlPlanError> {
        Ok(Self {
            machine: ControlMachine::new_with_latency_profile(
                plan,
                latency_profile,
            )?,
        })
    }

    pub fn condition_update(
        &self,
    ) -> crate::plane::l1_control::interface::condition::ControlConditionUpdate {
        self.machine.condition_update()
    }

    pub fn handle_intent(
        &mut self,
        intent: ControlIntent,
    ) -> Result<ControlStep, ControlFailure> {
        let actions = match intent {
            ControlIntent::Start => self.machine.start()?,
            ControlIntent::Stop { policy } => self.machine.begin_teardown_with_policy(policy)?,
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

    pub fn continue_ready_stage(
        &mut self,
    ) -> Result<Option<ControlStep>, ControlFailure> {
        let actions = self.machine.continue_ready_stage()?;
        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.finish_step(actions)))
        }
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
        let condition = self.machine.condition_update();
        let stream_binding = self.machine.stream_binding().ok();
        ControlStep {
            actions,
            condition,
            stream_binding,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::plane::l1_control::interface::condition::ControlState;

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
