//! Audio-clock -> ISO-clock scheduling boundary.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    pub sample_rate_hz: u32,
    pub sdu_interval_us: u32,
    pub startup_cushion_frames: u32,
    pub max_buffer_frames: u32,
}

impl SchedulerConfig {
    pub fn frame_quantum(&self) -> Option<u32> {
        let product = u64::from(self.sample_rate_hz)
            .checked_mul(u64::from(self.sdu_interval_us))?;
        if product % 1_000_000 != 0 {
            return None;
        }
        u32::try_from(product / 1_000_000).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_ms_at_48k_is_480_frames() {
        let config = SchedulerConfig {
            sample_rate_hz: 48_000,
            sdu_interval_us: 10_000,
            startup_cushion_frames: 480,
            max_buffer_frames: 3_840,
        };
        assert_eq!(config.frame_quantum(), Some(480));
    }
}

pub mod first_sound;
