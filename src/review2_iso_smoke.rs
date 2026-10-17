use crate::review2_bond_smoke::{
    parse_bond_smoke_args, BondSmokeConfig,
};

const DEFAULT_FRAMES: usize = 100;
const MAX_FRAMES: usize = 500;
const OCTETS_PER_SDU: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSmokeConfig {
    pub bond: BondSmokeConfig,
    pub frames: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoTransportFrame {
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
            "--sfi" | "--ddc" | "--bond-root" | "--name"
            | "--connect-seconds" => {
                let flag = args[index].clone();
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {flag}"))?
                    .clone();
                bond_args.push(flag);
                bond_args.push(value);
                index += 2;
            }
            unknown => {
                return Err(format!(
                    "unknown review2 iso-smoke argument: {unknown}"
                ))
            }
        }
    }

    if !confirmed {
        return Err(
            "--confirm-iso-hil is required for real stored-bond ISO HIL"
                .into(),
        );
    }

    bond_args.push("--confirm-stored-bond-hil".into());
    let bond = parse_bond_smoke_args(&bond_args)?;
    Ok(IsoSmokeConfig { bond, frames })
}

/// Review-2D smoke payloads are deliberately codec-opaque. The smoke validates
/// HCI ISO packetization, controller credits, USB TX and drain/teardown; it does
/// not claim LC3 or acoustic correctness. Product Media/LC3 remains a later
/// architecture phase.
pub fn prepare_transport_frames(
    frame_count: usize,
) -> Result<Vec<IsoTransportFrame>, String> {
    if !(1..=MAX_FRAMES).contains(&frame_count) {
        return Err(format!(
            "frame count must be between 1 and {MAX_FRAMES}"
        ));
    }

    Ok((0..frame_count)
        .map(|frame| {
            let seed = frame as u8;
            let mut left = vec![0u8; OCTETS_PER_SDU];
            let mut right = vec![0u8; OCTETS_PER_SDU];
            for (index, byte) in left.iter_mut().enumerate() {
                *byte = seed.wrapping_add(index as u8);
            }
            for (index, byte) in right.iter_mut().enumerate() {
                *byte = seed
                    .wrapping_add(index as u8)
                    .wrapping_add(0x55);
            }
            IsoTransportFrame { left, right }
        })
        .collect())
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
    fn transport_frames_are_dual_opaque_100_byte_sdus() {
        let frames = prepare_transport_frames(3).unwrap();
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|frame| {
            frame.left.len() == OCTETS_PER_SDU
                && frame.right.len() == OCTETS_PER_SDU
        }));
        assert_ne!(frames[1].left, frames[1].right);
    }
}
