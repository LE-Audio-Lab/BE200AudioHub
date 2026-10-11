//! Device/topology policy. No Buds3 Pro topology assumptions are frozen here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamTopology {
    Mono,
    StereoSingleAse,
    StereoDualMonoAse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPlan {
    pub sample_rate_hz: u32,
    pub frame_duration_us: u32,
    pub topology: StreamTopology,
    pub octets_per_codec_frame: u16,
}

impl StreamPlan {
    pub fn first_sound(
        topology: StreamTopology,
        octets_per_codec_frame: u16,
    ) -> Self {
        Self {
            sample_rate_hz: 48_000,
            frame_duration_us: 10_000,
            topology,
            octets_per_codec_frame,
        }
    }
}
