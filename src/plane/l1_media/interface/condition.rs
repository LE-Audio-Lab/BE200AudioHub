//! L1-Media condition updates.
//!
//! State and aggregate telemetry are separate so coalesced metrics can never
//! overwrite a newer semantic Media state.

use crate::plane::l1_media::{
    clock::frame::FrameId,
    telemetry::MediaBridgeTelemetrySnapshot,
};

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
    pub current_frame: FrameId,
    pub lock: ClockLock,
    pub rate_error_ppm: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaConditionState {
    pub state: MediaState,
    pub produced_frame: Option<FrameId>,
    pub source_ring_fill_frames: u32,
    pub encoded_ring_fill_frames: u32,
    pub asrc_ratio: f64,
    pub clock: Option<StreamClockSnapshot>,
}

impl MediaConditionState {
    pub const fn initial() -> Self {
        Self {
            state: MediaState::Stopped,
            produced_frame: None,
            source_ring_fill_frames: 0,
            encoded_ring_fill_frames: 0,
            asrc_ratio: 1.0,
            clock: None,
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
