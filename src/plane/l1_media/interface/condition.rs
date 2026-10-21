//! L1-Media condition updates.
//!
//! State and aggregate telemetry are separate so coalesced metrics can never
//! overwrite a newer semantic Media state.

use crate::plane::l1_media::telemetry::MediaBridgeTelemetrySnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaState {
    Stopped,
    Prepared,
    Primed,
    Running,
    Draining,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockLock {
    Unlocked,
    LocalMonotonic,
    ControllerAnchored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaConditionState {
    pub state: MediaState,
    pub clock_lock: ClockLock,
}

impl MediaConditionState {
    pub const fn initial() -> Self {
        Self {
            state: MediaState::Stopped,
            clock_lock: ClockLock::Unlocked,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaConditionUpdate {
    State(MediaConditionState),
    Telemetry(MediaBridgeTelemetrySnapshot),
    Final {
        state: MediaConditionState,
        bridge_telemetry: MediaBridgeTelemetrySnapshot,
    },
}
