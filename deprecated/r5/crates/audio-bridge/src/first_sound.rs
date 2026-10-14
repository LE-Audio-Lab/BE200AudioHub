//! Bounded, offline-prepared R5.3 signal. No device, filesystem, or Bluetooth I/O.
//!
//! One persistent mono LC3 encoder per ear; every SDU is exactly one 10 ms frame.
//! Pre-encoding removes codec worker rendezvous/allocation from the HCI deadline.
use bumble_codecs::lc3::{Lc3Decoder, Lc3Encoder, Lc3FrameDuration, Lc3StreamConfig};

pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME_US: u32 = 10_000;
pub const FRAME_SAMPLES: usize = 480;
pub const FRAME_BYTES: usize = 100;
pub const MAX_PLAY_SECONDS: u16 = 30;
const PEAK: f64 = 2_048.0; // About -24 dBFS peak. Not a calibrated acoustic level.
const FADE_SAMPLES: usize = 2_400; // 50 ms cosine fade at each end.

#[derive(Clone, Debug)]
pub struct EncodedStereoFrame {
    pub left: [u8; FRAME_BYTES],
    pub right: [u8; FRAME_BYTES],
}

#[derive(Clone, Debug)]
pub struct PreparedStereoTone {
    frames: Vec<EncodedStereoFrame>,
    play_seconds: u16,
}
impl PreparedStereoTone {
    pub fn frames(&self) -> &[EncodedStereoFrame] { &self.frames }
    pub fn play_seconds(&self) -> u16 { self.play_seconds }
}

fn config() -> Lc3StreamConfig {
    Lc3StreamConfig {
        sampling_frequency: SAMPLE_RATE,
        frame_duration: Lc3FrameDuration::TenMs,
        channels: 1,
        octets_per_codec_frame: FRAME_BYTES,
        codec_frames_per_sdu: 1,
    }
}

/// Validate before controller bootstrap, so a malformed CLI cannot touch hardware.
pub fn validate_play_seconds(seconds: u16) -> Result<(), String> {
    if !(1..=MAX_PLAY_SECONDS).contains(&seconds) {
        return Err(format!("play duration must be 1..={MAX_PLAY_SECONDS} seconds"));
    }
    Ok(())
}

fn pcm_frame(frame: usize, total_samples: usize, frequency: u32) -> [i16; FRAME_SAMPLES] {
    let mut pcm = [0i16; FRAME_SAMPLES];
    for (offset, value) in pcm.iter_mut().enumerate() {
        let sample = frame * FRAME_SAMPLES + offset;
        let distance = sample.min(total_samples.saturating_sub(sample + 1));
        let fade = if distance < FADE_SAMPLES {
            0.5 - 0.5 * (std::f64::consts::PI * distance as f64 / FADE_SAMPLES as f64).cos()
        } else { 1.0 };
        // Absolute sample index, not a restarted oscillator at SDU boundaries.
        let phase = std::f64::consts::TAU * frequency as f64 * sample as f64 / SAMPLE_RATE as f64;
        *value = (PEAK * fade * phase.sin()).round() as i16;
    }
    pcm
}

pub fn prepare_first_sound(seconds: u16) -> Result<PreparedStereoTone, String> {
    validate_play_seconds(seconds)?;
    let left = Lc3Encoder::new(config()).map_err(|e| format!("left LC3 init: {e}"))?;
    let right = Lc3Encoder::new(config()).map_err(|e| format!("right LC3 init: {e}"))?;
    let frame_count = usize::from(seconds) * 100;
    let total_samples = frame_count * FRAME_SAMPLES;
    let mut frames = Vec::with_capacity(frame_count);
    for index in 0..frame_count {
        let l = left.encode_sdu(&pcm_frame(index, total_samples, 440))
            .map_err(|e| format!("left LC3 frame {index}: {e}"))?;
        let r = right.encode_sdu(&pcm_frame(index, total_samples, 880))
            .map_err(|e| format!("right LC3 frame {index}: {e}"))?;
        frames.push(EncodedStereoFrame {
            left: l.try_into().map_err(|v: Vec<u8>| format!("left LC3 frame has {} bytes, expected 100", v.len()))?,
            right: r.try_into().map_err(|v: Vec<u8>| format!("right LC3 frame has {} bytes, expected 100", v.len()))?,
        });
    }
    Ok(PreparedStereoTone { frames, play_seconds: seconds })
}

#[derive(Clone, Debug)]
pub struct CodecSelfTest {
    pub stereo_frames: usize,
    pub left_rms: f64,
    pub right_rms: f64,
    pub left_440_over_880: f64,
    pub right_880_over_440: f64,
}

fn tone_power(samples: &[i16], frequency: u32) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &value) in samples.iter().enumerate() {
        let phase = std::f64::consts::TAU * frequency as f64 * n as f64 / SAMPLE_RATE as f64;
        re += f64::from(value) * phase.cos();
        im += f64::from(value) * phase.sin();
    }
    re * re + im * im
}

/// Real encoder/decoder check, entirely offline. Shape-only checks are insufficient.
pub fn first_sound_codec_self_test() -> Result<CodecSelfTest, String> {
    let tone = prepare_first_sound(1)?;
    let left = Lc3Decoder::new(config()).map_err(|e| format!("left decoder: {e}"))?;
    let right = Lc3Decoder::new(config()).map_err(|e| format!("right decoder: {e}"))?;
    let (mut decoded_left, mut decoded_right) = (Vec::new(), Vec::new());
    for (index, frame) in tone.frames.iter().enumerate() {
        let l = left.decode_sdu(&frame.left).map_err(|e| format!("left decode {index}: {e}"))?;
        let r = right.decode_sdu(&frame.right).map_err(|e| format!("right decode {index}: {e}"))?;
        if l.len() != FRAME_SAMPLES || r.len() != FRAME_SAMPLES {
            return Err(format!("decoded PCM length mismatch at frame {index}"));
        }
        // Ignore startup codec delay and both fades in the spectral/energy gate.
        if (15..85).contains(&index) {
            decoded_left.extend(l);
            decoded_right.extend(r);
        }
    }
    let rms = |pcm: &[i16]| -> f64 {
        (pcm.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / pcm.len() as f64).sqrt()
    };
    let report = CodecSelfTest {
        stereo_frames: tone.frames.len(),
        left_rms: rms(&decoded_left), right_rms: rms(&decoded_right),
        left_440_over_880: tone_power(&decoded_left, 440) / tone_power(&decoded_left, 880).max(1.0),
        right_880_over_440: tone_power(&decoded_right, 880) / tone_power(&decoded_right, 440).max(1.0),
    };
    if !(100.0..4_000.0).contains(&report.left_rms)
        || !(100.0..4_000.0).contains(&report.right_rms)
        || report.left_440_over_880 < 10.0 || report.right_880_over_440 < 10.0
    {
        return Err(format!("LC3 round-trip energy/channel-frequency gate failed: {report:?}"));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_duration_and_pcm_envelope() {
        assert!(validate_play_seconds(0).is_err());
        assert!(validate_play_seconds(31).is_err());
        assert!(validate_play_seconds(30).is_ok());
        let first = pcm_frame(0, 48_000, 440);
        let last = pcm_frame(99, 48_000, 440);
        assert_eq!(first[0], 0); assert_eq!(last[479], 0);
        for frame in 0..100 {
            assert!(pcm_frame(frame, 48_000, 440).iter().all(|v| i32::from(*v).abs() <= 2_048));
        }
        assert_ne!(pcm_frame(20, 48_000, 440), pcm_frame(20, 48_000, 880));
    }
    #[test]
    fn real_lc3_dual_mono_roundtrip_preserves_both_channel_tones() {
        let report = first_sound_codec_self_test().unwrap();
        assert_eq!(report.stereo_frames, 100);
    }
}
