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
                stream::StreamBinding,
            },
            model::set_plan::{
                ControlMemberPlan, ControlSetPlan,
            },
        },
        l1_hci::{
            acl::{
                credit::AclCreditSnapshot,
                security::StoredLeKeyMaterial,
            },
            host::stack::{
                spawn_be200_hci_stack, HciStack, HciStackConfig,
                HciStackExit,
            },
            interface::report::HciReport,
            iso::credit::IsoCreditSnapshot,
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

fn drive_until(
    stack: &HciStack,
    control: &mut ControlPlane,
    condition: &mut Condition,
    scope: ConditionScope,
    timers: &mut TimerBook,
    binding: &mut Option<StreamBinding>,
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

pub(crate) fn read_acl_audit_diagnostic(
    stack: &HciStack,
) -> Result<AclCreditSnapshot, String> {
    stack
        .control()
        .try_read_acl_tx_audit()
        .map_err(|error| {
            format!("ACL audit query rejected: {error:?}")
        })?;

    let deadline = Instant::now() + AUDIT_TIMEOUT;
    loop {
        match stack.reports().try_recv() {
            Ok(HciReport::AclTxAudit(snapshot)) => return Ok(snapshot),
            Ok(_) => {
                // Teardown has already failed when this diagnostic is used.
                // Preserve HCI worker progress but do not feed further reports
                // into a terminal Control generation.
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err("ACL audit query timed out".into());
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err("HCI report lane disconnected".into())
            }
        }
    }
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
        let stop = execute_control(
            &self.condition,
            &mut self.control,
            CoreCommand::StopControl,
        )
        .map_err(|error| format!("StopControl failed: {error:?}"))?;
        apply_and_continue(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            stop.control_step,
        )?;
        drive_until(
            stack,
            &mut self.control,
            &mut self.condition,
            self.scope,
            &mut self.timers,
            &mut self.binding,
            DriveTarget::Idle,
        )
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
}
