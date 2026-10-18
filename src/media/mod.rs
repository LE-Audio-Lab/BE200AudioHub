//! L1 Media Plane implementation.
//!
//! The first Review media slice is intentionally hardware-free: a deterministic
//! PCM producer feeds later codec/scheduler stages without knowing Bluetooth,
//! CIS handles, controller credits, or USB transport.

pub mod synthetic;

pub use synthetic::{
    PcmStereoFrame, SyntheticToneConfig, SyntheticToneSource,
    FRAME_DURATION_US, FRAMES_PER_SECOND, SAMPLE_RATE_HZ,
    SAMPLES_PER_FRAME,
};
