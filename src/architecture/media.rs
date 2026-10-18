//! L1 — Media Plane.
//!
//! Owns Windows audio ingestion and the complete media clock domain:
//! WASAPI source -> PCM clock-domain buffer -> clock discipline / ASRC ->
//! channel mapping -> codec encode -> encoded stereo frame production.
//!
//! It does not know CIS handles, HCI credits, USB endpoints or controller
//! packet framing.

use crate::plane::l1_media::clock::frame::FrameId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaState {
    Stopped,
    Prepared,
    Primed,
    Running,
    Draining,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaSnapshot {
    pub state: MediaState,
    pub produced_frame: Option<FrameId>,
    pub source_ring_fill_frames: u32,
    pub encoded_ring_fill_frames: u32,
    pub asrc_ratio: f64,
}
