//! Coarse L1-Media condition projection.
//!
//! Only already-aggregated telemetry may be attached to this update.

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
pub struct MediaConditionUpdate {
    pub state: MediaState,
    pub produced_frame: Option<FrameId>,
    pub source_ring_fill_frames: u32,
    pub encoded_ring_fill_frames: u32,
    pub asrc_ratio: f64,
    pub clock: Option<StreamClockSnapshot>,
    pub bridge_telemetry: Option<MediaBridgeTelemetrySnapshot>,
}

impl MediaConditionUpdate {
    pub const fn initial() -> Self {
        Self {
            state: MediaState::Stopped,
            produced_frame: None,
            source_ring_fill_frames: 0,
            encoded_ring_fill_frames: 0,
            asrc_ratio: 1.0,
            clock: None,
            bridge_telemetry: None,
        }
    }
}
