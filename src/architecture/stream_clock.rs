//! Session-scoped authoritative media timeline.
//!
//! There is one 10 ms grid per active stream group. Media production and the
//! IsoTxBridge reference the same FrameId -> deadline mapping.
//!
//! ASRC is not the clock. ASRC is an actuator that maps a source clock (WASAPI)
//! into this target clock domain. The initial clock source may be local
//! monotonic time; a later implementation may lock the same abstraction to the
//! controller ISO transmit grid.

use crate::plane::l1_media::clock::frame::FrameId;

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
