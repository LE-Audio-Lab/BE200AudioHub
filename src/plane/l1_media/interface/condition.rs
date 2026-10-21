//! L1-Media condition updates.
//!
//! State and aggregate telemetry are separate so coalesced metrics can never
//! overwrite a newer semantic Media state.

use crate::plane::l1_media::telemetry::MediaTelemetrySnapshot;

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
pub struct StreamClockSnapshot {
    pub period_us: u32,
    pub current_frame: crate::plane::l1_media::clock::frame::FrameId,
    pub lock: ClockLock,
    pub rate_error_ppm: i32,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaConditionUpdate {
    State(MediaConditionState),
    Telemetry(MediaTelemetrySnapshot),
    Final {
        state: MediaConditionState,
        telemetry: MediaTelemetrySnapshot,
    },
}
