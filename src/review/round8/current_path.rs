//! Post-Round8 current-path Control + First Sound HIL.
//!
//! Unlike the Round7 harness, Bluetooth setup and teardown are driven through
//! Core Command/Condition + the current L1-Control facade. Only immutable test
//! parameters and generic hardware/bond loading support are reused.

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use crate::{
    core::{
        command::{execute_control, CoreCommand},
        condition::{interface::ConditionScope, Condition},
        lifecycle::{
            LifecycleSnapshot, LifecycleState, SessionId, SessionState,
            StreamId,
        },
    },
    plane::{
        l1_control::{
            discovery::audio::parse_ase_status_compat,
            engine::{
                facade::ControlPlane,
                machine::ControlTimer,
                step::ControlStep,
            },
            interface::{
                command::ControlIntent,
                condition::{ControlConditionUpdate, ControlState},
                dispatch::{
                    dispatch_control_action, ControlDispatchEffect,
                },
                report::ControlLowerReport,
                stream::{AudioSide, StreamBinding},
            },
            model::set_plan::{
                ControlMemberPlan, ControlSetPlan,
            },
        },
        l1_hci::{
            acl::{
                connection::AclRuntimeSnapshot,
                credit::AclCreditSnapshot,
                security::StoredLeKeyMaterial,
            },
            gatt::transaction::{
                GattOperation, GattReport, GattRequest, GattResult,
                GattValueEvent,
            },
            host::{
                engine::{
                    AclLlFeaturesProbeResult, AclRssiProbeResult,
                },
                stack::{
                spawn_be200_hci_stack, HciStack, HciStackConfig,
                HciStackExit,
            },
            },
            interface::report::HciReport,
            iso::{
                cig_cis::CisRuntimeSnapshot,
                credit::IsoCreditSnapshot,
            },
        },
    },
    platform::firmware::{
        loader::FirmwareLoadOutcome,
        port as be200_firmware_port,
    },
    review::{
        review2::bond::{
            fingerprint_bond_store, load_verified_bond_set,
            ReviewBondMaterial,
        },
        support::first_sound::submit_synthetic_lc3,
    },
    runtime::{
        generation::Generation,
        mailbox::RecvError,
    },
};

const DEFAULT_CONNECT_SECONDS: u64 = 12;
const DEFAULT_FRAMES: usize = 200;
const MAX_FRAMES: usize = 500;
const PRODUCT_GENERATION: Generation = Generation(7);
const PRODUCT_SESSION_ID: SessionId = SessionId(7);
const PRODUCT_STREAM_ID: StreamId = StreamId(11);
const CONTROL_DRIVE_DEADLINE: Duration = Duration::from_secs(90);
const AUDIT_TIMEOUT: Duration = Duration::from_secs(2);
const MEDIA_COMPLETION_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_CLEANUP_GATT_TRACE: usize = 16;
const MAX_CLEANUP_GATT_VALUE_PREFIX: usize = 24;
const RELEASE_TIMEOUT_READBACK_TIMEOUT: Duration = Duration::from_secs(2);
const RELEASE_TIMEOUT_READBACK_LEFT_ID: u64 = u64::MAX - 1;
const RELEASE_TIMEOUT_READBACK_RIGHT_ID: u64 = u64::MAX - 2;
const LIVE_LIVENESS_REQUEST_ID_START: u64 = u64::MAX - 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentPathConfig {
    pub sfi: PathBuf,
    pub ddc: PathBuf,
    pub bond_root: PathBuf,
    pub target_name: String,
    pub connect_timeout: Duration,
    pub frames: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentPathResult {
    pub firmware: FirmwareLoadOutcome,
    pub controller_identity: String,
    pub frames: usize,
    pub audit_before: IsoCreditSnapshot,
    pub audit_after: IsoCreditSnapshot,
    pub bond_unchanged: bool,
    pub exit: HciStackExit,
}

pub fn parse_current_path_args(
    args: &[String],
) -> Result<CurrentPathConfig, String> {
    let mut sfi = None;
    let mut ddc = None;
    let mut bond_root = None;
    let mut target_name = None;
    let mut connect_timeout =
        Duration::from_secs(DEFAULT_CONNECT_SECONDS);
    let mut frames = DEFAULT_FRAMES;
    let mut confirmed = false;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--sfi" | "--ddc" | "--bond-root" | "--name"
            | "--connect-seconds" | "--frames" => {
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
                    "--frames" => {
                        frames = value.parse::<usize>().map_err(|_| {
                            "--frames must be an integer".to_string()
                        })?;
                        if !(1..=MAX_FRAMES).contains(&frames) {
                            return Err(format!(
                                "--frames must be between 1 and {MAX_FRAMES}"
                            ));
                        }
                    }
                    _ => unreachable!(),
                }
                index += 2;
            }
            "--confirm-first-sound-hil" => {
                if confirmed {
                    return Err(
                        "--confirm-first-sound-hil specified more than once"
                            .into(),
                    );
                }
                confirmed = true;
                index += 1;
            }
            unknown => {
                return Err(format!(
                    "unknown round8 control-first-sound argument: {unknown}"
                ))
            }
        }
    }

    if !confirmed {
        return Err(
            "--confirm-first-sound-hil is required; first run with the earbuds out of your ears"
                .into(),
        );
    }

    Ok(CurrentPathConfig {
        sfi: sfi.ok_or_else(|| "--sfi is required".to_string())?,
        ddc: ddc.ok_or_else(|| "--ddc is required".to_string())?,
        bond_root: bond_root
            .ok_or_else(|| "--bond-root is required".to_string())?,
        target_name: target_name
            .ok_or_else(|| "--name is required".to_string())?,
        connect_timeout,
        frames,
    })
}

pub(crate) fn control_plan(material: &ReviewBondMaterial) -> ControlSetPlan {
    ControlSetPlan {
        controller_identity_le: material.controller_identity_le,
        local_irk: material.local_irk,
        members: material
            .members
            .iter()
            .map(|member| ControlMemberPlan {
                identity_address_le: member.identity_address_le,
                identity_address_type: member.address_type,
                peer_irk: member.peer_irk,
                stored_key: StoredLeKeyMaterial {
                    long_term_key: member.long_term_key,
                    encrypted_diversifier:
                        member.encrypted_diversifier,
                    random_number: member.random_number,
                },
            })
            .collect(),
    }
}

struct TimerBook {
    connect_timeout: Duration,
    entries: Vec<(ControlTimer, Instant)>,
}

#[derive(Debug)]
enum CleanupAseAclAuditResult {
    Snapshot(AclCreditSnapshot),
    Unavailable(String),
}

#[derive(Debug)]
struct CleanupAseAclAudit {
    side: AudioSide,
    result: CleanupAseAclAuditResult,
}

#[derive(Debug)]
enum CleanupAseReadbackResult {
    Status {
        ase_id: u8,
        state: u8,
    },
    ValueParseFailed {
        value: Vec<u8>,
        error: String,
    },
    GattFailed(String),
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AseLivenessOutcome {
    Responsive {
        ase_id: u8,
        state: u8,
    },
    ParseFailed {
        value: Vec<u8>,
        error: String,
    },
    GattFailed(String),
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AseLivenessProbe {
    pub(crate) side: AudioSide,
    pub(crate) connection_handle: u16,
    pub(crate) attribute_handle: u16,
    pub(crate) outcome: AseLivenessOutcome,
    pub(crate) before: AclCreditSnapshot,
    pub(crate) after: AclCreditSnapshot,
}

impl AseLivenessProbe {
    pub(crate) fn timed_out(&self) -> bool {
        matches!(self.outcome, AseLivenessOutcome::TimedOut)
    }

    pub(crate) fn summary(&self) -> String {
        let outcome = match &self.outcome {
            AseLivenessOutcome::Responsive { ase_id, state } => {
                format!("responsive(ase_id={ase_id}, state=0x{state:02x})")
            }
            AseLivenessOutcome::ParseFailed { value, error } => {
                format!(
                    "parse-failed(value={value:02x?}, error={error})"
                )
            }
            AseLivenessOutcome::GattFailed(error) => {
                format!("gatt-failed({error})")
            }
            AseLivenessOutcome::TimedOut => "timeout".into(),
        };

        format!(
            "{:?} conn=0x{:04x} ase_handle=0x{:04x} result={} delta_host_submit={} delta_ncp_complete={} delta_mailbox={} delta_usb_submit={} delta_usb_complete={} delta_acl_rx={} delta_att_pdus={} delta_gatt_values={} event_usb_read={}/{}/cancelled:{} pending:{} acl_usb_read={}/{}/cancelled:{} pending:{}",
            self.side,
            self.connection_handle,
            self.attribute_handle,
            outcome,
            self.after.audit_total_submitted
                .saturating_sub(self.before.audit_total_submitted),
            self.after.audit_total_completed
                .saturating_sub(self.before.audit_total_completed),
            self.after.transport_mailbox_enqueued
                .saturating_sub(self.before.transport_mailbox_enqueued),
            self.after.transport_usb_submitted
                .saturating_sub(self.before.transport_usb_submitted),
            self.after.transport_usb_completed
                .saturating_sub(self.before.transport_usb_completed),
            self.after.controller_acl_rx_packets
                .saturating_sub(self.before.controller_acl_rx_packets),
            self.after.att_pdus_drained
                .saturating_sub(self.before.att_pdus_drained),
            self.after.gatt_value_events
                .saturating_sub(self.before.gatt_value_events),
            self.after.transport_event_read_arms,
            self.after.transport_event_read_completions,
            self.after.transport_event_read_cancelled,
            self.after.transport_event_read_pending_estimate(),
            self.after.transport_acl_read_arms,
            self.after.transport_acl_read_completions,
            self.after.transport_acl_read_cancelled,
            self.after.transport_acl_read_pending_estimate(),
        )
    }
}

#[derive(Debug)]
struct CleanupAseReadbackDiagnostic {
    side: AudioSide,
    connection_handle: u16,
    attribute_handle: u16,
    result: CleanupAseReadbackResult,
    post_probe_acl: CleanupAseAclAuditResult,
}

impl TimerBook {
    fn new(connect_timeout: Duration) -> Self {
        Self {
            connect_timeout,
            entries: Vec::new(),
        }
    }

    fn duration(&self, timer: ControlTimer) -> Duration {
        match timer {
            ControlTimer::Attach
            | ControlTimer::ControllerIdentity
            | ControlTimer::CisHostFeature
            | ControlTimer::ScanStart
            | ControlTimer::ScanStop
            | ControlTimer::Privacy
            | ControlTimer::CancelConnect { .. }
            | ControlTimer::SetProjection
            | ControlTimer::CleanupDisconnect(_)
            | ControlTimer::CleanupPrivacy => Duration::from_secs(5),
            ControlTimer::DiscoveryWindow => Duration::from_secs(8),
            ControlTimer::Connect { .. } => self.connect_timeout,
            ControlTimer::Encryption { .. } => Duration::from_secs(10),
            ControlTimer::Gatt { .. }
            | ControlTimer::AseOperation { .. }
            | ControlTimer::CigConfig
            | ControlTimer::CisCreate
            | ControlTimer::IsoPathSetup { .. }
            | ControlTimer::StreamRouteBind { .. }
            | ControlTimer::CleanupStreamRoute { .. }
            | ControlTimer::CleanupIsoPath { .. }
            | ControlTimer::CleanupCis { .. }
            | ControlTimer::CleanupAse { .. }
            | ControlTimer::CleanupCig { .. } => Duration::from_secs(8),
            ControlTimer::StreamingProbe { .. } => {
                Duration::from_millis(50)
            }
            ControlTimer::StreamingDeadline { .. } => {
                Duration::from_secs(6)
            }
        }
    }

    fn arm(&mut self, timer: ControlTimer) {
        self.disarm(timer);
        self.entries
            .push((timer, Instant::now() + self.duration(timer)));
    }

    fn disarm(&mut self, timer: ControlTimer) {
        self.entries.retain(|(value, _)| *value != timer);
    }

    fn pop_expired(&mut self) -> Option<ControlTimer> {
        let now = Instant::now();
        let index = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, (_, deadline))| *deadline <= now)
            .min_by_key(|(_, (_, deadline))| *deadline)
            .map(|(index, _)| index)?;
        Some(self.entries.swap_remove(index).0)
    }
}

fn apply_step(
    stack: &HciStack,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
    step: ControlStep,
) -> Result<(), String> {
    let change = condition.apply_control(scope, step.condition);
    if change.stale {
        return Err(
            "Control ConditionUpdate was rejected as stale".into(),
        );
    }
    *binding = step.stream_binding;

    for action in step.actions {
        match dispatch_control_action(stack.control(), action)
            .map_err(|error| {
                format!("Control action dispatch failed: {error:?}")
            })?
        {
            ControlDispatchEffect::HciSubmitted => {}
            ControlDispatchEffect::ArmTimer(timer) => {
                timers.arm(timer)
            }
            ControlDispatchEffect::DisarmTimer(timer) => {
                timers.disarm(timer)
            }
        }
    }
    Ok(())
}

fn continue_ready_stages(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
) -> Result<(), String> {
    for _ in 0..16 {
        let Some(step) = control
            .continue_ready_stage()
            .map_err(|error| {
                format!("Control ready-stage continuation failed: {error:?}")
            })?
        else {
            return Ok(());
        };
        apply_step(
            stack, condition, scope, timers, binding, step,
        )?;
    }
    Err("Control ready-stage continuation did not quiesce".into())
}

fn apply_and_continue(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
    step: ControlStep,
) -> Result<(), String> {
    apply_step(
        stack, condition, scope, timers, binding, step,
    )?;
    continue_ready_stages(
        stack, control, condition, scope, timers, binding,
    )
}

fn process_report(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
    report: HciReport,
) -> Result<(), String> {
    let step = control
        .handle_lower_report(ControlLowerReport::Hci(report))
        .map_err(|error| {
            format!("Control rejected HCI report: {error:?}")
        })?;
    apply_and_continue(
        stack, control, condition, scope, timers, binding, step,
    )
}

fn process_timer(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
    timer: ControlTimer,
) -> Result<(), String> {
    let intent = if timer == ControlTimer::DiscoveryWindow {
        ControlIntent::DiscoveryWindowElapsed
    } else {
        ControlIntent::TimerExpired(timer)
    };
    let step = control.handle_intent(intent).map_err(|error| {
        format!("Control timer/event failed: {error:?}")
    })?;
    apply_and_continue(
        stack, control, condition, scope, timers, binding, step,
    )
}

#[derive(Clone, Copy)]
enum DriveTarget {
    StreamReadyUnbound,
    StreamBound,
    Idle,
}

fn target_reached(
    condition: &Condition,
    binding: Option<StreamBinding>,
    target: DriveTarget,
) -> bool {
    let control = condition.control_status();
    match target {
        DriveTarget::StreamReadyUnbound => {
            control.state == ControlState::StreamReady
                && control.stream_id.is_none()
        }
        DriveTarget::StreamBound => binding.is_some(),
        DriveTarget::Idle => control.state == ControlState::Idle,
    }
}

fn push_cleanup_gatt_trace(trace: &mut Vec<String>, entry: String) {
    if trace.len() >= MAX_CLEANUP_GATT_TRACE {
        trace.remove(0);
    }
    trace.push(entry);
}

fn capture_cleanup_gatt_report(
    report: &HciReport,
    trace: &mut Vec<String>,
) {
    let HciReport::Gatt(report) = report else {
        return;
    };

    match report {
        GattReport::Complete {
            request_id,
            connection,
            result,
        } => {
            push_cleanup_gatt_trace(
                trace,
                format!(
                    "complete conn=0x{:04x}/g{} request={} result={result:?}",
                    connection.connection_handle,
                    connection.generation,
                    request_id,
                ),
            );
        }
        GattReport::Failed(failure) => {
            push_cleanup_gatt_trace(
                trace,
                format!(
                    "failed conn=0x{:04x}/g{} request={} kind={:?}",
                    failure.connection.connection_handle,
                    failure.connection.generation,
                    failure.request_id,
                    failure.kind,
                ),
            );
        }
        GattReport::Value(event) => {
            let (kind, connection, attribute_handle, value) = match event {
                GattValueEvent::Notification {
                    connection,
                    attribute_handle,
                    value,
                } => ("notification", connection, attribute_handle, value),
                GattValueEvent::Indication {
                    connection,
                    attribute_handle,
                    value,
                } => ("indication", connection, attribute_handle, value),
            };
            let prefix_len = value.len().min(MAX_CLEANUP_GATT_VALUE_PREFIX);
            push_cleanup_gatt_trace(
                trace,
                format!(
                    "{kind} conn=0x{:04x}/g{} handle=0x{:04x} len={} prefix={:02x?}",
                    connection.connection_handle,
                    connection.generation,
                    attribute_handle,
                    value.len(),
                    &value[..prefix_len],
                ),
            );
        }
        GattReport::LateResponseDropped {
            connection,
            request_id,
            opcode,
        } => {
            push_cleanup_gatt_trace(
                trace,
                format!(
                    "late-response conn=0x{:04x}/g{} request={} opcode=0x{:02x}",
                    connection.connection_handle,
                    connection.generation,
                    request_id,
                    opcode,
                ),
            );
        }
        GattReport::UnexpectedPdu {
            connection,
            opcode,
        } => {
            push_cleanup_gatt_trace(
                trace,
                format!(
                    "unexpected-pdu conn=0x{:04x}/g{} opcode=0x{:02x}",
                    connection.connection_handle,
                    connection.generation,
                    opcode,
                ),
            );
        }
        _ => {}
    }
}

fn release_timeout_readback_request_id(side: AudioSide) -> u64 {
    match side {
        AudioSide::Left => RELEASE_TIMEOUT_READBACK_LEFT_ID,
        AudioSide::Right => RELEASE_TIMEOUT_READBACK_RIGHT_ID,
    }
}

fn probe_release_timeout_ase_readback(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
    cleanup_gatt_trace: &mut Vec<String>,
    side: AudioSide,
) -> Result<(u16, u16, CleanupAseReadbackResult), String> {
    let prepared = control
        .prepared_ase_for_review(side)
        .ok_or_else(|| {
            format!("Release timeout readback has no prepared ASE for {side:?}")
        })?;
    let connection = prepared.sink.connection;
    let attribute_handle = prepared.sink.ase_value_handle;
    let request_id = release_timeout_readback_request_id(side);

    stack
        .control()
        .try_begin_gatt(GattRequest {
            request_id,
            connection,
            operation: GattOperation::ReadValue {
                attribute_handle,
            },
        })
        .map_err(|error| {
            format!(
                "Release timeout ASE readback query rejected for {side:?}: {error:?}"
            )
        })?;

    let deadline = Instant::now() + RELEASE_TIMEOUT_READBACK_TIMEOUT;
    let mut deferred = Vec::new();

    let result = loop {
        match stack.reports().try_recv() {
            Ok(HciReport::Gatt(GattReport::Complete {
                request_id: actual_request_id,
                connection: actual_connection,
                result:
                    GattResult::Value {
                        attribute_handle: actual_handle,
                        value,
                    },
            })) if actual_request_id == request_id
                && actual_connection == connection
                && actual_handle == attribute_handle =>
            {
                break match parse_ase_status_compat(&value) {
                    Ok(status) => CleanupAseReadbackResult::Status {
                        ase_id: status.ase_id,
                        state: status.state.0,
                    },
                    Err(error) => {
                        CleanupAseReadbackResult::ValueParseFailed {
                            value,
                            error: format!("{error:?}"),
                        }
                    }
                };
            }
            Ok(HciReport::Gatt(GattReport::Complete {
                request_id: actual_request_id,
                connection: actual_connection,
                result,
            })) if actual_request_id == request_id
                && actual_connection == connection =>
            {
                break CleanupAseReadbackResult::GattFailed(format!(
                    "unexpected result: {result:?}"
                ));
            }
            Ok(HciReport::Gatt(GattReport::Failed(failure)))
                if failure.request_id == request_id
                    && failure.connection == connection =>
            {
                break CleanupAseReadbackResult::GattFailed(format!(
                    "{:?}",
                    failure.kind,
                ));
            }
            Ok(HciReport::Gatt(GattReport::BearerPoisoned {
                connection: actual_connection,
                request_id: actual_request_id,
            })) if actual_request_id == request_id
                && actual_connection == connection =>
            {
                break CleanupAseReadbackResult::GattFailed(
                    "bearer poisoned".into(),
                );
            }
            Ok(report) => {
                capture_cleanup_gatt_report(
                    &report,
                    cleanup_gatt_trace,
                );
                deferred.push(report);
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    break CleanupAseReadbackResult::TimedOut;
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err("HCI report lane disconnected".into())
            }
        }
    };

    // Preserve reliable report ordering without letting a late Release
    // notification trigger teardown actions while the diagnostic read is
    // still in flight.
    for report in deferred {
        process_report(
            stack,
            control,
            condition,
            scope,
            timers,
            binding,
            report,
        )?;
    }

    Ok((connection.connection_handle, attribute_handle, result))
}

fn drive_until(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
    cleanup_acl_audits: &mut Vec<CleanupAseAclAudit>,
    cleanup_ase_readbacks: &mut Vec<CleanupAseReadbackDiagnostic>,
    cleanup_gatt_trace: &mut Vec<String>,
    target: DriveTarget,
) -> Result<(), String> {
    let deadline = Instant::now() + CONTROL_DRIVE_DEADLINE;

    loop {
        if target_reached(condition, *binding, target) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("current-path Control drive exceeded 90 s".into());
        }

        let mut progressed = false;
        loop {
            match stack.reports().try_recv() {
                Ok(report) => {
                    if matches!(target, DriveTarget::Idle) {
                        capture_cleanup_gatt_report(
                            &report,
                            cleanup_gatt_trace,
                        );
                    }
                    process_report(
                        stack, control, condition, scope, timers,
                        binding, report,
                    )?;
                    progressed = true;
                    if target_reached(condition, *binding, target) {
                        return Ok(());
                    }
                }
                Err(RecvError::Empty) => break,
                Err(RecvError::Disconnected) => {
                    return Err("HCI report lane disconnected".into())
                }
            }
        }

        while let Some(timer) = timers.pop_expired() {
            if let ControlTimer::CleanupAse { side } = timer {
                // Capture ACL state before this timeout is reduced into
                // transport/CIS/ACL cleanup. The audit command enters the same
                // reliable HCI control lane before any cleanup commands emitted
                // by process_timer(), so later disconnect flushes cannot erase
                // the evidence we are trying to classify.
                let result = read_acl_audit_before_cleanup(
                    stack,
                    control,
                    condition,
                    scope,
                    timers,
                    binding,
                    cleanup_gatt_trace,
                )?;
                cleanup_acl_audits.push(CleanupAseAclAudit {
                    side,
                    result,
                });

                let (
                    connection_handle,
                    attribute_handle,
                    readback_result,
                ) = probe_release_timeout_ase_readback(
                    stack,
                    control,
                    condition,
                    scope,
                    timers,
                    binding,
                    cleanup_gatt_trace,
                    side,
                )?;
                let post_probe_acl = read_acl_audit_before_cleanup(
                    stack,
                    control,
                    condition,
                    scope,
                    timers,
                    binding,
                    cleanup_gatt_trace,
                )?;
                cleanup_ase_readbacks.push(
                    CleanupAseReadbackDiagnostic {
                        side,
                        connection_handle,
                        attribute_handle,
                        result: readback_result,
                        post_probe_acl,
                    },
                );
            }

            process_timer(
                stack, control, condition, scope, timers, binding,
                timer,
            )?;
            progressed = true;
            if target_reached(condition, *binding, target) {
                return Ok(());
            }
        }

        if !progressed {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn read_acl_audit_before_cleanup(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
    cleanup_gatt_trace: &mut Vec<String>,
) -> Result<CleanupAseAclAuditResult, String> {
    if let Err(error) = stack.control().try_read_acl_tx_audit() {
        return Ok(CleanupAseAclAuditResult::Unavailable(format!(
            "query rejected: {error:?}"
        )));
    }

    let deadline = Instant::now() + AUDIT_TIMEOUT;
    loop {
        match stack.reports().try_recv() {
            Ok(HciReport::AclTxAudit(snapshot)) => {
                return Ok(CleanupAseAclAuditResult::Snapshot(snapshot))
            }
            Ok(report) => {
                // This is a live pre-cleanup probe, not a terminal dump.
                // Forward every other reliable HCI report so a late CP/ASE
                // notification cannot be swallowed by diagnostics.
                capture_cleanup_gatt_report(
                    &report,
                    cleanup_gatt_trace,
                );
                process_report(
                    stack, control, condition, scope, timers, binding,
                    report,
                )?;
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Ok(CleanupAseAclAuditResult::Unavailable(
                        "query timed out".into(),
                    ));
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err("HCI report lane disconnected".into())
            }
        }
    }
}

fn format_att_write_signatures(
    writes: [Option<crate::plane::l1_hci::acl::credit::AclAttWriteSignature>; 4],
) -> String {
    let entries = writes
        .into_iter()
        .flatten()
        .map(|write| {
            let prefix_len = usize::from(write.value_prefix_len)
                .min(write.value_prefix.len());
            format!(
                "conn=0x{:04x}/attr=0x{:04x}/len={}/prefix={:02x?}",
                write.connection_handle,
                write.attribute_handle,
                write.value_len,
                &write.value_prefix[..prefix_len],
            )
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        "none".into()
    } else {
        entries.join(",")
    }
}

fn format_acl_snapshot(snapshot: AclCreditSnapshot) -> String {
    format!(
        "bumble_pending={} audit_in_flight={} waiting_estimate={} limit={:?} acl_rx={} att_pdus={} gatt_values={} host_submit={} ncp_complete={} host_retired={} mailbox={} usb_submit={} usb_complete={} event_usb_read={}/{}/cancelled:{} pending:{} acl_usb_read={}/{}/cancelled:{} pending:{} att_writes=[{}]",
        snapshot.bumble_pending,
        snapshot.audit_in_flight,
        snapshot.waiting_estimate(),
        snapshot.audit_limit,
        snapshot.controller_acl_rx_packets,
        snapshot.att_pdus_drained,
        snapshot.gatt_value_events,
        snapshot.audit_total_submitted,
        snapshot.audit_total_completed,
        snapshot.audit_total_retired,
        snapshot.transport_mailbox_enqueued,
        snapshot.transport_usb_submitted,
        snapshot.transport_usb_completed,
        snapshot.transport_event_read_arms,
        snapshot.transport_event_read_completions,
        snapshot.transport_event_read_cancelled,
        snapshot.transport_event_read_pending_estimate(),
        snapshot.transport_acl_read_arms,
        snapshot.transport_acl_read_completions,
        snapshot.transport_acl_read_cancelled,
        snapshot.transport_acl_read_pending_estimate(),
        format_att_write_signatures(snapshot.recent_att_writes),
    )
}

fn format_cleanup_acl_audits(
    baseline: &CleanupAseAclAuditResult,
    audits: &[CleanupAseAclAudit],
) -> String {
    let mut previous = match baseline {
        CleanupAseAclAuditResult::Snapshot(snapshot) => Some(*snapshot),
        CleanupAseAclAuditResult::Unavailable(_) => None,
    };

    let baseline_text = match baseline {
        CleanupAseAclAuditResult::Snapshot(snapshot) => {
            format!("baseline: {}", format_acl_snapshot(*snapshot))
        }
        CleanupAseAclAuditResult::Unavailable(error) => {
            format!("baseline: unavailable({error})")
        }
    };

    let entries = audits
        .iter()
        .map(|audit| match &audit.result {
            CleanupAseAclAuditResult::Snapshot(snapshot) => {
                let interval = previous
                    .map(|before| {
                        format!(
                            " delta_acl_rx={} delta_att_pdus={} delta_gatt_values={} delta_host_submit={} delta_ncp_complete={} delta_host_retired={} delta_mailbox={} delta_usb_submit={} delta_usb_complete={}",
                            snapshot.controller_acl_rx_packets
                                .saturating_sub(before.controller_acl_rx_packets),
                            snapshot.att_pdus_drained
                                .saturating_sub(before.att_pdus_drained),
                            snapshot.gatt_value_events
                                .saturating_sub(before.gatt_value_events),
                            snapshot.audit_total_submitted
                                .saturating_sub(before.audit_total_submitted),
                            snapshot.audit_total_completed
                                .saturating_sub(before.audit_total_completed),
                            snapshot.audit_total_retired
                                .saturating_sub(before.audit_total_retired),
                            snapshot.transport_mailbox_enqueued
                                .saturating_sub(before.transport_mailbox_enqueued),
                            snapshot.transport_usb_submitted
                                .saturating_sub(before.transport_usb_submitted),
                            snapshot.transport_usb_completed
                                .saturating_sub(before.transport_usb_completed),
                        )
                    })
                    .unwrap_or_default();
                previous = Some(*snapshot);
                format!(
                    "{:?}: {}{}",
                    audit.side,
                    format_acl_snapshot(*snapshot),
                    interval,
                )
            }
            CleanupAseAclAuditResult::Unavailable(error) => {
                format!("{:?}: unavailable({error})", audit.side)
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");

    format!("{baseline_text} | {entries}")
}

fn format_cleanup_ase_readbacks(
    readbacks: &[CleanupAseReadbackDiagnostic],
    pre_audits: &[CleanupAseAclAudit],
) -> String {
    readbacks
        .iter()
        .map(|readback| {
            let result = match &readback.result {
                CleanupAseReadbackResult::Status {
                    ase_id,
                    state,
                } => format!(
                    "status(ase_id={ase_id}, state=0x{state:02x})"
                ),
                CleanupAseReadbackResult::ValueParseFailed {
                    value,
                    error,
                } => format!(
                    "parse-failed(value={value:02x?}, error={error})"
                ),
                CleanupAseReadbackResult::GattFailed(error) => {
                    format!("gatt-failed({error})")
                }
                CleanupAseReadbackResult::TimedOut => {
                    "timeout".to_string()
                }
            };

            let probe_delta = pre_audits
                .iter()
                .find(|audit| audit.side == readback.side)
                .and_then(|audit| match (
                    &audit.result,
                    &readback.post_probe_acl,
                ) {
                    (
                        CleanupAseAclAuditResult::Snapshot(before),
                        CleanupAseAclAuditResult::Snapshot(after),
                    ) => Some(format!(
                        " probe_delta_host_submit={} probe_delta_ncp_complete={} probe_delta_mailbox={} probe_delta_usb_submit={} probe_delta_usb_complete={} probe_delta_acl_rx={} probe_delta_att_pdus={}",
                        after.audit_total_submitted
                            .saturating_sub(before.audit_total_submitted),
                        after.audit_total_completed
                            .saturating_sub(before.audit_total_completed),
                        after.transport_mailbox_enqueued
                            .saturating_sub(before.transport_mailbox_enqueued),
                        after.transport_usb_submitted
                            .saturating_sub(before.transport_usb_submitted),
                        after.transport_usb_completed
                            .saturating_sub(before.transport_usb_completed),
                        after.controller_acl_rx_packets
                            .saturating_sub(before.controller_acl_rx_packets),
                        after.att_pdus_drained
                            .saturating_sub(before.att_pdus_drained),
                    )),
                    _ => None,
                })
                .unwrap_or_default();

            format!(
                "{:?}: conn=0x{:04x} ase_handle=0x{:04x} result={}{}",
                readback.side,
                readback.connection_handle,
                readback.attribute_handle,
                result,
                probe_delta,
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn read_iso_audit(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
) -> Result<IsoCreditSnapshot, String> {
    stack
        .control()
        .try_read_iso_tx_audit()
        .map_err(|error| {
            format!("ISO audit query rejected: {error:?}")
        })?;

    let deadline = Instant::now() + AUDIT_TIMEOUT;
    loop {
        match stack.reports().try_recv() {
            Ok(HciReport::IsoTxAudit(snapshot)) => return Ok(snapshot),
            Ok(report) => {
                process_report(
                    stack, control, condition, scope, timers,
                    binding, report,
                )?;
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err("ISO audit query timed out".into());
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err("HCI report lane disconnected".into())
            }
        }
    }
}

fn wait_media_completion(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
    before: &IsoCreditSnapshot,
    frames: usize,
) -> Result<IsoCreditSnapshot, String> {
    let expected = u64::try_from(frames)
        .map_err(|_| "frame count does not fit u64".to_string())?
        .saturating_mul(2);
    let deadline = Instant::now() + MEDIA_COMPLETION_TIMEOUT;

    loop {
        let after = read_iso_audit(
            stack, control, condition, scope, timers, binding,
        )?;
        let submitted = after
            .total_submitted
            .saturating_sub(before.total_submitted);
        let completed = after
            .total_completed
            .saturating_sub(before.total_completed);

        if submitted > expected || completed > expected {
            return Err(format!(
                "ISO audit exceeded expected product traffic: submitted={submitted} completed={completed} expected={expected}"
            ));
        }
        if submitted == expected
            && completed == expected
            && after.in_flight == 0
        {
            return Ok(after);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "ISO completion timeout: submitted={submitted}/{expected} completed={completed}/{expected} in_flight={}",
                after.in_flight
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) struct CurrentPathControlDriver {
    condition: Condition,
    control: ControlPlane,
    scope: ConditionScope,
    timers: TimerBook,
    binding: Option<StreamBinding>,
    cleanup_acl_baseline: Option<CleanupAseAclAuditResult>,
    cleanup_acl_audits: Vec<CleanupAseAclAudit>,
    cleanup_ase_readbacks: Vec<CleanupAseReadbackDiagnostic>,
    cleanup_gatt_trace: Vec<String>,
    next_liveness_request_id: u64,
}

impl CurrentPathControlDriver {
    pub(crate) fn new(
        plan: ControlSetPlan,
        connect_timeout: Duration,
    ) -> Result<Self, String> {
        let lifecycle = LifecycleSnapshot {
            generation: PRODUCT_GENERATION,
            session_id: Some(PRODUCT_SESSION_ID),
            lifecycle: LifecycleState::Running,
            session: SessionState::Preparing,
        };
        let scope = ConditionScope::session(
            PRODUCT_GENERATION,
            PRODUCT_SESSION_ID,
        );
        let condition = Condition::new(lifecycle);
        let control = ControlPlane::new(plan)
            .map_err(|error| {
                format!("Control plan rejected: {error:?}")
            })?;

        Ok(Self {
            condition,
            control,
            scope,
            timers: TimerBook::new(connect_timeout),
            binding: None,
            cleanup_acl_baseline: None,
            cleanup_acl_audits: Vec::new(),
            cleanup_ase_readbacks: Vec::new(),
            cleanup_gatt_trace: Vec::new(),
            next_liveness_request_id: LIVE_LIVENESS_REQUEST_ID_START,
        })
    }

    pub(crate) fn start_and_bind(
        &mut self,
        stack: &HciStack,
    ) -> Result<StreamBinding, String> {
        let start = execute_control(
            &self.condition,
            &mut self.control,
            CoreCommand::StartControl,
        )
        .map_err(|error| format!("StartControl failed: {error:?}"))?;
        apply_and_continue(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            start.control_step,
        )?;

        drive_until(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            &mut self.cleanup_acl_audits,
            &mut self.cleanup_ase_readbacks,
            &mut self.cleanup_gatt_trace,
            DriveTarget::StreamReadyUnbound,
        )?;

        let handoff = execute_control(
            &self.condition,
            &mut self.control,
            CoreCommand::BeginStreamHandoff {
                session_id: PRODUCT_SESSION_ID,
                stream_id: PRODUCT_STREAM_ID,
            },
        )
        .map_err(|error| {
            format!("BeginStreamHandoff failed: {error:?}")
        })?;
        apply_and_continue(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            handoff.control_step,
        )?;
        drive_until(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            &mut self.cleanup_acl_audits,
            &mut self.cleanup_ase_readbacks,
            &mut self.cleanup_gatt_trace,
            DriveTarget::StreamBound,
        )?;

        let binding = self.binding.ok_or_else(|| {
            "Control reached bound target without StreamBinding".to_string()
        })?;
        if binding.session_id != PRODUCT_SESSION_ID
            || binding.stream_id != PRODUCT_STREAM_ID
            || binding.format.sample_rate_hz != 48_000
            || binding.format.frame_duration_us != 10_000
            || binding.format.octets_per_sdu != 100
            || binding.presentation_delay_us != 40_000
        {
            return Err(format!(
                "Control StreamBinding does not match frozen product parameters: {binding:?}"
            ));
        }

        Ok(binding)
    }

    pub(crate) fn control_status(&self) -> ControlConditionUpdate {
        self.condition.control_status()
    }

    /// Review/HIL-only ATT liveness read. A timeout deliberately leaves the
    /// GATT transaction pending; callers must stop this diagnostic run rather
    /// than continue protocol work, because cancelling would poison the fixed
    /// ATT bearer and contaminate subsequent evidence.
    pub(crate) fn probe_ase_liveness(
        &mut self,
        stack: &HciStack,
        side: AudioSide,
        timeout: Duration,
    ) -> Result<AseLivenessProbe, String> {
        let prepared = self
            .control
            .prepared_ase_for_review(side)
            .ok_or_else(|| {
                format!(
                    "ASE liveness probe has no prepared control for {side:?}"
                )
            })?;
        let connection = prepared.sink.connection;
        let attribute_handle = prepared.sink.ase_value_handle;
        let request_id = self.next_liveness_request_id;
        self.next_liveness_request_id = self
            .next_liveness_request_id
            .checked_sub(1)
            .ok_or_else(|| {
                "ASE liveness request ID space exhausted".to_string()
            })?;

        let mut trace = Vec::new();
        let before = match read_acl_audit_before_cleanup(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            &mut trace,
        )? {
            CleanupAseAclAuditResult::Snapshot(snapshot) => snapshot,
            CleanupAseAclAuditResult::Unavailable(error) => {
                return Err(format!(
                    "ASE liveness pre-audit unavailable: {error}"
                ));
            }
        };

        stack
            .control()
            .try_begin_gatt(GattRequest {
                request_id,
                connection,
                operation: GattOperation::ReadValue {
                    attribute_handle,
                },
            })
            .map_err(|error| {
                format!(
                    "ASE liveness ReadValue dispatch failed for {side:?}: {error:?}"
                )
            })?;

        let deadline = Instant::now() + timeout;
        let outcome = loop {
            match stack.reports().try_recv() {
                Ok(HciReport::Gatt(GattReport::Complete {
                    request_id: actual_request_id,
                    connection: actual_connection,
                    result:
                        GattResult::Value {
                            attribute_handle: actual_handle,
                            value,
                        },
                })) if actual_request_id == request_id
                    && actual_connection == connection
                    && actual_handle == attribute_handle =>
                {
                    break match parse_ase_status_compat(&value) {
                        Ok(status) => AseLivenessOutcome::Responsive {
                            ase_id: status.ase_id,
                            state: status.state.0,
                        },
                        Err(error) => AseLivenessOutcome::ParseFailed {
                            value,
                            error: format!("{error:?}"),
                        },
                    };
                }
                Ok(HciReport::Gatt(GattReport::Complete {
                    request_id: actual_request_id,
                    connection: actual_connection,
                    result,
                })) if actual_request_id == request_id
                    && actual_connection == connection =>
                {
                    break AseLivenessOutcome::GattFailed(format!(
                        "unexpected result: {result:?}"
                    ));
                }
                Ok(HciReport::Gatt(GattReport::Failed(failure)))
                    if failure.request_id == request_id
                        && failure.connection == connection =>
                {
                    break AseLivenessOutcome::GattFailed(format!(
                        "{:?}",
                        failure.kind,
                    ));
                }
                Ok(HciReport::Gatt(GattReport::BearerPoisoned {
                    connection: actual_connection,
                    request_id: actual_request_id,
                })) if actual_request_id == request_id
                    && actual_connection == connection =>
                {
                    break AseLivenessOutcome::GattFailed(
                        "bearer poisoned".into(),
                    );
                }
                Ok(report) => {
                    process_report(
                        stack,
                        &mut self.control,
                        &mut self.condition,
                        self.scope,
                        &mut self.timers,
                        &mut self.binding,
                        report,
                    )?;
                }
                Err(RecvError::Empty) => {
                    if Instant::now() >= deadline {
                        break AseLivenessOutcome::TimedOut;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(RecvError::Disconnected) => {
                    return Err("HCI report lane disconnected".into());
                }
            }
        };

        let after = match read_acl_audit_before_cleanup(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            &mut trace,
        )? {
            CleanupAseAclAuditResult::Snapshot(snapshot) => snapshot,
            CleanupAseAclAuditResult::Unavailable(error) => {
                return Err(format!(
                    "ASE liveness post-audit unavailable: {error}"
                ));
            }
        };

        Ok(AseLivenessProbe {
            side,
            connection_handle: connection.connection_handle,
            attribute_handle,
            outcome,
            before,
            after,
        })
    }

    pub(crate) fn read_acl_runtime_snapshot(
        &mut self,
        stack: &HciStack,
    ) -> Result<AclRuntimeSnapshot, String> {
        stack
            .control()
            .try_read_acl_runtime_snapshot()
            .map_err(|error| {
                format!(
                    "ACL runtime snapshot query rejected: {error:?}"
                )
            })?;

        let deadline = Instant::now() + AUDIT_TIMEOUT;
        loop {
            match stack.reports().try_recv() {
                Ok(HciReport::AclRuntimeSnapshot(snapshot)) => {
                    return Ok(snapshot);
                }
                Ok(report) => {
                    process_report(
                        stack,
                        &mut self.control,
                        &mut self.condition,
                        self.scope,
                        &mut self.timers,
                        &mut self.binding,
                        report,
                    )?;
                }
                Err(RecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return Err(
                            "ACL runtime snapshot query timed out".into()
                        );
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(RecvError::Disconnected) => {
                    return Err("HCI report lane disconnected".into());
                }
            }
        }
    }

    pub(crate) fn format_acl_runtime_snapshot(
        snapshot: &AclRuntimeSnapshot,
    ) -> String {
        snapshot
            .connections
            .iter()
            .map(|record| {
                let data_length = record.data_length
                    .map(|length| {
                        format!(
                            "{}/{}/{}/{}",
                            length.max_tx_octets,
                            length.max_tx_time,
                            length.max_rx_octets,
                            length.max_rx_time,
                        )
                    })
                    .unwrap_or_else(|| "none".into());
                let rssi = record.rssi
                    .map(|value| format!("{value}dBm"))
                    .unwrap_or_else(|| "none".into());

                format!(
                    "conn=0x{:04x}/g{} interval={} latency={} supervision={} subrate={} continuation={} phy={}/{} data_len={} rssi={} enc={}/{}",
                    record.connection.connection_handle,
                    record.connection.generation,
                    record.connection_interval,
                    record.peripheral_latency,
                    record.supervision_timeout,
                    record.subrate_factor,
                    record.continuation_number,
                    record.tx_phy,
                    record.rx_phy,
                    data_length,
                    rssi,
                    record.encryption_enabled,
                    record.encryption_key_size,
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub(crate) fn read_cis_runtime_snapshot(
        &mut self,
        stack: &HciStack,
    ) -> Result<CisRuntimeSnapshot, String> {
        stack
            .control()
            .try_read_cis_runtime_snapshot()
            .map_err(|error| {
                format!("CIS runtime snapshot query rejected: {error:?}")
            })?;

        let deadline = Instant::now() + AUDIT_TIMEOUT;
        loop {
            match stack.reports().try_recv() {
                Ok(HciReport::CisRuntimeSnapshot(snapshot)) => {
                    return Ok(snapshot);
                }
                Ok(report) => {
                    process_report(
                        stack,
                        &mut self.control,
                        &mut self.condition,
                        self.scope,
                        &mut self.timers,
                        &mut self.binding,
                        report,
                    )?;
                }
                Err(RecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return Err("CIS runtime snapshot query timed out".into());
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(RecvError::Disconnected) => {
                    return Err("HCI report lane disconnected".into());
                }
            }
        }
    }

    pub(crate) fn probe_controller_rssi(
        &mut self,
        stack: &HciStack,
        side: AudioSide,
        timeout: Duration,
    ) -> Result<String, String> {
        let prepared = self
            .control
            .prepared_ase_for_review(side)
            .ok_or_else(|| {
                format!(
                    "controller RSSI probe has no prepared control for {side:?}"
                )
            })?;
        let connection = prepared.sink.connection;

        stack
            .control()
            .try_read_acl_rssi_probe(connection)
            .map_err(|error| {
                format!(
                    "controller RSSI probe dispatch failed for {side:?}: {error:?}"
                )
            })?;

        let deadline = Instant::now() + timeout;
        loop {
            match stack.reports().try_recv() {
                Ok(HciReport::AclRssiProbe(result)) => {
                    return Ok(match result {
                        AclRssiProbeResult::Alive {
                            connection,
                            rssi,
                        } => format!(
                            "alive conn=0x{:04x}/g{} rssi={}dBm",
                            connection.connection_handle,
                            connection.generation,
                            rssi,
                        ),
                        AclRssiProbeResult::Rejected {
                            connection,
                            status,
                        } => format!(
                            "rejected conn=0x{:04x}/g{} status=0x{status:02x}",
                            connection.connection_handle,
                            connection.generation,
                        ),
                        AclRssiProbeResult::Malformed {
                            connection,
                            parameters_after_status,
                        } => format!(
                            "malformed conn=0x{:04x}/g{} params={:02x?}",
                            connection.connection_handle,
                            connection.generation,
                            parameters_after_status,
                        ),
                        AclRssiProbeResult::HandleMismatch {
                            connection,
                            returned_handle,
                        } => format!(
                            "handle-mismatch expected=0x{:04x}/g{} returned=0x{returned_handle:04x}",
                            connection.connection_handle,
                            connection.generation,
                        ),
                    });
                }
                Ok(report) => {
                    process_report(
                        stack,
                        &mut self.control,
                        &mut self.condition,
                        self.scope,
                        &mut self.timers,
                        &mut self.binding,
                        report,
                    )?;
                }
                Err(RecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return Ok("timeout".into());
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(RecvError::Disconnected) => {
                    return Err("HCI report lane disconnected".into());
                }
            }
        }
    }

    pub(crate) fn probe_peer_ll_features(
        &mut self,
        stack: &HciStack,
        side: AudioSide,
        timeout: Duration,
    ) -> Result<String, String> {
        let prepared = self
            .control
            .prepared_ase_for_review(side)
            .ok_or_else(|| {
                format!(
                    "peer LL feature probe has no prepared control for {side:?}"
                )
            })?;
        let connection = prepared.sink.connection;

        stack
            .control()
            .try_read_acl_ll_features_probe(connection)
            .map_err(|error| {
                format!(
                    "peer LL feature probe dispatch failed for {side:?}: {error:?}"
                )
            })?;

        let deadline = Instant::now() + timeout;
        loop {
            match stack.reports().try_recv() {
                Ok(HciReport::AclLlFeaturesProbe(result)) => {
                    return Ok(match result {
                        AclLlFeaturesProbeResult::Alive {
                            connection,
                            features,
                        } => format!(
                            "alive conn=0x{:04x}/g{} features={:02x?}",
                            connection.connection_handle,
                            connection.generation,
                            features,
                        ),
                        AclLlFeaturesProbeResult::Rejected {
                            connection,
                            status,
                        } => format!(
                            "rejected conn=0x{:04x}/g{} status=0x{status:02x}",
                            connection.connection_handle,
                            connection.generation,
                        ),
                        AclLlFeaturesProbeResult::HandleMismatch {
                            connection,
                            returned_handle,
                        } => format!(
                            "handle-mismatch expected=0x{:04x}/g{} returned=0x{returned_handle:04x}",
                            connection.connection_handle,
                            connection.generation,
                        ),
                    });
                }
                Ok(report) => {
                    process_report(
                        stack,
                        &mut self.control,
                        &mut self.condition,
                        self.scope,
                        &mut self.timers,
                        &mut self.binding,
                        report,
                    )?;
                }
                Err(RecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return Ok("timeout".into());
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(RecvError::Disconnected) => {
                    return Err("HCI report lane disconnected".into());
                }
            }
        }
    }

    pub(crate) fn iso_audit(
        &mut self,
        stack: &HciStack,
    ) -> Result<IsoCreditSnapshot, String> {
        read_iso_audit(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
        )
    }

    pub(crate) fn wait_media_completion(
        &mut self,
        stack: &HciStack,
        before: &IsoCreditSnapshot,
        frames: usize,
    ) -> Result<IsoCreditSnapshot, String> {
        wait_media_completion(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            before,
            frames,
        )
    }

    pub(crate) fn wait_iso_idle(
        &mut self,
        stack: &HciStack,
        timeout: Duration,
    ) -> Result<IsoCreditSnapshot, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let audit = self.iso_audit(stack)?;
            if audit.in_flight == 0 {
                return Ok(audit);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "ISO drain timeout: in_flight={}",
                    audit.in_flight
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn stop(
        &mut self,
        stack: &HciStack,
    ) -> Result<(), String> {
        self.stop_with_group_release_ab(stack, false)
    }

    pub(crate) fn stop_group_release_ab(
        &mut self,
        stack: &HciStack,
    ) -> Result<(), String> {
        self.stop_with_group_release_ab(stack, true)
    }

    fn stop_with_group_release_ab(
        &mut self,
        stack: &HciStack,
        group_release_ab: bool,
    ) -> Result<(), String> {
        self.cleanup_acl_audits.clear();
        self.cleanup_ase_readbacks.clear();
        self.cleanup_gatt_trace.clear();

        let mut baseline_trace = Vec::new();
        self.cleanup_acl_baseline = Some(
            read_acl_audit_before_cleanup(
                stack,
                &mut self.control,
                &mut self.condition,
                self.scope,
                &mut self.timers,
                &mut self.binding,
                &mut baseline_trace,
            )?,
        );

        let stop_step = if group_release_ab {
            self.control
                .begin_group_release_teardown_for_review()
                .map_err(|error| {
                    format!(
                        "group Release A/B StopControl failed: {error:?}"
                    )
                })?
        } else {
            execute_control(
                &self.condition,
                &mut self.control,
                CoreCommand::StopControl,
            )
            .map_err(|error| {
                format!("StopControl failed: {error:?}")
            })?
            .control_step
        };
        apply_and_continue(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            stop_step,
        )?;
        let result = drive_until(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            &mut self.cleanup_acl_audits,
            &mut self.cleanup_ase_readbacks,
            &mut self.cleanup_gatt_trace,
            DriveTarget::Idle,
        );

        match result {
            Ok(()) => Ok(()),
            Err(error) if self.cleanup_acl_audits.is_empty() => Err(error),
            Err(error) => {
                let baseline = self.cleanup_acl_baseline
                    .as_ref()
                    .expect("stop always records ACL baseline");
                let gatt_trace = if self.cleanup_gatt_trace.is_empty() {
                    "none".to_string()
                } else {
                    self.cleanup_gatt_trace.join(" || ")
                };
                let readbacks = if self.cleanup_ase_readbacks.is_empty() {
                    "none".to_string()
                } else {
                    format_cleanup_ase_readbacks(
                        &self.cleanup_ase_readbacks,
                        &self.cleanup_acl_audits,
                    )
                };
                Err(format!(
                    "{error}; pre-cleanup ACL audits: {}; release-timeout ASE readback: {readbacks}; teardown GATT RX trace: {gatt_trace}",
                    format_cleanup_acl_audits(
                        baseline,
                        &self.cleanup_acl_audits,
                    ),
                ))
            },
        }
    }
}

fn run_current_path(
    stack: &HciStack,
    plan: ControlSetPlan,
    connect_timeout: Duration,
    frames: usize,
) -> Result<(IsoCreditSnapshot, IsoCreditSnapshot), String> {
    let mut driver =
        CurrentPathControlDriver::new(plan, connect_timeout)?;
    let binding = driver.start_and_bind(stack)?;
    let audit_before = driver.iso_audit(stack)?;

    submit_synthetic_lc3(stack, binding, frames)?;
    let audit_after =
        driver.wait_media_completion(stack, &audit_before, frames)?;

    driver.stop(stack)?;
    Ok((audit_before, audit_after))
}

#[cfg(windows)]
pub fn execute_current_path(
    config: CurrentPathConfig,
) -> Result<CurrentPathResult, String> {
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
    let run = run_current_path(
        &stack,
        plan,
        config.connect_timeout,
        config.frames,
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

    let (audit_before, audit_after) = run?;
    if !bond_unchanged {
        return Err("bond store fingerprint changed".into());
    }

    Ok(CurrentPathResult {
        firmware,
        controller_identity,
        frames: config.frames,
        audit_before,
        audit_after,
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
            "--connect-seconds".into(),
            "12".into(),
            "--frames".into(),
            "20".into(),
            "--confirm-first-sound-hil".into(),
        ]
    }

    #[test]
    fn parser_keeps_round7_hil_parameters() {
        let config = parse_current_path_args(&args()).unwrap();
        assert_eq!(config.connect_timeout, Duration::from_secs(12));
        assert_eq!(config.frames, 20);
        assert_eq!(config.target_name, "Galaxy Buds3 Pro (2F23)");
    }

    #[test]
    fn parser_requires_explicit_first_sound_consent() {
        let mut value = args();
        value.pop();
        assert!(parse_current_path_args(&value).is_err());
    }

    #[test]
    fn pre_cleanup_acl_audit_format_keeps_side_and_boundary_state() {
        let baseline = CleanupAseAclAuditResult::Snapshot(
            AclCreditSnapshot {
                bumble_pending: 0,
                audit_in_flight: 0,
                audit_limit: Some(6),
                controller_acl_rx_packets: 10,
                att_pdus_drained: 5,
                gatt_value_events: 3,
                audit_total_submitted: 20,
                audit_total_completed: 20,
                audit_total_retired: 0,
                transport_mailbox_enqueued: 20,
                transport_usb_submitted: 20,
                transport_usb_completed: 20,
                transport_event_read_arms: 1,
                transport_event_read_completions: 0,
                transport_event_read_cancelled: 0,
                transport_acl_read_arms: 1,
                transport_acl_read_completions: 0,
                transport_acl_read_cancelled: 0,
                recent_att_writes: [None; 4],
            },
        );
        let text = format_cleanup_acl_audits(
            &baseline,
            &[
                CleanupAseAclAudit {
                    side: AudioSide::Right,
                    result: CleanupAseAclAuditResult::Snapshot(
                        AclCreditSnapshot {
                            bumble_pending: 2,
                            audit_in_flight: 1,
                            audit_limit: Some(6),
                            controller_acl_rx_packets: 12,
                            att_pdus_drained: 7,
                            gatt_value_events: 4,
                audit_total_submitted: 22,
                audit_total_completed: 22,
                audit_total_retired: 0,
                transport_mailbox_enqueued: 22,
                transport_usb_submitted: 22,
                transport_usb_completed: 22,
                transport_event_read_arms: 1,
                transport_event_read_completions: 0,
                transport_event_read_cancelled: 0,
                transport_acl_read_arms: 1,
                transport_acl_read_completions: 0,
                transport_acl_read_cancelled: 0,
                recent_att_writes: [None; 4],
                        },
                    ),
                },
                CleanupAseAclAudit {
                    side: AudioSide::Left,
                    result: CleanupAseAclAuditResult::Unavailable(
                        "query timed out".into(),
                    ),
                },
            ],
        );

        assert!(text.contains(
            "baseline: bumble_pending=0 audit_in_flight=0 waiting_estimate=0 limit=Some(6) acl_rx=10 att_pdus=5 gatt_values=3 host_submit=20 ncp_complete=20 host_retired=0 mailbox=20 usb_submit=20 usb_complete=20 event_usb_read=1/0/cancelled:0 pending:1 acl_usb_read=1/0/cancelled:0 pending:1 att_writes=[none]"
        ));
        assert!(text.contains(
            "Right: bumble_pending=2 audit_in_flight=1 waiting_estimate=1 limit=Some(6) acl_rx=12 att_pdus=7 gatt_values=4 host_submit=22 ncp_complete=22 host_retired=0 mailbox=22 usb_submit=22 usb_complete=22 event_usb_read=1/0/cancelled:0 pending:1 acl_usb_read=1/0/cancelled:0 pending:1 att_writes=[none] delta_acl_rx=2 delta_att_pdus=2 delta_gatt_values=1 delta_host_submit=2 delta_ncp_complete=2 delta_host_retired=0 delta_mailbox=2 delta_usb_submit=2 delta_usb_complete=2"
        ));
        assert!(text.contains("Left: unavailable(query timed out)"));
    }

    #[test]
    fn cleanup_gatt_trace_is_bounded_and_records_value_metadata() {
        let mut trace = Vec::new();
        let report = HciReport::Gatt(GattReport::Value(
            GattValueEvent::Notification {
                connection: crate::plane::l1_hci::acl::connection::ConnectionToken {
                    connection_handle: 0x0801,
                    generation: 9,
                },
                attribute_handle: 0x0046,
                value: vec![1, 2, 3, 4],
            },
        ));

        capture_cleanup_gatt_report(&report, &mut trace);

        assert_eq!(trace.len(), 1);
        assert!(trace[0].contains(
            "notification conn=0x0801/g9 handle=0x0046 len=4"
        ));
        assert!(trace[0].contains("prefix=[01, 02, 03, 04]"));
    }


    #[test]
    fn release_timeout_readback_format_includes_status_and_probe_delta() {
        let before = AclCreditSnapshot {
            bumble_pending: 0,
            audit_in_flight: 0,
            audit_limit: Some(6),
            controller_acl_rx_packets: 10,
            att_pdus_drained: 10,
            gatt_value_events: 3,
            audit_total_submitted: 20,
            audit_total_completed: 20,
            audit_total_retired: 0,
            transport_mailbox_enqueued: 20,
            transport_usb_submitted: 20,
            transport_usb_completed: 20,
            transport_event_read_arms: 1,
            transport_event_read_completions: 0,
            transport_event_read_cancelled: 0,
            transport_acl_read_arms: 1,
            transport_acl_read_completions: 0,
            transport_acl_read_cancelled: 0,
            recent_att_writes: [None; 4],
        };
        let after = AclCreditSnapshot {
            audit_total_submitted: 21,
            audit_total_completed: 21,
            transport_mailbox_enqueued: 21,
            transport_usb_submitted: 21,
            transport_usb_completed: 21,
            controller_acl_rx_packets: 11,
            att_pdus_drained: 11,
            ..before
        };
        let text = format_cleanup_ase_readbacks(
            &[CleanupAseReadbackDiagnostic {
                side: AudioSide::Right,
                connection_handle: 0x0800,
                attribute_handle: 0x0046,
                result: CleanupAseReadbackResult::Status {
                    ase_id: 1,
                    state: 0x06,
                },
                post_probe_acl: CleanupAseAclAuditResult::Snapshot(after),
            }],
            &[CleanupAseAclAudit {
                side: AudioSide::Right,
                result: CleanupAseAclAuditResult::Snapshot(before),
            }],
        );

        assert!(text.contains(
            "Right: conn=0x0800 ase_handle=0x0046 result=status(ase_id=1, state=0x06)"
        ));
        assert!(text.contains("probe_delta_host_submit=1"));
        assert!(text.contains("probe_delta_acl_rx=1"));
    }


    #[test]
    fn ase_liveness_summary_exposes_tx_and_rx_deltas() {
        let before = AclCreditSnapshot {
            bumble_pending: 0,
            audit_in_flight: 0,
            audit_limit: Some(6),
            controller_acl_rx_packets: 10,
            att_pdus_drained: 10,
            gatt_value_events: 4,
            audit_total_submitted: 20,
            audit_total_completed: 20,
            audit_total_retired: 0,
            transport_mailbox_enqueued: 20,
            transport_usb_submitted: 20,
            transport_usb_completed: 20,
            transport_event_read_arms: 1,
            transport_event_read_completions: 0,
            transport_event_read_cancelled: 0,
            transport_acl_read_arms: 1,
            transport_acl_read_completions: 0,
            transport_acl_read_cancelled: 0,
            recent_att_writes: [None; 4],
        };
        let after = AclCreditSnapshot {
            controller_acl_rx_packets: 11,
            att_pdus_drained: 11,
            audit_total_submitted: 21,
            audit_total_completed: 21,
            transport_mailbox_enqueued: 21,
            transport_usb_submitted: 21,
            transport_usb_completed: 21,
            ..before
        };
        let probe = AseLivenessProbe {
            side: AudioSide::Right,
            connection_handle: 0x0800,
            attribute_handle: 0x0046,
            outcome: AseLivenessOutcome::Responsive {
                ase_id: 1,
                state: 0x04,
            },
            before,
            after,
        };

        let text = probe.summary();
        assert!(text.contains("result=responsive(ase_id=1, state=0x04)"));
        assert!(text.contains("delta_host_submit=1"));
        assert!(text.contains("delta_ncp_complete=1"));
        assert!(text.contains("delta_acl_rx=1"));
        assert!(text.contains("delta_att_pdus=1"));
    }

}
