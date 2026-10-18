use crate::core::{
    command::types::CoreCommand,
    condition::{
        interface::{StartControlAdmission, StreamHandoffAdmission},
        Condition,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandAdmissionError {
    StartControl(StartControlAdmission),
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
        CoreCommand::BeginStreamHandoff { session_id, .. } => {
            match condition.stream_handoff_admission(session_id) {
                StreamHandoffAdmission::Allowed => Ok(()),
                denied => Err(CommandAdmissionError::StreamHandoff(denied)),
            }
        }
    }
}
