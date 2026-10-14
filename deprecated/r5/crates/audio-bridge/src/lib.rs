//! PCM handoff boundary between LE-Audio-Relay and the LE Audio sender.

use device_policy::StreamPlan;

pub const RELAY_SAMPLE_RATE_HZ: u32 = 48_000;
pub const RELAY_CHANNELS: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmSampleFormat {
    Float32Le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub sample_format: PcmSampleFormat,
}

pub const RELAY_PCM_FORMAT: PcmFormat = PcmFormat {
    sample_rate_hz: RELAY_SAMPLE_RATE_HZ,
    channels: RELAY_CHANNELS,
    sample_format: PcmSampleFormat::Float32Le,
};

pub fn validate_plan_input(plan: &StreamPlan) -> bool {
    plan.sample_rate_hz == RELAY_SAMPLE_RATE_HZ
}

pub mod first_sound;
