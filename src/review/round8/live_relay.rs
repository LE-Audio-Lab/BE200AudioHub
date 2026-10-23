//! Round8 current-path live Windows audio relay HIL.
//!
//! Bluetooth setup/teardown uses the same CurrentPathControlDriver as the
//! current First Sound HIL. Only the Media source is replaced with Windows
//! Process Loopback + Relay PCM boundary + live LC3/ISO worker.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    core::condition::{
        publication::{
            ConditionPublicationInbox, ConditionPublishers,
            MediaConditionPublisher,
        },
        ConditionSnapshot,
    },
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
            planning::iso::StreamLatencyProfile,
        },
        l1_hci::{
            host::stack::{
                spawn_be200_hci_stack_with_condition, HciStack,
                HciStackConditionPublishers, HciStackConfig, HciStackExit,
            },
            iso::credit::IsoCreditSnapshot,
        },
        l1_media::{
            interface::condition::{
                MediaConditionState, MediaConditionUpdate, MediaState,
            },
            live::{
                LiveGain, LivePcmIsoWorker, LivePcmWorkerExit,
                LivePcmWorkerSnapshot,
            },
            telemetry::MediaTelemetrySnapshot,
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
            control_plan, current_path_condition_runtime,
            CurrentPathControlDriver, StereoCisRawDiagnostics,
            StereoIsoTxSyncDiagnostics,
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
const GUI_CONDITION_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);
const GUI_IMAGING_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRelayConfig {
    pub sfi: PathBuf,
    pub ddc: PathBuf,
    pub bond_root: PathBuf,
    pub target_name: String,
    pub connect_timeout: Duration,
    pub live_duration: Duration,
    pub longevity_test: bool,
    pub media_telemetry_hil: bool,
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
    pub latency_profile: StreamLatencyProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveRelayConditionSample {
    pub sequence: u64,
    pub snapshot: ConditionSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveRelayImagingSample {
    pub sequence: u64,
    pub snapshot: ConditionSnapshot,
    pub cis: StereoCisRawDiagnostics,
    pub iso: IsoCreditSnapshot,
    pub tx_sync: Option<StereoIsoTxSyncDiagnostics>,
}

#[derive(Clone)]
pub struct LiveRelayRuntimeControl {
    stop: Arc<AtomicBool>,
    gain: LiveGain,
    condition_sample_sequence: Arc<AtomicU64>,
    latest_condition_sample: Arc<Mutex<Option<LiveRelayConditionSample>>>,
    imaging_sample_sequence: Arc<AtomicU64>,
    latest_imaging_sample: Arc<Mutex<Option<LiveRelayImagingSample>>>,
}

impl LiveRelayRuntimeControl {
    pub fn new(initial_gain: f32) -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            gain: LiveGain::new(initial_gain),
            condition_sample_sequence: Arc::new(AtomicU64::new(0)),
            latest_condition_sample: Arc::new(Mutex::new(None)),
            imaging_sample_sequence: Arc::new(AtomicU64::new(0)),
            latest_imaging_sample: Arc::new(Mutex::new(None)),
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

    pub fn latest_condition_sample(
        &self,
    ) -> Option<LiveRelayConditionSample> {
        self.latest_condition_sample
            .lock()
            .ok()
            .and_then(|sample| *sample)
    }

    pub(crate) fn latest_imaging_sample(
        &self,
    ) -> Option<LiveRelayImagingSample> {
        self.latest_imaging_sample
            .lock()
            .ok()
            .and_then(|sample| sample.clone())
    }

    fn publish_imaging_snapshot(
        &self,
        snapshot: ConditionSnapshot,
        cis: StereoCisRawDiagnostics,
        iso: IsoCreditSnapshot,
        tx_sync: Option<StereoIsoTxSyncDiagnostics>,
    ) {
        let sequence = self
            .imaging_sample_sequence
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if let Ok(mut slot) = self.latest_imaging_sample.lock() {
            *slot = Some(LiveRelayImagingSample {
                sequence,
                snapshot,
                cis,
                iso,
                tx_sync,
            });
        }
    }

    fn publish_condition_snapshot(&self, snapshot: ConditionSnapshot) {
        let sequence = self
            .condition_sample_sequence
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if let Ok(mut slot) = self.latest_condition_sample.lock() {
            *slot = Some(LiveRelayConditionSample {
                sequence,
                snapshot,
            });
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaTelemetryHilResult {
    pub state_updates_observed: u64,
    pub telemetry_updates_observed: u64,
    pub publisher_dropped_updates: u64,
    pub final_condition_epoch: u64,
    pub final_state: MediaConditionState,
    pub final_telemetry: MediaTelemetrySnapshot,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MediaTelemetryHilProgress {
    state_updates_observed: u64,
    telemetry_updates_observed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRelayMediaResult {
    pub source: ProcessLoopbackSourceStats,
    pub capture: ProcessLoopbackCaptureStats,
    pub media: LivePcmWorkerExit,
    pub media_telemetry_hil: Option<MediaTelemetryHilResult>,
    pub audit_before: IsoCreditSnapshot,
    pub audit_after: IsoCreditSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRelayResult {
    pub firmware: FirmwareLoadOutcome,
    pub controller_identity: String,
    pub duration: Duration,
    pub media: LiveRelayMediaResult,
    pub final_condition: ConditionSnapshot,
    pub bond_unchanged: bool,
    pub exit: HciStackExit,
}

struct LiveMediaRun {
    media: LiveRelayMediaResult,
    driver: CurrentPathControlDriver,
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
    let mut media_telemetry_hil = false;
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
            "--media-telemetry-hil" => {
                if media_telemetry_hil {
                    return Err(
                        "--media-telemetry-hil specified more than once"
                            .into(),
                    );
                }
                media_telemetry_hil = true;
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
        media_telemetry_hil,
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

fn drain_media_condition_hil(
    media: &LivePcmIsoWorker,
    progress: &mut MediaTelemetryHilProgress,
) -> Result<(), String> {
    while let Some(update) = media.try_recv_condition_hil_update() {
        match update {
            MediaConditionUpdate::State(_) => {
                progress.state_updates_observed = progress
                    .state_updates_observed
                    .saturating_add(1);
            }
            MediaConditionUpdate::Telemetry(_) => {
                progress.telemetry_updates_observed = progress
                    .telemetry_updates_observed
                    .saturating_add(1);
            }
            MediaConditionUpdate::Final { .. } => {
                return Err(
                    "temporary Media HIL lane received unexpected Final update"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

fn validate_media_condition_hil_final(
    media_exit: &LivePcmWorkerExit,
    driver: &mut CurrentPathControlDriver,
    progress: MediaTelemetryHilProgress,
    publisher_dropped_updates: u64,
) -> Result<MediaTelemetryHilResult, String> {
    if publisher_dropped_updates != 0 {
        return Err(format!(
            "Media telemetry HIL publication lane dropped {publisher_dropped_updates} update(s)"
        ));
    }
    if progress.state_updates_observed == 0 {
        return Err(
            "Media telemetry HIL observed no semantic State publication".into(),
        );
    }
    if progress.telemetry_updates_observed == 0 {
        return Err(
            "Media telemetry HIL observed no coalesced Telemetry publication"
                .into(),
        );
    }

    let MediaConditionUpdate::Final {
        state,
        telemetry,
    } = media_exit.condition_final
    else {
        return Err(
            "Media worker exit did not carry forced Final Condition update"
                .into(),
        );
    };

    driver.reconcile_condition_publications();
    let snapshot = driver.condition_snapshot();
    if snapshot.media.state != state
        || snapshot.media.telemetry != Some(telemetry)
    {
        return Err(
            "production Condition did not materialize the forced Media Final update exactly"
                .into(),
        );
    }
    if state.state != MediaState::Stopped {
        return Err(format!(
            "Media Final state is not Stopped: {:?}",
            state.state
        ));
    }
    if telemetry.timing.produced_frame.is_none()
        || telemetry.timing.intervals_consumed == 0
        || telemetry.bridge.submitted_intervals == 0
    {
        return Err(format!(
            "Media Final telemetry is incomplete: produced_frame={:?} intervals={} bridge_submitted={}",
            telemetry.timing.produced_frame,
            telemetry.timing.intervals_consumed,
            telemetry.bridge.submitted_intervals,
        ));
    }
    if telemetry.timing.trace.capacity != 256
        || telemetry.bridge.trace.capacity != 256
    {
        return Err(format!(
            "Media raw trace capacities do not match frozen policy: timing={} bridge={}",
            telemetry.timing.trace.capacity,
            telemetry.bridge.trace.capacity,
        ));
    }

    Ok(MediaTelemetryHilResult {
        state_updates_observed: progress.state_updates_observed,
        telemetry_updates_observed: progress.telemetry_updates_observed,
        publisher_dropped_updates,
        final_condition_epoch: snapshot.epoch,
        final_state: state,
        final_telemetry: telemetry,
    })
}

fn service_condition_runtime(
    media: &LivePcmIsoWorker,
    driver: &mut CurrentPathControlDriver,
    media_telemetry_hil: bool,
    hil_progress: &mut MediaTelemetryHilProgress,
) -> Result<(), String> {
    driver.reconcile_condition_publications();
    if media_telemetry_hil {
        drain_media_condition_hil(media, hil_progress)?;
    }
    Ok(())
}

fn wait_live_duration(
    duration: Duration,
    longevity_test: bool,
    media_telemetry_hil: bool,
    source: &ProcessLoopbackSource,
    media: &LivePcmIsoWorker,
    stack: &HciStack,
    driver: &mut CurrentPathControlDriver,
    audit_origin: &IsoCreditSnapshot,
    hil_progress: &mut MediaTelemetryHilProgress,
) -> Result<(), String> {
    let started = Instant::now();

    if !longevity_test {
        while started.elapsed() < duration {
            service_condition_runtime(
                media,
                driver,
                media_telemetry_hil,
                hil_progress,
            )?;
            let remaining = duration.saturating_sub(started.elapsed());
            thread::sleep(remaining.min(Duration::from_millis(5)));
        }
        service_condition_runtime(
            media,
            driver,
            media_telemetry_hil,
            hil_progress,
        )?;
        return Ok(());
    }
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
        while started.elapsed() < target {
            service_condition_runtime(
                media,
                driver,
                media_telemetry_hil,
                hil_progress,
            )?;
            let remaining = target.saturating_sub(started.elapsed());
            thread::sleep(remaining.min(Duration::from_millis(5)));
        }
        service_condition_runtime(
            media,
            driver,
            media_telemetry_hil,
            hil_progress,
        )?;

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
    latency_profile: StreamLatencyProfile,
    gain: LiveGain,
    publication: ConditionPublicationInbox,
    media_publisher: MediaConditionPublisher,
    media_telemetry_hil: bool,
    wait: F,
) -> Result<LiveMediaRun, String>
where
    F: FnOnce(
        &ProcessLoopbackSource,
        &LivePcmIsoWorker,
        &HciStack,
        &mut CurrentPathControlDriver,
        &IsoCreditSnapshot,
        &mut MediaTelemetryHilProgress,
    ) -> Result<(), String>,
{
    let mut driver =
        CurrentPathControlDriver::new_with_latency_profile_and_publication(
            plan,
            connect_timeout,
            latency_profile,
            Some(publication),
        )?;
    let binding = driver.start_and_bind(stack)?;
    let audit_before = driver.iso_audit(stack)?;

    let (boundary, producer) = PcmRelayBoundary::relay_defaults();
    let media = LivePcmIsoWorker::start_with_gain_and_condition(
        binding,
        boundary,
        stack.logical_iso_tx(),
        gain,
        Some(media_publisher),
        media_telemetry_hil,
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

    let mut hil_progress = MediaTelemetryHilProgress::default();
    if media_telemetry_hil {
        drain_media_condition_hil(
            &media,
            &mut hil_progress,
        )?;
    }

    let wait_result = wait(
        &source,
        &media,
        stack,
        &mut driver,
        &audit_before,
        &mut hil_progress,
    );

    if media_telemetry_hil {
        drain_media_condition_hil(
            &media,
            &mut hil_progress,
        )?;
    }

    let source_result = source.shutdown();
    let publisher_dropped_updates =
        media.condition_hil_dropped_updates();
    let media_result = media.shutdown();
    driver.reconcile_condition_publications();

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
    let media_telemetry_hil_result = if media_telemetry_hil {
        Some(validate_media_condition_hil_final(
            &media_exit,
            &mut driver,
            hil_progress,
            publisher_dropped_updates,
        )?)
    } else {
        None
    };

    let audit_after = audit_after_result?;
    stop_result?;
    wait_result?;

    Ok(LiveMediaRun {
        media: LiveRelayMediaResult {
            source: source_stats,
            capture: capture_stats,
            media: media_exit,
            media_telemetry_hil: media_telemetry_hil_result,
            audit_before,
            audit_after,
        },
        driver,
    })
}

fn run_live_media(
    stack: &HciStack,
    plan: ControlSetPlan,
    connect_timeout: Duration,
    duration: Duration,
    longevity_test: bool,
    media_telemetry_hil: bool,
    teardown_policy: TeardownPolicy,
    publication: ConditionPublicationInbox,
    media_publisher: MediaConditionPublisher,
) -> Result<LiveMediaRun, String> {
    run_live_media_with_wait(
        stack,
        plan,
        connect_timeout,
        teardown_policy,
        StreamLatencyProfile::Daily,
        LiveGain::unity(),
        publication,
        media_publisher,
        media_telemetry_hil,
        |source, media, stack, driver, audit_before, hil_progress| {
            wait_live_duration(
                duration,
                longevity_test,
                media_telemetry_hil,
                source,
                media,
                stack,
                driver,
                audit_before,
                hil_progress,
            )
        },
    )
}

fn stereo_flight_mismatch(
    cis: StereoCisRawDiagnostics,
    iso: &IsoCreditSnapshot,
) -> bool {
    let (Some(left), Some(right)) = (cis.left, cis.right) else {
        return false;
    };
    let left_flight = iso
        .in_flight_per_handle
        .get(&left.cis_handle)
        .copied()
        .unwrap_or(0);
    let right_flight = iso
        .in_flight_per_handle
        .get(&right.cis_handle)
        .copied()
        .unwrap_or(0);
    left_flight != right_flight
}

fn wait_until_stop(
    control: &LiveRelayRuntimeControl,
    stack: &HciStack,
    driver: &mut CurrentPathControlDriver,
    audit_before: &IsoCreditSnapshot,
) {
    driver.reconcile_condition_publications();
    let initial_snapshot = driver.condition_snapshot();
    control.publish_condition_snapshot(initial_snapshot);
    control.publish_imaging_snapshot(
        initial_snapshot,
        driver.cis_raw_diagnostics(),
        audit_before.clone(),
        None,
    );

    let now = Instant::now();
    let mut next_condition_snapshot_at =
        now + GUI_CONDITION_SNAPSHOT_INTERVAL;
    let mut next_imaging_snapshot_at =
        now + GUI_IMAGING_SNAPSHOT_INTERVAL;
    let mut desync_active = false;

    while !control.stop_requested() {
        driver.reconcile_condition_publications();

        let now = Instant::now();
        if now >= next_imaging_snapshot_at {
            if let Ok(iso) = driver.iso_audit(stack) {
                driver.reconcile_condition_publications();
                let cis = driver.cis_raw_diagnostics();
                let desync = stereo_flight_mismatch(cis, &iso);
                let tx_sync = if desync && !desync_active {
                    Some(
                        driver
                            .stereo_iso_tx_sync(stack)
                            .unwrap_or_else(|error| {
                                StereoIsoTxSyncDiagnostics {
                                    left: Err(error.clone()),
                                    right: Err(error),
                                }
                            }),
                    )
                } else {
                    None
                };
                desync_active = desync;
                driver.reconcile_condition_publications();
                control.publish_imaging_snapshot(
                    driver.condition_snapshot(),
                    cis,
                    iso,
                    tx_sync,
                );
            }
            next_imaging_snapshot_at =
                now + GUI_IMAGING_SNAPSHOT_INTERVAL;
        }

        if now >= next_condition_snapshot_at {
            control.publish_condition_snapshot(
                driver.condition_snapshot(),
            );
            next_condition_snapshot_at =
                now + GUI_CONDITION_SNAPSHOT_INTERVAL;
        }

        thread::sleep(Duration::from_millis(10));
    }

    driver.reconcile_condition_publications();
    control.publish_condition_snapshot(driver.condition_snapshot());
}

fn validate_final_condition(
    snapshot: &ConditionSnapshot,
    exit: HciStackExit,
) -> Result<(), String> {
    use crate::plane::{
        l1_control::interface::condition::ControlState,
        l1_hci::interface::condition::HciHostState,
        l2_transport::interface::condition::TransportState,
    };

    if snapshot.control.state != ControlState::Idle {
        return Err(format!(
            "final Condition Control state is not Idle: {:?}",
            snapshot.control.state,
        ));
    }
    if snapshot.media.state.state != MediaState::Stopped {
        return Err(format!(
            "final Condition Media state is not Stopped: {:?}",
            snapshot.media.state.state,
        ));
    }
    if snapshot.hci.state.state != HciHostState::Detached {
        return Err(format!(
            "final Condition HCI state is not Detached: {:?}; worker={:?}",
            snapshot.hci.state.state,
            exit.hci,
        ));
    }
    if snapshot.transport.state.state != TransportState::Detached {
        return Err(format!(
            "final Condition Transport state is not Detached: {:?}; worker={:?}",
            snapshot.transport.state.state,
            exit.transport,
        ));
    }
    if snapshot.media.telemetry.is_none()
        || snapshot.hci.iso_telemetry.is_none()
        || snapshot.transport.telemetry.is_none()
    {
        return Err(
            "final Condition is missing one or more Plane telemetry projections"
                .into(),
        );
    }

    Ok(())
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

    let (publishers, publication) =
        current_path_condition_runtime();
    let ConditionPublishers {
        hci,
        media,
        transport,
    } = publishers;
    let stack = spawn_be200_hci_stack_with_condition(
        HciStackConfig::default(),
        HciStackConditionPublishers { hci, transport },
    );
    let wait_control = control.clone();
    let run = run_live_media_with_wait(
        &stack,
        plan,
        config.connect_timeout,
        config.teardown_policy,
        config.latency_profile,
        control.gain(),
        publication,
        media,
        false,
        move |_source, _media, stack, driver, audit_before, _hil_progress| {
            wait_until_stop(
                &wait_control,
                stack,
                driver,
                audit_before,
            );
            Ok(())
        },
    );
    let exit = stack.shutdown();

    let bond_unchanged = fingerprint_bond_store(&config.bond_root)
        .map(|after| after == before)
        .map_err(|error| {
            format!(
                "cannot fingerprint bond store after GUI relay: {}",
                error.0
            )
        })?;

    let LiveMediaRun {
        media: _media,
        mut driver,
    } = run?;
    driver.reconcile_condition_publications();
    let final_condition = driver.condition_snapshot();
    control.publish_condition_snapshot(final_condition);
    validate_final_condition(&final_condition, exit)?;

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

    let (publishers, publication) =
        current_path_condition_runtime();
    let ConditionPublishers {
        hci,
        media,
        transport,
    } = publishers;
    let stack = spawn_be200_hci_stack_with_condition(
        HciStackConfig::default(),
        HciStackConditionPublishers { hci, transport },
    );
    let run = run_live_media(
        &stack,
        plan,
        config.connect_timeout,
        config.live_duration,
        config.longevity_test,
        config.media_telemetry_hil,
        config.teardown_policy,
        publication,
        media,
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

    let LiveMediaRun {
        media,
        mut driver,
    } = run?;
    driver.reconcile_condition_publications();
    let final_condition = driver.condition_snapshot();
    validate_final_condition(&final_condition, exit)?;

    if !bond_unchanged {
        return Err("bond store fingerprint changed".into());
    }

    Ok(LiveRelayResult {
        firmware,
        controller_identity,
        duration: config.live_duration,
        media,
        final_condition,
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
        assert!(!config.media_telemetry_hil);
        assert_eq!(config.teardown_policy, TeardownPolicy::Graceful);
    }

    #[test]
    fn parser_accepts_temporary_media_telemetry_hil_flag() {
        let mut value = args();
        value.push("--media-telemetry-hil".into());
        let config = parse_live_relay_args(&value).unwrap();
        assert!(config.media_telemetry_hil);
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
