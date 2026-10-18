//! Round7 synthetic First Sound HIL entry point.
//!
//! This intentionally reuses the sealed Review-2D setup/teardown oracle while
//! replacing only its data plane with the product LC3/StreamClock/Bridge/
//! logical-HCI path. It is not yet the full Core/Lifecycle product runtime.

use crate::review::review2::iso_smoke::{
    parse_iso_smoke_args, IsoSmokeConfig,
};

pub fn parse_first_sound_args(
    args: &[String],
) -> Result<IsoSmokeConfig, String> {
    let mut mapped = Vec::with_capacity(args.len());
    let mut confirmed = false;

    for arg in args {
        match arg.as_str() {
            "--confirm-first-sound-hil" => {
                if confirmed {
                    return Err(
                        "--confirm-first-sound-hil specified more than once"
                            .into(),
                    );
                }
                confirmed = true;
                mapped.push("--confirm-iso-hil".to_string());
            }
            "--confirm-iso-hil" => {
                return Err(
                    "round7 first-sound requires its own --confirm-first-sound-hil consent; --confirm-iso-hil is only for the Review-2D opaque transport smoke"
                        .into(),
                )
            }
            _ => mapped.push(arg.clone()),
        }
    }

    if !confirmed {
        return Err(
            "--confirm-first-sound-hil is required; first run with the earbuds out of your ears"
                .into(),
        );
    }

    parse_iso_smoke_args(&mapped)
}

#[cfg(windows)]
pub fn execute_first_sound(
    config: IsoSmokeConfig,
) -> Result<
    crate::review::review2::iso_smoke::IsoSmokeResult,
    crate::review::review2::iso_smoke::IsoSmokeExecuteError,
> {
    crate::review::review2::iso_smoke::execute_lc3_first_sound(config)
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
    fn parser_requires_first_sound_specific_consent() {
        let error = parse_first_sound_args(&base_args()).unwrap_err();
        assert!(error.contains("--confirm-first-sound-hil"));
    }

    #[test]
    fn parser_rejects_review2_transport_smoke_consent() {
        let mut args = base_args();
        args.push("--confirm-iso-hil".into());

        let error = parse_first_sound_args(&args).unwrap_err();
        assert!(error.contains("--confirm-first-sound-hil"));
        assert!(error.contains("--confirm-iso-hil"));
    }

    #[test]
    fn parser_preserves_existing_bond_and_frame_arguments() {
        let mut args = base_args();
        args.extend([
            "--connect-seconds".into(),
            "12".into(),
            "--frames".into(),
            "37".into(),
            "--confirm-first-sound-hil".into(),
        ]);

        let config = parse_first_sound_args(&args).unwrap();
        assert_eq!(config.frames, 37);
        assert_eq!(config.bond.connect_timeout.as_secs(), 12);
        assert_eq!(
            config.bond.target_name,
            "Galaxy Buds3 Pro (2F23)"
        );
    }

    #[test]
    fn parser_rejects_duplicate_first_sound_consent() {
        let mut args = base_args();
        args.push("--confirm-first-sound-hil".into());
        args.push("--confirm-first-sound-hil".into());

        assert!(parse_first_sound_args(&args).is_err());
    }
}
