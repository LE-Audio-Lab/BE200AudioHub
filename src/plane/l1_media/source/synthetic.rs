//! Deterministic synthetic PCM source for the first Review media slice.
//!
//! This module owns no clock/timer and performs no I/O. It only maps a logical
//! FrameId to one 10 ms stereo PCM frame. Scheduling belongs to StreamClock /
//! IsoTxBridge later in the architecture.

use crate::plane::l1_media::clock::frame::FrameId;

pub const SAMPLE_RATE_HZ: u32 = 48_000;
pub const FRAME_DURATION_US: u32 = 10_000;
pub const SAMPLES_PER_FRAME: usize = 480;
pub const FRAMES_PER_SECOND: u64 = 100;

pub const DEFAULT_LEFT_TONE_HZ: u32 = 400;
pub const DEFAULT_RIGHT_TONE_HZ: u32 = 600;

/// Approximately -50 dBFS peak for signed 16-bit PCM.
///
/// This is deliberately far below the sealed R5 First Sound fixture
/// (peak=2048, about -24 dBFS). dBFS is not a calibrated acoustic SPL; real HIL
/// should still begin with the earbuds out of the ears.
pub const DEFAULT_PEAK_I16: i16 = 104;

/// 100 ms cosine fade at both ends of the one-second default fixture.
pub const DEFAULT_FADE_SAMPLES: u64 = 4_800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntheticToneConfig {
    pub frame_count: u64,
    pub left_hz: u32,
    pub right_hz: u32,
    pub peak_i16: i16,
    pub fade_samples: u64,
}

impl Default for SyntheticToneConfig {
    fn default() -> Self {
        Self {
            frame_count: FRAMES_PER_SECOND,
            left_hz: DEFAULT_LEFT_TONE_HZ,
            right_hz: DEFAULT_RIGHT_TONE_HZ,
            peak_i16: DEFAULT_PEAK_I16,
            fade_samples: DEFAULT_FADE_SAMPLES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcmStereoFrame {
    pub frame_id: FrameId,
    pub left: [i16; SAMPLES_PER_FRAME],
    pub right: [i16; SAMPLES_PER_FRAME],
}

pub struct SyntheticToneSource {
    config: SyntheticToneConfig,
    next_frame: u64,
    total_samples: u64,
}

impl SyntheticToneSource {
    pub fn new(config: SyntheticToneConfig) -> Result<Self, String> {
        if config.frame_count == 0 {
            return Err("synthetic tone requires at least one frame".into());
        }
        let nyquist_hz = SAMPLE_RATE_HZ / 2;
        for (label, frequency) in [
            ("left", config.left_hz),
            ("right", config.right_hz),
        ] {
            if frequency == 0 || frequency >= nyquist_hz {
                return Err(format!(
                    "{label} tone must be in 1..{nyquist_hz} Hz"
                ));
            }
        }
        if config.peak_i16 <= 0 {
            return Err("synthetic tone peak must be positive".into());
        }

        let total_samples = config
            .frame_count
            .checked_mul(SAMPLES_PER_FRAME as u64)
            .ok_or_else(|| "synthetic tone sample count overflow".to_string())?;

        if config.fade_samples.saturating_mul(2) > total_samples {
            return Err(
                "synthetic tone fades must fit inside the requested duration"
                    .into(),
            );
        }

        Ok(Self {
            config,
            next_frame: 0,
            total_samples,
        })
    }

    pub fn remaining_frames(&self) -> u64 {
        self.config.frame_count.saturating_sub(self.next_frame)
    }

    pub fn next_frame(&mut self) -> Option<PcmStereoFrame> {
        if self.next_frame >= self.config.frame_count {
            return None;
        }

        let frame_id = FrameId(self.next_frame);
        let frame_index = self.next_frame;
        self.next_frame += 1;

        Some(PcmStereoFrame {
            frame_id,
            left: self.render_channel(frame_index, self.config.left_hz),
            right: self.render_channel(frame_index, self.config.right_hz),
        })
    }

    fn render_channel(
        &self,
        frame_index: u64,
        frequency_hz: u32,
    ) -> [i16; SAMPLES_PER_FRAME] {
        let mut pcm = [0i16; SAMPLES_PER_FRAME];

        for (offset, value) in pcm.iter_mut().enumerate() {
            let sample =
                frame_index * SAMPLES_PER_FRAME as u64 + offset as u64;
            let distance_from_end =
                self.total_samples.saturating_sub(sample + 1);
            let edge_distance = sample.min(distance_from_end);

            let fade = if self.config.fade_samples == 0
                || edge_distance >= self.config.fade_samples
            {
                1.0
            } else {
                0.5
                    - 0.5
                        * (std::f64::consts::PI
                            * edge_distance as f64
                            / self.config.fade_samples as f64)
                            .cos()
            };

            // Absolute sample position preserves oscillator phase across frame
            // boundaries. FrameId partitions the signal; it does not restart it.
            let phase = std::f64::consts::TAU
                * frequency_hz as f64
                * sample as f64
                / SAMPLE_RATE_HZ as f64;
            *value = (f64::from(self.config.peak_i16)
                * fade
                * phase.sin())
                .round() as i16;
        }

        pcm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fixture_matches_review_first_sound_source_contract() {
        let config = SyntheticToneConfig::default();
        assert_eq!(config.frame_count, 100);
        assert_eq!(config.left_hz, 400);
        assert_eq!(config.right_hz, 600);
        assert_eq!(config.peak_i16, 104);
        assert_eq!(config.fade_samples, 4_800);
        assert_eq!(SAMPLE_RATE_HZ, 48_000);
        assert_eq!(FRAME_DURATION_US, 10_000);
        assert_eq!(SAMPLES_PER_FRAME, 480);
    }

    #[test]
    fn source_emits_exact_monotonic_frame_ids_then_stops() {
        let mut source =
            SyntheticToneSource::new(SyntheticToneConfig::default())
                .unwrap();

        for expected in 0..FRAMES_PER_SECOND {
            let frame = source.next_frame().unwrap();
            assert_eq!(frame.frame_id, FrameId(expected));
            assert_eq!(
                source.remaining_frames(),
                FRAMES_PER_SECOND - expected - 1
            );
        }

        assert!(source.next_frame().is_none());
        assert_eq!(source.remaining_frames(), 0);
    }

    #[test]
    fn default_fixture_is_low_level_and_fades_to_zero_at_both_edges() {
        let mut source =
            SyntheticToneSource::new(SyntheticToneConfig::default())
                .unwrap();

        let first = source.next_frame().unwrap();
        assert_eq!(first.left[0], 0);
        assert_eq!(first.right[0], 0);

        let mut last = first.clone();
        while let Some(frame) = source.next_frame() {
            last = frame;
        }

        assert_eq!(last.left[SAMPLES_PER_FRAME - 1], 0);
        assert_eq!(last.right[SAMPLES_PER_FRAME - 1], 0);

        let mut source =
            SyntheticToneSource::new(SyntheticToneConfig::default())
                .unwrap();
        while let Some(frame) = source.next_frame() {
            assert!(frame
                .left
                .iter()
                .chain(frame.right.iter())
                .all(|sample| sample.abs() <= DEFAULT_PEAK_I16));
        }
    }

    #[test]
    fn left_and_right_channels_are_distinct() {
        let mut source =
            SyntheticToneSource::new(SyntheticToneConfig::default())
                .unwrap();

        // Skip the very beginning of the fade and compare a stable frame.
        for _ in 0..20 {
            source.next_frame().unwrap();
        }
        let frame = source.next_frame().unwrap();
        assert_ne!(frame.left, frame.right);
    }

    #[test]
    fn rejects_invalid_or_nyquist_frequency_and_oversized_fade() {
        let mut config = SyntheticToneConfig::default();
        config.left_hz = 0;
        assert!(SyntheticToneSource::new(config).is_err());

        let mut config = SyntheticToneConfig::default();
        config.right_hz = SAMPLE_RATE_HZ / 2;
        assert!(SyntheticToneSource::new(config).is_err());

        let mut config = SyntheticToneConfig::default();
        config.fade_samples = 24_001;
        assert!(SyntheticToneSource::new(config).is_err());
    }
}
