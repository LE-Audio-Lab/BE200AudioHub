//! Core read-side owner.
//!
//! Blackboard is private implementation state. All external reads go through
//! Condition queries or value snapshots.

mod blackboard;
pub mod interface;
mod reducer;

use blackboard::Blackboard;
use reducer::ConditionReducer;

use crate::{
    core::{
        condition::interface::{
            ConditionChange, ConditionScope, RuntimeStatus,
            StartControlAdmission, StreamHandoffAdmission,
        },
        lifecycle::{LifecycleSnapshot, SessionId},
    },
    plane::{
        l1_control::interface::condition::{ControlConditionUpdate, ControlState},
        l1_hci::interface::condition::HciConditionUpdate,
        l1_media::interface::condition::MediaConditionUpdate,
        l2_transport::interface::condition::TransportConditionUpdate,
    },
};
pub use interface::snapshot::ConditionSnapshot;

pub struct Condition {
    blackboard: Blackboard,
}

impl Condition {
    pub const fn new(lifecycle: LifecycleSnapshot) -> Self {
        Self { blackboard: Blackboard::new(lifecycle) }
    }

    pub fn snapshot(&self) -> ConditionSnapshot {
        ConditionSnapshot {
            epoch: self.blackboard.epoch,
            generation: self.blackboard.generation,
            lifecycle: self.blackboard.lifecycle.value(),
            control: self.blackboard.control.value(),
            hci: self.blackboard.hci.value(),
            media: self.blackboard.media.value(),
            transport: self.blackboard.transport.value(),
        }
    }

    pub const fn runtime_status(&self) -> RuntimeStatus { self.blackboard.lifecycle.value() }
    pub const fn control_status(&self) -> ControlConditionUpdate { self.blackboard.control.value() }

    pub fn start_control_admission(&self) -> StartControlAdmission {
        if self.blackboard.control.value().state == ControlState::Idle {
            StartControlAdmission::Allowed
        } else {
            StartControlAdmission::ControlNotIdle
        }
    }

    pub fn stream_handoff_admission(&self, session_id: SessionId) -> StreamHandoffAdmission {
        if self.blackboard.lifecycle.value().session_id != Some(session_id) {
            return StreamHandoffAdmission::SessionMismatch;
        }
        let control = self.blackboard.control.value();
        if control.state != ControlState::StreamReady {
            return StreamHandoffAdmission::ControlNotReady;
        }
        if control.stream_id.is_some() {
            return StreamHandoffAdmission::StreamAlreadyPresent;
        }
        StreamHandoffAdmission::Allowed
    }

    pub fn apply_lifecycle(&mut self, update: LifecycleSnapshot) -> ConditionChange {
        ConditionReducer::lifecycle(&mut self.blackboard, update)
    }
    pub fn apply_control(&mut self, scope: ConditionScope, update: ControlConditionUpdate) -> ConditionChange {
        ConditionReducer::control(&mut self.blackboard, scope, update)
    }
    pub fn apply_hci(&mut self, scope: ConditionScope, update: HciConditionUpdate) -> ConditionChange {
        ConditionReducer::hci(&mut self.blackboard, scope, update)
    }
    pub fn apply_media(&mut self, scope: ConditionScope, update: MediaConditionUpdate) -> ConditionChange {
        ConditionReducer::media(&mut self.blackboard, scope, update)
    }
    pub fn apply_transport(&mut self, scope: ConditionScope, update: TransportConditionUpdate) -> ConditionChange {
        ConditionReducer::transport(&mut self.blackboard, scope, update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::lifecycle::{LifecycleState, SessionState, StreamId},
        plane::l1_control::interface::{
            condition::{ControlConditionUpdate, ControlState},
            stream::LeAudioFormat,
        },
        runtime::generation::Generation,
    };

    fn lifecycle(generation: u64, session: Option<u64>) -> LifecycleSnapshot {
        LifecycleSnapshot {
            generation: Generation(generation),
            session_id: session.map(SessionId),
            lifecycle: LifecycleState::Running,
            session: if session.is_some() { SessionState::Preparing } else { SessionState::Idle },
        }
    }

    #[test]
    fn identical_update_does_not_churn_epoch() {
        let mut condition = Condition::new(lifecycle(7, None));
        let update = TransportConditionUpdate::initial();
        let change = condition.apply_transport(ConditionScope::runtime(Generation(7)), update);
        assert!(!change.changed);
        assert!(!change.stale);
        assert_eq!(change.epoch, 0);
    }

    #[test]
    fn stale_generation_and_session_updates_are_rejected() {
        let mut condition = Condition::new(lifecycle(7, Some(11)));
        let update = ControlConditionUpdate { state: ControlState::Resolving, stream_id: None, format: None };
        assert!(condition.apply_control(ConditionScope::session(Generation(6), SessionId(11)), update).stale);
        assert!(condition.apply_control(ConditionScope::session(Generation(7), SessionId(12)), update).stale);
        assert_eq!(condition.snapshot().control, ControlConditionUpdate::initial());
    }

    #[test]
    fn generation_advance_resets_plane_read_projection() {
        let mut condition = Condition::new(lifecycle(7, Some(11)));
        condition.apply_control(
            ConditionScope::session(Generation(7), SessionId(11)),
            ControlConditionUpdate {
                state: ControlState::StreamReady,
                stream_id: Some(StreamId(9)),
                format: Some(LeAudioFormat { sample_rate_hz: 48_000, frame_duration_us: 10_000, octets_per_sdu: 100 }),
            },
        );
        condition.apply_lifecycle(lifecycle(8, None));
        assert_eq!(condition.snapshot().control, ControlConditionUpdate::initial());
        assert_eq!(condition.snapshot().generation, Generation(8));
    }

    #[test]
    fn stream_handoff_query_is_semantic_and_blackboard_stays_private() {
        let mut condition = Condition::new(lifecycle(7, Some(11)));
        condition.apply_control(
            ConditionScope::session(Generation(7), SessionId(11)),
            ControlConditionUpdate { state: ControlState::StreamReady, stream_id: None, format: None },
        );
        assert_eq!(condition.stream_handoff_admission(SessionId(11)), StreamHandoffAdmission::Allowed);
        assert_eq!(condition.stream_handoff_admission(SessionId(12)), StreamHandoffAdmission::SessionMismatch);
    }
}
