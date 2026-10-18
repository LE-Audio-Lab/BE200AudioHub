//! LC3 encoding boundary for the first product media path.
//!
//! The initial synthetic First Sound is fully pre-encoded before playback.
//! Bumble Codecs owns persistent mono LC3 codec state for each ear while
//! preparation runs; those codec workers are dropped before realtime playback.
//! This module has no Bluetooth, CIS, HCI, USB, credit, or scheduling knowledge.

use bumble_codecs::lc3::{
    Lc3Encoder, Lc3FrameDuration, Lc3StreamConfig,
};

use crate::architecture::{
    control::{LeAudioFormat, StreamBinding, StreamTopology},
    media::EncodedStereoFrame,
};

use super::synthetic::{
    PcmStereoFrame, SyntheticToneConfig, SyntheticToneSource,
    FRAME_DURATION_US, SAMPLE_RATE_HZ, SAMPLES_PER_FRAME,
};

pub type EncodedLc3StereoFrame = EncodedStereoFrame<Vec<u8>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaLc3Error {
    UnsupportedTopology(StreamTopology),
    UnsupportedPcmFormat {
        sample_rate_hz: u32,
        frame_duration_us: u32,
    },
    InvalidOctetsPerSdu(u16),
    EncoderInit {
        side: &'static str,
        detail: String,
    },
    Encode {
        side: &'static str,
        frame_id: u64,
        detail: String,
    },
    EncodedLength {
        side: &'static str,
        frame_id: u64,
        expected: usize,
        actual: usize,
    },
    SyntheticSource(String),
}

#[derive(Debug)]
pub struct Lc3StereoEncoder {
    format: LeAudioFormat,
    left: Lc3Encoder,
    right: Lc3Encoder,
}

impl Lc3StereoEncoder {
    pub fn new(format: LeAudioFormat) -> Result<Self, MediaLc3Error> {
        let config = stream_config(format)?;
        let left = Lc3Encoder::new(config).map_err(|error| {
            MediaLc3Error::EncoderInit {
                side: "left",
                detail: error.to_string(),
            }
        })?;
        let right = Lc3Encoder::new(config).map_err(|error| {
            MediaLc3Error::EncoderInit {
                side: "right",
                detail: error.to_string(),
            }
        })?;

        Ok(Self {
            format,
            left,
            right,
        })
    }

    pub fn encode(
        &self,
        pcm: &PcmStereoFrame,
    ) -> Result<EncodedLc3StereoFrame, MediaLc3Error> {
        let expected = usize::from(self.format.octets_per_sdu);
        let left = self.left.encode_sdu(&pcm.left).map_err(|error| {
            MediaLc3Error::Encode {
                side: "left",
                frame_id: pcm.frame_id.0,
                detail: error.to_string(),
            }
        })?;
        let right = self
            .right
            .encode_sdu(&pcm.right)
            .map_err(|error| MediaLc3Error::Encode {
                side: "right",
                frame_id: pcm.frame_id.0,
                detail: error.to_string(),
            })?;

        if left.len() != expected {
            return Err(MediaLc3Error::EncodedLength {
                side: "left",
                frame_id: pcm.frame_id.0,
                expected,
                actual: left.len(),
            });
        }
        if right.len() != expected {
            return Err(MediaLc3Error::EncodedLength {
                side: "right",
                frame_id: pcm.frame_id.0,
                expected,
                actual: right.len(),
            });
        }

        Ok(EncodedStereoFrame {
            frame_id: pcm.frame_id,
            left,
            right,
        })
    }
}

pub fn prepare_synthetic_lc3(
    binding: StreamBinding,
    config: SyntheticToneConfig,
) -> Result<Vec<EncodedLc3StereoFrame>, MediaLc3Error> {
    if binding.topology != StreamTopology::StereoSplit {
        return Err(MediaLc3Error::UnsupportedTopology(
            binding.topology,
        ));
    }

    let frame_count = config.frame_count;
    let mut source = SyntheticToneSource::new(config)
        .map_err(MediaLc3Error::SyntheticSource)?;
    let encoder = Lc3StereoEncoder::new(binding.format)?;
    let capacity = usize::try_from(frame_count).map_err(|_| {
        MediaLc3Error::SyntheticSource(
            "synthetic frame count does not fit usize".into(),
        )
    })?;
    let mut frames = Vec::with_capacity(capacity);

    while let Some(pcm) = source.next_frame() {
        frames.push(encoder.encode(&pcm)?);
    }

    Ok(frames)
}

fn stream_config(
    format: LeAudioFormat,
) -> Result<Lc3StreamConfig, MediaLc3Error> {
    if format.sample_rate_hz != SAMPLE_RATE_HZ
        || format.frame_duration_us != FRAME_DURATION_US
    {
        return Err(MediaLc3Error::UnsupportedPcmFormat {
            sample_rate_hz: format.sample_rate_hz,
            frame_duration_us: format.frame_duration_us,
        });
    }
    if format.octets_per_sdu == 0 {
        return Err(MediaLc3Error::InvalidOctetsPerSdu(
            format.octets_per_sdu,
        ));
    }

    let config = Lc3StreamConfig {
        sampling_frequency: format.sample_rate_hz,
        frame_duration: Lc3FrameDuration::TenMs,
        channels: 1,
        octets_per_codec_frame:
            usize::from(format.octets_per_sdu),
        codec_frames_per_sdu: 1,
    };

    debug_assert_eq!(config.frame_samples(), SAMPLES_PER_FRAME);
    Ok(config)
}

#[cfg(test)]
mod tests {
    use bumble_codecs::lc3::Lc3Decoder;

    use crate::architecture::{
        control::{StreamBinding, StreamTopology},
        types::{SessionId, StreamId},
    };

    use super::*;

    fn binding() -> StreamBinding {
        StreamBinding {
            session_id: SessionId(7),
            stream_id: StreamId(11),
            format: LeAudioFormat {
                sample_rate_hz: 48_000,
                frame_duration_us: 10_000,
                octets_per_sdu: 100,
            },
            presentation_delay_us: 40_000,
            topology: StreamTopology::StereoSplit,
        }
    }

    fn tone_power(samples: &[i16], frequency_hz: u32) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (index, &sample) in samples.iter().enumerate() {
            let phase = std::f64::consts::TAU
                * f64::from(frequency_hz)
                * index as f64
                / f64::from(SAMPLE_RATE_HZ);
            re += f64::from(sample) * phase.cos();
            im += f64::from(sample) * phase.sin();
        }
        re * re + im * im
    }

    #[test]
    fn synthetic_lc3_preserves_frame_ids_and_exact_sdu_size() {
        let frames = prepare_synthetic_lc3(
            binding(),
            SyntheticToneConfig::default(),
        )
        .unwrap();

        assert_eq!(frames.len(), 100);
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame.frame_id.0, index as u64);
            assert_eq!(frame.left.len(), 100);
            assert_eq!(frame.right.len(), 100);
        }
        assert_ne!(frames[20].left, frames[20].right);
    }

    #[test]
    fn real_lc3_roundtrip_keeps_low_level_400_600_hz_channel_identity() {
        let binding = binding();
        let frames = prepare_synthetic_lc3(
            binding,
            SyntheticToneConfig::default(),
        )
        .unwrap();

        let config = stream_config(binding.format).unwrap();
        let left = Lc3Decoder::new(config).unwrap();
        let right = Lc3Decoder::new(config).unwrap();
        let mut decoded_left = Vec::new();
        let mut decoded_right = Vec::new();

        for (index, frame) in frames.iter().enumerate() {
            let l = left.decode_sdu(&frame.left).unwrap();
            let r = right.decode_sdu(&frame.right).unwrap();
            assert_eq!(l.len(), SAMPLES_PER_FRAME);
            assert_eq!(r.len(), SAMPLES_PER_FRAME);

            if (20..80).contains(&index) {
                decoded_left.extend(l);
                decoded_right.extend(r);
            }
        }

        let left_400 =
            tone_power(&decoded_left, 400);
        let left_600 =
            tone_power(&decoded_left, 600).max(1.0);
        let right_600 =
            tone_power(&decoded_right, 600);
        let right_400 =
            tone_power(&decoded_right, 400).max(1.0);

        assert!(
            left_400 / left_600 > 4.0,
            "left LC3 roundtrip lost 400 Hz identity"
        );
        assert!(
            right_600 / right_400 > 4.0,
            "right LC3 roundtrip lost 600 Hz identity"
        );
    }

    #[test]
    fn encoder_rejects_format_outside_current_pcm_source_contract() {
        let mut format = binding().format;
        format.sample_rate_hz = 44_100;
        assert_eq!(
            Lc3StereoEncoder::new(format).unwrap_err(),
            MediaLc3Error::UnsupportedPcmFormat {
                sample_rate_hz: 44_100,
                frame_duration_us: 10_000,
            }
        );

        let mut format = binding().format;
        format.octets_per_sdu = 0;
        assert_eq!(
            Lc3StereoEncoder::new(format).unwrap_err(),
            MediaLc3Error::InvalidOctetsPerSdu(0)
        );
    }
}
