//! Core write-side API.
//!
//! Command queries Condition for admission and then invokes the typed owner
//! facade. It never reads Blackboard directly and never mutates Condition.

pub mod admission;
pub mod dispatch;
pub mod interface;
pub mod types;

pub use dispatch::{execute_control, CommandError};
pub use types::CoreCommand;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{
            command::admission::{admit, CommandAdmissionError},
            condition::{
                interface::{ConditionScope, StreamHandoffAdmission},
                Condition,
            },
            lifecycle::{
                LifecycleSnapshot, LifecycleState, SessionId, SessionState,
                StreamId,
            },
        },
        plane::l1_control::interface::condition::{
            ControlConditionUpdate, ControlState,
        },
        runtime::generation::Generation,
    };

    fn test_condition() -> Condition {
        Condition::new(LifecycleSnapshot {
            generation: Generation(7),
            session_id: Some(SessionId(11)),
            lifecycle: LifecycleState::Running,
            session: SessionState::Preparing,
        })
    }

    #[test]
    fn command_admission_uses_condition_query_not_blackboard() {
        let mut condition = test_condition();
        condition.apply_control(
            ConditionScope::session(Generation(7), SessionId(11)),
            ControlConditionUpdate {
                state: ControlState::StreamReady,
                stream_id: None,
                format: None,
            },
        );

        assert_eq!(
            admit(
                &condition,
                CoreCommand::BeginStreamHandoff {
                    session_id: SessionId(11),
                    stream_id: StreamId(99),
                },
            ),
            Ok(())
        );
    }

    #[test]
    fn command_admission_rejects_wrong_session_before_owner_call() {
        let mut condition = test_condition();
        condition.apply_control(
            ConditionScope::session(Generation(7), SessionId(11)),
            ControlConditionUpdate {
                state: ControlState::StreamReady,
                stream_id: None,
                format: None,
            },
        );

        assert_eq!(
            admit(
                &condition,
                CoreCommand::BeginStreamHandoff {
                    session_id: SessionId(12),
                    stream_id: StreamId(99),
                },
            ),
            Err(CommandAdmissionError::StreamHandoff(
                StreamHandoffAdmission::SessionMismatch,
            )),
        );
    }
}
