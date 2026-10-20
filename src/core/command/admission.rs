use crate::core::{
    command::types::CoreCommand,
    condition::{
        interface::{StartControlAdmission, StopControlAdmission, StreamHandoffAdmission},
        Condition,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandAdmissionError {
    StartControl(StartControlAdmission),
    StopControl(StopControlAdmission),
    StreamHandoff(StreamHandoffAdmission),
}

pub fn admit(
    condition: &Condition,
    command: CoreCommand,
) -> Result<(), CommandAdmissionError> {
    match command {
        CoreCommand::StartControl => match condition.start_control_admission() {
            StartControlAdmission::Allowed => Ok(()),
            denied => Err(CommandAdmissionError::StartControl(denied)),
        },
        CoreCommand::StopControl { .. } => match condition.stop_control_admission() {
            StopControlAdmission::Allowed => Ok(()),
            denied => Err(CommandAdmissionError::StopControl(denied)),
        },
        CoreCommand::BeginStreamHandoff { session_id, .. } => {
            match condition.stream_handoff_admission(session_id) {
                StreamHandoffAdmission::Allowed => Ok(()),
                denied => Err(CommandAdmissionError::StreamHandoff(denied)),
            }
        }
    }
}
