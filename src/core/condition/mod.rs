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
            StartControlAdmission, StopControlAdmission, StreamHandoffAdmission,
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

    pub fn stop_control_admission(&self) -> StopControlAdmission {
        let control = self.blackboard.control.value();
        if control.state == ControlState::Idle {
            return StopControlAdmission::ControlIdle;
        }
        if control.state != ControlState::StreamReady
            || control.stream_id.is_none()
        {
            return StopControlAdmission::ControlNotBound;
        }
        StopControlAdmission::Allowed
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
        plane::{
            l1_control::interface::{
                condition::{ControlConditionUpdate, ControlState},
                stream::LeAudioFormat,
            },
            l1_hci::{
                interface::condition::{
                    HciConditionState, HciConditionUpdate, HciHostState,
                },
                telemetry::HciIsoTelemetrySnapshot,
            },
            l1_media::{
                interface::condition::{
                    ClockLock, MediaConditionState, MediaConditionUpdate,
                    MediaState, StreamClockSnapshot,
                },
                telemetry::{
                    MediaBridgeTelemetrySnapshot, MediaTelemetrySnapshot,
                    MediaTimingTelemetrySnapshot,
                },
            },
            l2_transport::{
                interface::condition::{
                    TransportConditionState, TransportConditionUpdate,
                    TransportState,
                },
                telemetry::TransportTelemetrySnapshot,
            },
        },
        runtime::{
            generation::Generation,
            telemetry::TelemetryRingStats,
        },
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
        let change = condition.apply_transport(
            ConditionScope::runtime(Generation(7)),
            update,
        );
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
    fn coalesced_hci_telemetry_does_not_clobber_newer_semantic_state() {
        let mut condition = Condition::new(lifecycle(7, Some(11)));
        let scope = ConditionScope::session(Generation(7), SessionId(11));

        let semantic = HciConditionState {
            state: HciHostState::StreamBound,
            stream_id: Some(StreamId(99)),
        };
        condition.apply_hci(scope, HciConditionUpdate::State(semantic));

        let telemetry = HciIsoTelemetrySnapshot {
            controller_limit: Some(8),
            submitted_packets: 20,
            completed_packets: 18,
            retired_packets: 0,
            in_flight_packets: 2,
            peak_in_flight_packets: 8,
            trace: TelemetryRingStats {
                capacity: 256,
                len: 12,
                stored: 12,
                dropped_newest: 0,
                overwritten_oldest: 0,
            },
        };
        condition.apply_hci(scope, HciConditionUpdate::Telemetry(telemetry));

        let snapshot = condition.snapshot();
        assert_eq!(snapshot.hci.state, semantic);
        assert_eq!(snapshot.hci.iso_telemetry, Some(telemetry));
    }

    #[test]
    fn coalesced_media_telemetry_does_not_clobber_newer_semantic_state() {
        let mut condition = Condition::new(lifecycle(7, Some(11)));
        let scope = ConditionScope::session(Generation(7), SessionId(11));

        let semantic = MediaConditionState {
            state: MediaState::Draining,
            clock_lock: ClockLock::LocalMonotonic,
        };
        condition.apply_media(
            scope,
            MediaConditionUpdate::State(semantic),
        );

        let telemetry = MediaTelemetrySnapshot {
            timing: MediaTimingTelemetrySnapshot {
                produced_frame: Some(crate::plane::l1_media::clock::frame::FrameId(12)),
                clock: Some(StreamClockSnapshot {
                    period_us: 10_000,
                    current_frame: crate::plane::l1_media::clock::frame::FrameId(12),
                    lock: ClockLock::LocalMonotonic,
                    rate_error_ppm: 0,
                }),
                pcm_fill_frames: 480,
                intervals_consumed: 13,
                source_intervals_skipped: 1,
                keepalive_intervals: 0,
                pcm_frames_read: 6_240,
                pcm_underflow_frames: 0,
                startup_trim_frames: 480,
                gradual_drift_trim_frames: 1,
                emergency_drift_trim_frames: 0,
                trace: TelemetryRingStats {
                    capacity: 256,
                    len: 1,
                    stored: 1,
                    dropped_newest: 0,
                    overwritten_oldest: 0,
                },
            },
            bridge: MediaBridgeTelemetrySnapshot {
                submitted_intervals: 13,
                dropped_backpressure_intervals: 0,
                fallback_intervals: 0,
                skipped_intervals: 0,
                stale_frames_dropped: 0,
                trace: TelemetryRingStats {
                    capacity: 256,
                    len: 13,
                    stored: 13,
                    dropped_newest: 0,
                    overwritten_oldest: 0,
                },
            },
        };
        condition.apply_media(
            scope,
            MediaConditionUpdate::Telemetry(telemetry),
        );

        let snapshot = condition.snapshot();
        assert_eq!(snapshot.media.state, semantic);
        assert_eq!(snapshot.media.telemetry, Some(telemetry));
    }

    #[test]
    fn coalesced_transport_telemetry_does_not_clobber_newer_fault_state() {
        let mut condition = Condition::new(lifecycle(7, None));
        let scope = ConditionScope::runtime(Generation(7));

        let faulted = TransportConditionState {
            state: TransportState::Faulted,
        };
        condition.apply_transport(
            scope,
            TransportConditionUpdate::State(faulted),
        );

        let telemetry = TransportTelemetrySnapshot {
            rx_packets: 20,
            tx_packets: 10,
            faults: 1,
            trace: TelemetryRingStats {
                capacity: 256,
                len: 4,
                stored: 4,
                dropped_newest: 0,
                overwritten_oldest: 0,
            },
        };
        condition.apply_transport(
            scope,
            TransportConditionUpdate::Telemetry(telemetry),
        );

        let snapshot = condition.snapshot();
        assert_eq!(snapshot.transport.state, faulted);
        assert_eq!(snapshot.transport.telemetry, Some(telemetry));
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
