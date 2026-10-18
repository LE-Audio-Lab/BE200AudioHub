use crate::plane::l1_control::engine::step::ControlStep;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutcome {
    pub control_step: ControlStep,
}
