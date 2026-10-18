//! Round8 current-path live Windows audio relay HIL.
//!
//! Bluetooth setup/teardown uses the same CurrentPathControlDriver as the
//! current First Sound HIL. Only the Media source is replaced with Windows
//! Process Loopback + Relay PCM boundary + live LC3/ISO worker.

use std::{
    path::PathBuf,
    thread,
    time::Duration,
};

use crate::{
    platform::{
        firmware::{
            loader::FirmwareLoadOutcome,
            port as be200_firmware_port,
        },
        windows::process_loopback::ProcessLoopbackCaptureStats,
    },
    plane::{
        l1_control::model::set_plan::ControlSetPlan,
        l1_hci::{
            host::stack::{
                spawn_be200_hci_stack, HciStack, HciStackConfig,
                HciStackExit,
            },
            iso::credit::IsoCreditSnapshot,
        },
        l1_media::{
            live::{LivePcmIsoWorker, LivePcmWorkerExit},
            pcm::relay_boundary::PcmRelayBoundary,
            source::process_loopback::{
                ProcessLoopbackSource, ProcessLoopbackSourceStats,
            },
        },
    },
    review::{
        review2::bond::{
            fingerprint_bond_store, load_verified_bond_set,
        },
        round8::current_path::{
            control_plan, CurrentPathControlDriver,
        },
    },
};

const DEFAULT_CONNECT_SECONDS: u64 = 12;
const DEFAULT_LIVE_SECONDS: u64 = 10;
const MAX_LIVE_SECONDS: u64 = 120;
const ISO_DRAIN_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRelayConfig {
    pub sfi: PathBuf,
    pub ddc: PathBuf,
    pub bond_root: PathBuf,
    pub target_name: String,
    pub connect_timeout: Duration,
    pub live_duration: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveRelayMediaResult {
    pub source: ProcessLoopbackSourceStats,
    pub capture: ProcessLoopbackCaptureStats,
    pub media: LivePcmWorkerExit,
    pub audit_before: IsoCreditSnapshot,
    pub audit_after: IsoCreditSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRelayResult {
    pub firmware: FirmwareLoadOutcome,
    pub controller_identity: String,
    pub duration: Duration,
    pub media: LiveRelayMediaResult,
    pub bond_unchanged: bool,
    pub exit: HciStackExit,
}

pub fn parse_live_relay_args(
    args: &[String],
) -> Result<LiveRelayConfig, String> {
    let mut sfi = None;
    let mut ddc = None;
    let mut bond_root = None;
    let mut target_name = None;
    let mut connect_timeout =
        Duration::from_secs(DEFAULT_CONNECT_SECONDS);
    let mut live_duration =
        Duration::from_secs(DEFAULT_LIVE_SECONDS);
    let mut confirmed = false;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--sfi" | "--ddc" | "--bond-root" | "--name"
            | "--connect-seconds" | "--seconds" => {
                let flag = args[index].clone();
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                match flag.as_str() {
                    "--sfi" => sfi = Some(PathBuf::from(value)),
                    "--ddc" => ddc = Some(PathBuf::from(value)),
                    "--bond-root" => {
                        bond_root = Some(PathBuf::from(value))
                    }
                    "--name" => {
                        if value.trim().is_empty() {
                            return Err("--name must not be empty".into());
                        }
                        target_name = Some(value.clone());
                    }
                    "--connect-seconds" => {
                        let seconds: u64 = value.parse().map_err(|_| {
                            "--connect-seconds must be an integer".to_string()
                        })?;
                        if !(3..=30).contains(&seconds) {
                            return Err(
                                "--connect-seconds must be in 3..=30".into(),
                            );
                        }
                        connect_timeout = Duration::from_secs(seconds);
                    }
                    "--seconds" => {
                        let seconds: u64 = value.parse().map_err(|_| {
                            "--seconds must be an integer".to_string()
                        })?;
                        if !(1..=MAX_LIVE_SECONDS).contains(&seconds) {
                            return Err(format!(
                                "--seconds must be in 1..={MAX_LIVE_SECONDS}"
                            ));
                        }
                        live_duration = Duration::from_secs(seconds);
                    }
                    _ => unreachable!(),
                }
                index += 2;
            }
            "--confirm-live-relay-hil" => {
                if confirmed {
                    return Err(
                        "--confirm-live-relay-hil specified more than once"
                            .into(),
                    );
                }
                confirmed = true;
                index += 1;
            }
            unknown => {
                return Err(format!(
                    "unknown round8 live-relay argument: {unknown}"
                ))
            }
        }
    }

    if !confirmed {
        return Err(
            "--confirm-live-relay-hil is required; first run at low application volume with the earbuds out of your ears"
                .into(),
        );
    }

    Ok(LiveRelayConfig {
        sfi: sfi.ok_or_else(|| "--sfi is required".to_string())?,
        ddc: ddc.ok_or_else(|| "--ddc is required".to_string())?,
        bond_root: bond_root
            .ok_or_else(|| "--bond-root is required".to_string())?,
        target_name: target_name
            .ok_or_else(|| "--name is required".to_string())?,
        connect_timeout,
        live_duration,
    })
}

fn run_live_media(
    stack: &HciStack,
    plan: ControlSetPlan,
    connect_timeout: Duration,
    duration: Duration,
) -> Result<LiveRelayMediaResult, String> {
    let mut driver =
        CurrentPathControlDriver::new(plan, connect_timeout)?;
    let binding = driver.start_and_bind(stack)?;
    let audit_before = driver.iso_audit(stack)?;

    let (boundary, producer) = PcmRelayBoundary::relay_defaults();
    let media = LivePcmIsoWorker::start(
        binding,
        boundary,
        stack.logical_iso_tx(),
    )
    .map_err(|error| {
        format!("live Media worker startup failed: {error:?}")
    })?;

    let source =
        match ProcessLoopbackSource::start_excluding_current_process_tree(
            producer,
        ) {
            Ok(source) => source,
            Err(error) => {
                let _ = media.shutdown();
                let _ = driver.wait_iso_idle(
                    stack,
                    ISO_DRAIN_TIMEOUT,
                );
                let _ = driver.stop(stack);
                return Err(format!(
                    "Windows Process Loopback startup failed: {error:?}"
                ));
            }
        };

    thread::sleep(duration);

    let source_result = source.shutdown();
    let media_result = media.shutdown();

    let audit_after_result =
        driver.wait_iso_idle(stack, ISO_DRAIN_TIMEOUT);
    let stop_result = driver.stop(stack);

    let (source_stats, capture_stats) = source_result.map_err(
        |error| {
            format!("Windows Process Loopback shutdown failed: {error:?}")
        },
    )?;
    let media_exit = media_result.map_err(|error| {
        format!("live Media worker failed: {error:?}")
    })?;
    let audit_after = audit_after_result?;
    stop_result?;

    Ok(LiveRelayMediaResult {
        source: source_stats,
        capture: capture_stats,
        media: media_exit,
        audit_before,
        audit_after,
    })
}

#[cfg(windows)]
pub fn execute_live_relay(
    config: LiveRelayConfig,
) -> Result<LiveRelayResult, String> {
    let before = fingerprint_bond_store(&config.bond_root)
        .map_err(|error| {
            format!(
                "cannot fingerprint bond store before HIL: {}",
                error.0
            )
        })?;
    let material = load_verified_bond_set(
        &config.bond_root,
        &config.target_name,
    )
    .map_err(|error| {
        format!("cannot load verified stored bond: {}", error.0)
    })?;
    let controller_identity = material.controller_identity.clone();
    let plan = control_plan(&material);

    let firmware = be200_firmware_port::ensure_operational(
        &config.sfi,
        &config.ddc,
    )
    .map_err(|error| {
        format!("firmware bypass failed: {error:?}")
    })?;

    let stack = spawn_be200_hci_stack(HciStackConfig::default());
    let run = run_live_media(
        &stack,
        plan,
        config.connect_timeout,
        config.live_duration,
    );
    let exit = stack.shutdown();

    let bond_unchanged = fingerprint_bond_store(&config.bond_root)
        .map(|after| after == before)
        .map_err(|error| {
            format!(
                "cannot fingerprint bond store after HIL: {}",
                error.0
            )
        })?;

    let media = run?;
    if !bond_unchanged {
        return Err("bond store fingerprint changed".into());
    }

    Ok(LiveRelayResult {
        firmware,
        controller_identity,
        duration: config.live_duration,
        media,
        bond_unchanged,
        exit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Vec<String> {
        vec![
            "--sfi".into(),
            "a.sfi".into(),
            "--ddc".into(),
            "a.ddc".into(),
            "--bond-root".into(),
            "bonding".into(),
            "--name".into(),
            "Galaxy Buds3 Pro (2F23)".into(),
            "--seconds".into(),
            "7".into(),
            "--confirm-live-relay-hil".into(),
        ]
    }

    #[test]
    fn parser_keeps_live_relay_defaults_and_duration_override() {
        let config = parse_live_relay_args(&args()).unwrap();
        assert_eq!(config.connect_timeout, Duration::from_secs(12));
        assert_eq!(config.live_duration, Duration::from_secs(7));
        assert_eq!(config.target_name, "Galaxy Buds3 Pro (2F23)");
    }

    #[test]
    fn parser_requires_explicit_live_relay_consent() {
        let mut value = args();
        value.pop();
        assert!(parse_live_relay_args(&value).is_err());
    }
}
