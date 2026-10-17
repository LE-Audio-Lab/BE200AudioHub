use bumble_codecs::lc3::{
    Lc3Encoder, Lc3FrameDuration, Lc3StreamConfig,
};

use crate::review2_bond_smoke::{
    parse_bond_smoke_args, BondSmokeConfig,
};

const DEFAULT_FRAMES: usize = 100;
const MAX_FRAMES: usize = 500;
const SAMPLE_RATE_HZ: u32 = 48_000;
const FRAME_SAMPLES: usize = 480;
const OCTETS_PER_SDU: usize = 100;
const TONE_PEAK: f64 = 2_048.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSmokeConfig {
    pub bond: BondSmokeConfig,
    pub frames: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoToneFrame {
    pub left: Vec<u8>,
    pub right: Vec<u8>,
}

pub fn parse_iso_smoke_args(
    args: &[String],
) -> Result<IsoSmokeConfig, String> {
    let mut bond_args = Vec::new();
    let mut frames = DEFAULT_FRAMES;
    let mut confirmed = false;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--frames" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --frames".to_string())?;
                frames = value
                    .parse::<usize>()
                    .map_err(|_| "--frames must be an integer".to_string())?;
                if !(1..=MAX_FRAMES).contains(&frames) {
                    return Err(format!(
                        "--frames must be between 1 and {MAX_FRAMES}"
                    ));
                }
                index += 2;
            }
            "--confirm-iso-hil" => {
                confirmed = true;
                index += 1;
            }
            _ => {
                bond_args.push(args[index].clone());
                if matches!(
                    args[index].as_str(),
                    "--sfi"
                        | "--ddc"
                        | "--bond-root"
                        | "--name"
                        | "--connect-seconds"
                ) {
                    let value = args.get(index + 1).ok_or_else(|| {
                        format!("missing value for {}", args[index])
                    })?;
                    bond_args.push(value.clone());
                    index += 2;
                } else {
                    index += 1;
                }
            }
        }
    }

    if !confirmed {
        return Err(
            "--confirm-iso-hil is required for real stored-bond ISO HIL"
                .into(),
        );
    }

    if !bond_args
        .iter()
        .any(|arg| arg == "--confirm-stored-bond-hil")
    {
        bond_args.push("--confirm-stored-bond-hil".into());
    }

    let bond = parse_bond_smoke_args(&bond_args)?;
    Ok(IsoSmokeConfig { bond, frames })
}

fn lc3_config() -> Lc3StreamConfig {
    Lc3StreamConfig {
        sampling_frequency: SAMPLE_RATE_HZ,
        frame_duration: Lc3FrameDuration::TenMs,
        channels: 1,
        octets_per_codec_frame: OCTETS_PER_SDU,
        codec_frames_per_sdu: 1,
    }
}

fn pcm_frame(
    frame_index: usize,
    total_frames: usize,
    frequency_hz: u32,
) -> Vec<i16> {
    let total_samples = total_frames * FRAME_SAMPLES;
    let first_sample = frame_index * FRAME_SAMPLES;
    let fade_samples = (5 * FRAME_SAMPLES).min(total_samples / 2);

    (0..FRAME_SAMPLES)
        .map(|offset| {
            let sample_index = first_sample + offset;
            let phase = 2.0
                * std::f64::consts::PI
                * f64::from(frequency_hz)
                * sample_index as f64
                / f64::from(SAMPLE_RATE_HZ);

            let fade_in = if fade_samples == 0 {
                1.0
            } else {
                (sample_index as f64 / fade_samples as f64).min(1.0)
            };
            let remaining = total_samples.saturating_sub(sample_index + 1);
            let fade_out = if fade_samples == 0 {
                1.0
            } else {
                (remaining as f64 / fade_samples as f64).min(1.0)
            };
            let gain = fade_in.min(fade_out);
            (phase.sin() * TONE_PEAK * gain).round() as i16
        })
        .collect()
}

pub fn prepare_tone_frames(
    frame_count: usize,
) -> Result<Vec<IsoToneFrame>, String> {
    if !(1..=MAX_FRAMES).contains(&frame_count) {
        return Err(format!(
            "frame count must be between 1 and {MAX_FRAMES}"
        ));
    }

    let left = Lc3Encoder::new(lc3_config())
        .map_err(|error| format!("left LC3 init: {error}"))?;
    let right = Lc3Encoder::new(lc3_config())
        .map_err(|error| format!("right LC3 init: {error}"))?;

    let mut frames = Vec::with_capacity(frame_count);
    for frame_index in 0..frame_count {
        let left_sdu = left
            .encode_sdu(&pcm_frame(
                frame_index,
                frame_count,
                440,
            ))
            .map_err(|error| {
                format!("left LC3 frame {frame_index}: {error}")
            })?;
        let right_sdu = right
            .encode_sdu(&pcm_frame(
                frame_index,
                frame_count,
                880,
            ))
            .map_err(|error| {
                format!("right LC3 frame {frame_index}: {error}")
            })?;

        if left_sdu.len() != OCTETS_PER_SDU
            || right_sdu.len() != OCTETS_PER_SDU
        {
            return Err(format!(
                "LC3 output length mismatch at frame {frame_index}: left={} right={} expected={OCTETS_PER_SDU}",
                left_sdu.len(),
                right_sdu.len(),
            ));
        }

        frames.push(IsoToneFrame {
            left: left_sdu,
            right: right_sdu,
        });
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> Vec<String> {
        vec![
            "--sfi".into(),
            "a.sfi".into(),
            "--ddc".into(),
            "a.ddc".into(),
            "--bond-root".into(),
            "bonding".into(),
            "--name".into(),
            "Galaxy Buds3 Pro (2F23)".into(),
        ]
    }

    #[test]
    fn parser_requires_iso_specific_hil_consent() {
        assert!(parse_iso_smoke_args(&base_args()).is_err());

        let mut args = base_args();
        args.push("--confirm-iso-hil".into());
        let config = parse_iso_smoke_args(&args).unwrap();
        assert_eq!(config.frames, DEFAULT_FRAMES);
    }

    #[test]
    fn parser_accepts_bounded_frame_count() {
        let mut args = base_args();
        args.extend([
            "--frames".into(),
            "25".into(),
            "--confirm-iso-hil".into(),
        ]);
        assert_eq!(parse_iso_smoke_args(&args).unwrap().frames, 25);

        let mut invalid = base_args();
        invalid.extend([
            "--frames".into(),
            "501".into(),
            "--confirm-iso-hil".into(),
        ]);
        assert!(parse_iso_smoke_args(&invalid).is_err());
    }

    #[test]
    fn prepared_tone_is_valid_dual_mono_lc3_shape() {
        let frames = prepare_tone_frames(3).unwrap();
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|frame| {
            frame.left.len() == OCTETS_PER_SDU
                && frame.right.len() == OCTETS_PER_SDU
        }));
        assert_ne!(frames[1].left, frames[1].right);
    }
}
