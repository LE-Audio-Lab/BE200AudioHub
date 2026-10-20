//! Round8 current-path live Windows audio relay HIL.
//!
//! Bluetooth setup/teardown uses the same CurrentPathControlDriver as the
//! current First Sound HIL. Only the Media source is replaced with Windows
//! Process Loopback + Relay PCM boundary + live LC3/ISO worker.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
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
        l1_control::{
            engine::machine::TeardownPolicy,
            model::set_plan::ControlSetPlan,
        },
        l1_hci::{
            host::stack::{
                spawn_be200_hci_stack, HciStack, HciStackConfig,
                HciStackExit,
            },
            iso::credit::IsoCreditSnapshot,
        },
        l1_media::{
            live::{
                LiveGain, LivePcmIsoWorker, LivePcmWorkerExit,
                LivePcmWorkerSnapshot,
            },
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
const DEFAULT_LONGEVITY_SECONDS: u64 = 300;
const MAX_LONGEVITY_SECONDS: u64 = 86_400;
const LONGEVITY_HEARTBEAT_STEPS: u64 = 10;
const ISO_DRAIN_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRelayConfig {
    pub sfi: PathBuf,
    pub ddc: PathBuf,
    pub bond_root: PathBuf,
    pub target_name: String,
    pub connect_timeout: Duration,
    pub live_duration: Duration,
    pub longevity_test: bool,
    pub teardown_policy: TeardownPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRelaySessionConfig {
    pub sfi: PathBuf,
    pub ddc: PathBuf,
    pub bond_root: PathBuf,
    pub target_name: String,
    pub connect_timeout: Duration,
    pub teardown_policy: TeardownPolicy,
}

#[derive(Clone)]
pub struct LiveRelayRuntimeControl {
    stop: Arc<AtomicBool>,
    gain: LiveGain,
}

impl LiveRelayRuntimeControl {
    pub fn new(initial_gain: f32) -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            gain: LiveGain::new(initial_gain),
        }
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    pub fn gain(&self) -> LiveGain {
        self.gain.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    let mut requested_live_seconds = None;
    let mut longevity_test = false;
    let mut confirmed = false;
    let mut ungraceful_teardown = false;
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
                        if seconds == 0 {
                            return Err(
                                "--seconds must be at least 1".into(),
                            );
                        }
                        requested_live_seconds = Some(seconds);
                    }
                    _ => unreachable!(),
                }
                index += 2;
            }
            "--longevity-test" => {
                if longevity_test {
                    return Err(
                        "--longevity-test specified more than once".into(),
                    );
                }
                longevity_test = true;
                index += 1;
            }
            "--ungraceful-teardown" => {
                if ungraceful_teardown {
                    return Err(
                        "--ungraceful-teardown specified more than once".into(),
                    );
                }
                ungraceful_teardown = true;
                index += 1;
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

    let (default_seconds, max_seconds, mode_name) = if longevity_test {
        (
            DEFAULT_LONGEVITY_SECONDS,
            MAX_LONGEVITY_SECONDS,
            "longevity",
        )
    } else {
        (DEFAULT_LIVE_SECONDS, MAX_LIVE_SECONDS, "normal")
    };
    let live_seconds =
        requested_live_seconds.unwrap_or(default_seconds);
    if live_seconds > max_seconds {
        return Err(format!(
            "--seconds must be in 1..={max_seconds} for {mode_name} mode"
        ));
    }
    let live_duration = Duration::from_secs(live_seconds);

    Ok(LiveRelayConfig {
        sfi: sfi.ok_or_else(|| "--sfi is required".to_string())?,
        ddc: ddc.ok_or_else(|| "--ddc is required".to_string())?,
        bond_root: bond_root
            .ok_or_else(|| "--bond-root is required".to_string())?,
        target_name: target_name
            .ok_or_else(|| "--name is required".to_string())?,
        connect_timeout,
        live_duration,
        longevity_test,
        teardown_policy: if ungraceful_teardown {
            TeardownPolicy::ControllerCleanUngraceful
        } else {
            TeardownPolicy::Graceful
        },
    })
}

fn percentage(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64 * 100.0
    }
}

fn iso_host_gap(
    before: &IsoCreditSnapshot,
    after: &IsoCreditSnapshot,
) -> i128 {
    let submitted = after
        .total_submitted
        .saturating_sub(before.total_submitted) as i128;
    let completed = after
        .total_completed
        .saturating_sub(before.total_completed) as i128;
    let in_flight_delta =
        after.in_flight as i128 - before.in_flight as i128;
    submitted - completed - in_flight_delta
}

fn print_longevity_snapshot(
    step: u64,
    duration: Duration,
    source_before: ProcessLoopbackSourceStats,
    source_after: ProcessLoopbackSourceStats,
    media_before: LivePcmWorkerSnapshot,
    media_after: LivePcmWorkerSnapshot,
    iso_before: &IsoCreditSnapshot,
    iso_after: &IsoCreditSnapshot,
    audit_origin: &IsoCreditSnapshot,
    control: crate::plane::l1_control::interface::condition::ControlConditionUpdate,
) {
    let percent = step as f64
        / LONGEVITY_HEARTBEAT_STEPS as f64
        * 100.0;
    let elapsed_seconds = duration.as_secs_f64()
        * step as f64
        / LONGEVITY_HEARTBEAT_STEPS as f64;

    let source_captured = source_after
        .captured_frames
        .saturating_sub(source_before.captured_frames);
    let source_dropped = source_after
        .dropped_frames
        .saturating_sub(source_before.dropped_frames);

    let media_intervals = media_after
        .pcm
        .intervals_consumed
        .saturating_sub(media_before.pcm.intervals_consumed);
    let underflow_frames = media_after
        .pcm
        .pcm_underflow_frames
        .saturating_sub(media_before.pcm.pcm_underflow_frames);
    let media_requested_frames =
        media_intervals.saturating_mul(480);

    let bridge_submitted = media_after
        .bridge_submitted_intervals
        .saturating_sub(media_before.bridge_submitted_intervals);
    let bridge_dropped = media_after
        .bridge_dropped_backpressure_intervals
        .saturating_sub(
            media_before.bridge_dropped_backpressure_intervals,
        );
    let bridge_attempts =
        bridge_submitted.saturating_add(bridge_dropped);

    let iso_submitted = iso_after
        .total_submitted
        .saturating_sub(iso_before.total_submitted);
    let iso_gap = iso_host_gap(iso_before, iso_after);
    let iso_gap_rate = if iso_submitted == 0 {
        0.0
    } else {
        iso_gap as f64 / iso_submitted as f64 * 100.0
    };

    let total_submitted = iso_after
        .total_submitted
        .saturating_sub(audit_origin.total_submitted);
    let total_completed = iso_after
        .total_completed
        .saturating_sub(audit_origin.total_completed);

    println!(
        "[longevity] elapsed {:.0}/{:.0} s ({:.1}%)",
        elapsed_seconds,
        duration.as_secs_f64(),
        percent,
    );
    println!(
        "  state: control={:?} stream={:?} capture_frames={} media_intervals={} pcm_fill={} hci_submitted={} hci_completed={} hci_in_flight={}",
        control.state,
        control.stream_id,
        source_after.captured_frames,
        media_after.pcm.intervals_consumed,
        media_after.pcm_fill_frames,
        total_submitted,
        total_completed,
        iso_after.in_flight,
    );
    println!(
        "  loss[{}-{}%]: pcm_drop={}/{} ({:.5}%) underflow={}/{} ({:.5}%) bridge_drop={}/{} ({:.5}%) iso_host_gap={:+}/{} ({:+.5}%)",
        (step - 1) * 10,
        step * 10,
        source_dropped,
        source_captured,
        percentage(source_dropped, source_captured),
        underflow_frames,
        media_requested_frames,
        percentage(underflow_frames, media_requested_frames),
        bridge_dropped,
        bridge_attempts,
        percentage(bridge_dropped, bridge_attempts),
        iso_gap,
        iso_submitted,
        iso_gap_rate,
    );
}

fn wait_live_duration(
    duration: Duration,
    longevity_test: bool,
    source: &ProcessLoopbackSource,
    media: &LivePcmIsoWorker,
    stack: &HciStack,
    driver: &mut CurrentPathControlDriver,
    audit_origin: &IsoCreditSnapshot,
) -> Result<(), String> {
    if !longevity_test {
        thread::sleep(duration);
        return Ok(());
    }

    let started = Instant::now();
    let mut source_before = source.stats();
    let mut media_before = media.snapshot();
    let mut iso_before = audit_origin.clone();

    for step in 1..=LONGEVITY_HEARTBEAT_STEPS {
        let target_nanos = duration
            .as_nanos()
            .saturating_mul(step as u128)
            / LONGEVITY_HEARTBEAT_STEPS as u128;
        let target = Duration::from_nanos(
            u64::try_from(target_nanos).unwrap_or(u64::MAX),
        );
        let elapsed = started.elapsed();
        if elapsed < target {
            thread::sleep(target - elapsed);
        }

        let source_after = source.stats();
        let media_after = media.snapshot();
        let iso_after = driver.iso_audit(stack)?;
        let control = driver.control_status();

        print_longevity_snapshot(
            step,
            duration,
            source_before,
            source_after,
            media_before,
            media_after,
            &iso_before,
            &iso_after,
            audit_origin,
            control,
        );

        source_before = source_after;
        media_before = media_after;
        iso_before = iso_after;
    }

    Ok(())
}

fn run_live_media_with_wait<F>(
    stack: &HciStack,
    plan: ControlSetPlan,
    connect_timeout: Duration,
    teardown_policy: TeardownPolicy,
    gain: LiveGain,
    wait: F,
) -> Result<LiveRelayMediaResult, String>
where
    F: FnOnce(
        &ProcessLoopbackSource,
        &LivePcmIsoWorker,
        &HciStack,
        &mut CurrentPathControlDriver,
        &IsoCreditSnapshot,
    ) -> Result<(), String>,
{
    let mut driver =
        CurrentPathControlDriver::new(plan, connect_timeout)?;
    let binding = driver.start_and_bind(stack)?;
    let audit_before = driver.iso_audit(stack)?;

    let (boundary, producer) = PcmRelayBoundary::relay_defaults();
    let media = LivePcmIsoWorker::start_with_gain(
        binding,
        boundary,
        stack.logical_iso_tx(),
        gain,
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
                let _ = driver.stop(stack, teardown_policy);
                return Err(format!(
                    "Windows Process Loopback startup failed: {error:?}"
                ));
            }
        };

    let wait_result =
        wait(&source, &media, stack, &mut driver, &audit_before);

    let source_result = source.shutdown();
    let media_result = media.shutdown();

    let audit_after_result =
        driver.wait_iso_idle(stack, ISO_DRAIN_TIMEOUT);
    let stop_result = driver.stop(stack, teardown_policy);

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
    wait_result?;

    Ok(LiveRelayMediaResult {
        source: source_stats,
        capture: capture_stats,
        media: media_exit,
        audit_before,
        audit_after,
    })
}

fn run_live_media(
    stack: &HciStack,
    plan: ControlSetPlan,
    connect_timeout: Duration,
    duration: Duration,
    longevity_test: bool,
    teardown_policy: TeardownPolicy,
) -> Result<LiveRelayMediaResult, String> {
    run_live_media_with_wait(
        stack,
        plan,
        connect_timeout,
        teardown_policy,
        LiveGain::unity(),
        |source, media, stack, driver, audit_before| {
            wait_live_duration(
                duration,
                longevity_test,
                source,
                media,
                stack,
                driver,
                audit_before,
            )
        },
    )
}

fn wait_until_stop(control: &LiveRelayRuntimeControl) {
    while !control.stop_requested() {
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
pub fn execute_live_relay_until_stop(
    config: LiveRelaySessionConfig,
    control: LiveRelayRuntimeControl,
) -> Result<(), String> {
    let before = fingerprint_bond_store(&config.bond_root)
        .map_err(|error| {
            format!(
                "cannot fingerprint bond store before GUI relay: {}",
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
    let plan = control_plan(&material);

    be200_firmware_port::ensure_operational(
        &config.sfi,
        &config.ddc,
    )
    .map_err(|error| {
        format!("firmware bypass failed: {error:?}")
    })?;

    let stack = spawn_be200_hci_stack(HciStackConfig::default());
    let wait_control = control.clone();
    let run = run_live_media_with_wait(
        &stack,
        plan,
        config.connect_timeout,
        config.teardown_policy,
        control.gain(),
        move |_source, _media, _stack, _driver, _audit_before| {
            wait_until_stop(&wait_control);
            Ok(())
        },
    );
    let _exit = stack.shutdown();

    let bond_unchanged = fingerprint_bond_store(&config.bond_root)
        .map(|after| after == before)
        .map_err(|error| {
            format!(
                "cannot fingerprint bond store after GUI relay: {}",
                error.0
            )
        })?;

    run?;
    if !bond_unchanged {
        return Err("bond store fingerprint changed".into());
    }
    Ok(())
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
        config.longevity_test,
        config.teardown_policy,
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
        assert_eq!(config.teardown_policy, TeardownPolicy::Graceful);
    }

    #[test]
    fn parser_accepts_explicit_ungraceful_teardown_workaround() {
        let mut value = args();
        value.push("--ungraceful-teardown".into());
        let config = parse_live_relay_args(&value).unwrap();
        assert_eq!(
            config.teardown_policy,
            TeardownPolicy::ControllerCleanUngraceful
        );
    }

    #[test]
    fn normal_mode_still_rejects_five_minute_duration() {
        let mut value = args();
        let seconds = value
            .iter()
            .position(|arg| arg == "--seconds")
            .unwrap();
        value[seconds + 1] = "300".into();

        let error = parse_live_relay_args(&value).unwrap_err();
        assert!(error.contains("1..=120"));
        assert!(error.contains("normal mode"));
    }

    #[test]
    fn longevity_mode_accepts_five_minutes_regardless_of_flag_order() {
        let mut value = args();
        let seconds = value
            .iter()
            .position(|arg| arg == "--seconds")
            .unwrap();
        value[seconds + 1] = "300".into();
        value.push("--longevity-test".into());

        let config = parse_live_relay_args(&value).unwrap();
        assert!(config.longevity_test);
        assert_eq!(config.live_duration, Duration::from_secs(300));
    }

    #[test]
    fn longevity_mode_defaults_to_five_minutes() {
        let mut value = args();
        let seconds = value
            .iter()
            .position(|arg| arg == "--seconds")
            .unwrap();
        value.drain(seconds..=seconds + 1);
        value.push("--longevity-test".into());

        let config = parse_live_relay_args(&value).unwrap();
        assert!(config.longevity_test);
        assert_eq!(config.live_duration, Duration::from_secs(300));
    }

    #[test]
    fn iso_host_gap_conserves_submitted_completed_and_in_flight() {
        let before = IsoCreditSnapshot {
            total_submitted: 100,
            total_completed: 98,
            in_flight: 2,
            ..IsoCreditSnapshot::default()
        };
        let after = IsoCreditSnapshot {
            total_submitted: 160,
            total_completed: 157,
            in_flight: 3,
            ..IsoCreditSnapshot::default()
        };
        assert_eq!(iso_host_gap(&before, &after), 0);
    }

    #[test]
    fn percentage_handles_zero_denominator() {
        assert_eq!(percentage(0, 0), 0.0);
        assert_eq!(percentage(1, 4), 25.0);
    }

    #[test]
    fn parser_requires_explicit_live_relay_consent() {
        let mut value = args();
        value.pop();
        assert!(parse_live_relay_args(&value).is_err());
    }
}
