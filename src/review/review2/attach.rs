use std::path::PathBuf;

use crate::{
    review::support::hci_smoke::{parse_smoke_args, SmokeConfig, SmokeError, SmokeResult},
    platform::firmware::loader::{FirmwareLoadError, FirmwareLoadOutcome},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HciSmokeConfig {
    pub sfi: PathBuf,
    pub ddc: PathBuf,
    pub smoke: SmokeConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HciSmokeResult {
    pub firmware: FirmwareLoadOutcome,
    pub hci: SmokeResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HciSmokeError {
    Firmware(FirmwareLoadError),
    Hci(SmokeError),
}

#[cfg(windows)]
pub fn execute_hci_smoke(config: HciSmokeConfig) -> Result<HciSmokeResult, HciSmokeError> {
    let firmware = crate::platform::firmware::port::ensure_operational(
        &config.sfi,
        &config.ddc,
    )
    .map_err(HciSmokeError::Firmware)?;

    let hci = crate::review::support::hci_smoke::run_be200_smoke(config.smoke)
        .map_err(HciSmokeError::Hci)?;

    Ok(HciSmokeResult { firmware, hci })
}

pub fn parse_hci_smoke_args(args: &[String]) -> Result<HciSmokeConfig, String> {
    let mut sfi = None;
    let mut ddc = None;
    let mut smoke_args = Vec::new();
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--sfi" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --sfi".to_string())?;
                if sfi.replace(PathBuf::from(value)).is_some() {
                    return Err("--sfi specified more than once".into());
                }
                index += 2;
            }
            "--ddc" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --ddc".to_string())?;
                if ddc.replace(PathBuf::from(value)).is_some() {
                    return Err("--ddc specified more than once".into());
                }
                index += 2;
            }
            _ => {
                smoke_args.push(args[index].clone());
                if matches!(args[index].as_str(), "--seconds" | "--name") {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| format!("missing value for {}", args[index]))?;
                    smoke_args.push(value.clone());
                    index += 2;
                } else {
                    index += 1;
                }
            }
        }
    }

    let sfi = sfi.ok_or_else(|| "--sfi is required".to_string())?;
    let ddc = ddc.ok_or_else(|| "--ddc is required".to_string())?;
    let smoke = parse_smoke_args(&smoke_args)?;

    Ok(HciSmokeConfig { sfi, ddc, smoke })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn parser_keeps_firmware_paths_outside_hci_smoke_config() {
        let args = vec![
            "--sfi".into(),
            "C:/fw/be200.sfi".into(),
            "--ddc".into(),
            "C:/fw/be200.ddc".into(),
            "--seconds".into(),
            "5".into(),
            "--name".into(),
            "Galaxy Buds3 Pro (2F23)".into(),
        ];

        let config = parse_hci_smoke_args(&args).unwrap();
        assert_eq!(config.sfi, PathBuf::from("C:/fw/be200.sfi"));
        assert_eq!(config.ddc, PathBuf::from("C:/fw/be200.ddc"));
        assert_eq!(config.smoke.scan_duration, Duration::from_secs(5));
        assert_eq!(
            config.smoke.target_name.as_deref(),
            Some("Galaxy Buds3 Pro (2F23)")
        );
    }

    #[test]
    fn parser_requires_both_explicit_firmware_paths() {
        assert!(parse_hci_smoke_args(&[]).is_err());
        assert!(parse_hci_smoke_args(&[
            "--sfi".into(),
            "be200.sfi".into(),
        ])
        .is_err());
    }
}
