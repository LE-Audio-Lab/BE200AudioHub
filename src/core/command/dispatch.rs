use crate::{
    core::{
        command::{
            admission::{admit, CommandAdmissionError},
            interface::CommandOutcome,
            types::CoreCommand,
        },
        condition::Condition,
    },
    plane::l1_control::{
        engine::{facade::ControlPlane, machine::ControlFailure},
        interface::command::ControlIntent,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Denied(CommandAdmissionError),
    Control(ControlFailure),
}

pub fn execute_control(
    condition: &Condition,
    control: &mut ControlPlane,
    command: CoreCommand,
) -> Result<CommandOutcome, CommandError> {
    admit(condition, command).map_err(CommandError::Denied)?;

    let intent = match command {
        CoreCommand::StartControl => ControlIntent::Start,
        CoreCommand::BeginStreamHandoff {
            session_id,
            stream_id,
        } => ControlIntent::BeginStreamHandoff {
            session_id,
            stream_id,
        },
    };

    // Condition admission is advisory/read-side. The actual Control owner
    // validates its own current state again inside handle_intent().
    let control_step = control
        .handle_intent(intent)
        .map_err(CommandError::Control)?;
    Ok(CommandOutcome { control_step })
}
