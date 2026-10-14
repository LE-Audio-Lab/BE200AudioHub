use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use crate::{
    hci_host::{
        connection::{
            ConnectionToken, DisconnectState, FalConnectRequest,
            FalConnectionState, LeConnectionSetSnapshot,
            LeInitiationParameters, LePeerIdentity,
        },
        engine::{
            ControllerIdentity, ControllerIdentityFailure,
            OperationalAttachState,
        },
        mailbox::{HciReport, HciReportInbox},
        stack::{
            spawn_be200_hci_stack, HciStack, HciStackConfig, HciStackExit,
        },
        privacy::{
            ControllerPrivacyState, ResolvingListConfig, ResolvingPeer,
            DEFAULT_RPA_TIMEOUT_SECONDS,
        },
        security::{
            StoredEncryptionRequest, StoredEncryptionState,
            StoredLeKeyMaterial,
        },
    },
    platform::{
        be200_firmware_loader::FirmwareLoadOutcome,
        be200_firmware_port,
    },
    review2_bond::{
        fingerprint_bond_store, load_verified_bond_set, ReviewBondMaterial,
        ReviewBondMember,
    },
    runtime::mailbox::{RecvError, SendError},
};

const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondSmokeConfig {
    pub sfi: PathBuf,
    pub ddc: PathBuf,
    pub bond_root: PathBuf,
    pub target_name: String,
    pub connect_timeout: Duration,
}

pub fn parse_bond_smoke_args(
    args: &[String],
) -> Result<BondSmokeConfig, String> {
    let mut sfi = None;
    let mut ddc = None;
    let mut bond_root = None;
    let mut target_name = None;
    let mut connect_timeout =
        Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECONDS);
    let mut confirmed = false;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--sfi" | "--ddc" | "--bond-root" | "--name"
            | "--connect-seconds" => {
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
                                "--connect-seconds must be in 3..=30".into()
                            );
                        }
                        connect_timeout = Duration::from_secs(seconds);
                    }
                    _ => unreachable!(),
                }
                index += 2;
            }
            "--confirm-stored-bond-hil" => {
                confirmed = true;
                index += 1;
            }
            unknown => {
                return Err(format!(
                    "unknown review2 bond-smoke argument: {unknown}"
                ))
            }
        }
    }

    if !confirmed {
        return Err(
            "--confirm-stored-bond-hil is required".into(),
        );
    }

    Ok(BondSmokeConfig {
        sfi: sfi.ok_or_else(|| "--sfi is required".to_string())?,
        ddc: ddc.ok_or_else(|| "--ddc is required".to_string())?,
        bond_root: bond_root
            .ok_or_else(|| "--bond-root is required".to_string())?,
        target_name: target_name
            .ok_or_else(|| "--name is required".to_string())?,
        connect_timeout,
    })
}

#[derive(Clone, PartialEq, Eq)]
pub struct BondSmokePlan {
    pub controller_identity_le: [u8; 6],
    pub local_irk: [u8; 16],
    pub members: Vec<ReviewBondMember>,
}

impl BondSmokePlan {
    pub fn from_material(
        material: ReviewBondMaterial,
    ) -> Result<Self, String> {
        if material.members.len() != 2 {
            return Err(format!(
                "Review-2B dual-ACL HIL requires exactly 2 CSIS-verified members; found {}",
                material.members.len()
            ));
        }

        Ok(Self {
            controller_identity_le: material.controller_identity_le,
            local_irk: material.local_irk,
            members: material.members,
        })
    }

    pub fn privacy_config(&self) -> ResolvingListConfig {
        ResolvingListConfig {
            peers: self
                .members
                .iter()
                .map(|member| ResolvingPeer {
                    identity_address_le: member.identity_address_le,
                    identity_address_type: member.address_type,
                    peer_irk: member.peer_irk,
                })
                .collect(),
            local_irk: self.local_irk,
            rpa_timeout_seconds: DEFAULT_RPA_TIMEOUT_SECONDS,
        }
    }

    pub fn connect_request(&self, index: usize) -> FalConnectRequest {
        let member = &self.members[index];
        FalConnectRequest {
            peer: LePeerIdentity {
                address_le: member.identity_address_le,
                address_type: member.address_type,
            },
            parameters: LeInitiationParameters::default(),
        }
    }

    pub fn encryption_request(
        &self,
        index: usize,
        connection: ConnectionToken,
    ) -> StoredEncryptionRequest {
        let member = &self.members[index];
        StoredEncryptionRequest {
            connection,
            key: StoredLeKeyMaterial {
                long_term_key: member.long_term_key,
                encrypted_diversifier:
                    member.encrypted_diversifier,
                random_number: member.random_number,
            },
        }
    }

    pub fn identity_matches(&self, identity: ControllerIdentity) -> bool {
        identity.public_address_le == self.controller_identity_le
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BondSmokeTracker {
    pub attach: Option<OperationalAttachState>,
    pub controller_identity:
        Option<Result<ControllerIdentity, ControllerIdentityFailure>>,
    pub privacy: Option<ControllerPrivacyState>,
    pub fal: Option<FalConnectionState>,
    pub disconnect: Option<DisconnectState>,
    pub encryption: Option<StoredEncryptionState>,
    pub connection_set: LeConnectionSetSnapshot,
}

impl BondSmokeTracker {
    pub fn observe(&mut self, report: &HciReport) {
        match report {
            HciReport::AttachState(state) => self.attach = Some(*state),
            HciReport::CapabilitiesReady(_) => {}
            HciReport::ControllerIdentity(identity) => {
                self.controller_identity = Some(*identity)
            }
            HciReport::PassiveScanState(_) | HciReport::ScanComplete(_) => {}
            HciReport::ControllerPrivacyState(state) => {
                self.privacy = Some(state.clone())
            }
            HciReport::FalConnectionState(state) => {
                self.fal = Some(state.clone())
            }
            HciReport::DisconnectState(state) => {
                self.disconnect = Some(state.clone())
            }
            HciReport::StoredEncryptionState(state) => {
                self.encryption = Some(*state)
            }
            HciReport::ConnectionSet(snapshot) => {
                self.connection_set = snapshot.clone()
            }
            HciReport::Gatt(_) => {}
        }
    }

    pub fn contains_connection(&self, token: ConnectionToken) -> bool {
        self.connection_set
            .connections
            .iter()
            .any(|connection| connection.token() == token)
    }

    pub fn is_encrypted(&self, token: ConnectionToken) -> bool {
        self.connection_set.connections.iter().any(|connection| {
            connection.token() == token && connection.encryption_enabled != 0
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BondSmokeRunError {
    Timeout(String),
    ReportDisconnected,
    ControlBackpressure(String),
    AttachFailed(String),
    ControllerIdentity(ControllerIdentityFailure),
    ControllerIdentityMismatch {
        expected: [u8; 6],
        actual: [u8; 6],
    },
    PrivacyFailed(String),
    ConnectFailed {
        member: String,
        detail: String,
    },
    ConnectTimeout {
        member: String,
    },
    EncryptionFailed {
        member: String,
        detail: String,
    },
    DualAclNotEncrypted,
    DisconnectFailed {
        connection: ConnectionToken,
        detail: String,
    },
    PrivacyCleanupFailed(String),
    CleanupIncomplete,
    BondStoreChanged,
    BondFingerprint(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BondSmokeCleanupSummary {
    pub disconnected: Vec<(ConnectionToken, u8)>,
    pub privacy_cleared: bool,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondSmokeMemberResult {
    pub identity_address: String,
    pub connection: ConnectionToken,
    pub encryption_key_size: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondSmokeResult {
    pub firmware: FirmwareLoadOutcome,
    pub controller_identity: String,
    pub members: Vec<BondSmokeMemberResult>,
    pub bond_unchanged: bool,
    pub cleanup: BondSmokeCleanupSummary,
    pub exit: HciStackExit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BondSmokeExecuteError {
    Preflight(String),
    Runtime {
        primary: BondSmokeRunError,
        cleanup: BondSmokeCleanupSummary,
        bond_unchanged: bool,
        exit: HciStackExit,
    },
}

fn wait_for_report<T>(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
    stage: &str,
    mut predicate: impl FnMut(
        &HciReport,
        &BondSmokeTracker,
    ) -> Option<Result<T, BondSmokeRunError>>,
) -> Result<T, BondSmokeRunError> {
    let deadline = Instant::now() + timeout;
    loop {
        match reports.try_recv() {
            Ok(report) => {
                tracker.observe(&report);
                if let Some(result) = predicate(&report, tracker) {
                    return result;
                }
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err(BondSmokeRunError::Timeout(
                        stage.to_string(),
                    ));
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err(BondSmokeRunError::ReportDisconnected)
            }
        }
    }
}

fn drain_reports(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
) -> Result<(), BondSmokeRunError> {
    loop {
        match reports.try_recv() {
            Ok(report) => tracker.observe(&report),
            Err(RecvError::Empty) => return Ok(()),
            Err(RecvError::Disconnected) => {
                return Err(BondSmokeRunError::ReportDisconnected)
            }
        }
    }
}

fn wait_attach(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
) -> Result<(), BondSmokeRunError> {
    wait_for_report(reports, tracker, timeout, "attach", |report, _| {
        match report {
            HciReport::CapabilitiesReady(_) => Some(Ok(())),
            HciReport::AttachState(
                OperationalAttachState::Failed(failure),
            ) => Some(Err(BondSmokeRunError::AttachFailed(
                format!("{failure:?}"),
            ))),
            _ => None,
        }
    })
}

fn wait_controller_identity(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
) -> Result<ControllerIdentity, BondSmokeRunError> {
    wait_for_report(
        reports,
        tracker,
        timeout,
        "controller identity",
        |report, _| match report {
            HciReport::ControllerIdentity(Ok(identity)) => {
                Some(Ok(*identity))
            }
            HciReport::ControllerIdentity(Err(failure)) => {
                Some(Err(BondSmokeRunError::ControllerIdentity(*failure)))
            }
            _ => None,
        },
    )
}

fn wait_privacy_ready(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
) -> Result<(), BondSmokeRunError> {
    wait_for_report(
        reports,
        tracker,
        timeout,
        "controller privacy",
        |report, _| match report {
            HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            ) => Some(Ok(())),
            HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Failed(failure),
            ) => Some(Err(BondSmokeRunError::PrivacyFailed(
                format!("{failure:?}"),
            ))),
            _ => None,
        },
    )
}

fn wait_privacy_idle(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
) -> Result<(), BondSmokeRunError> {
    wait_for_report(
        reports,
        tracker,
        timeout,
        "privacy cleanup",
        |report, _| match report {
            HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Idle,
            ) => Some(Ok(())),
            HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Failed(failure),
            ) => Some(Err(BondSmokeRunError::PrivacyCleanupFailed(
                format!("{failure:?}"),
            ))),
            _ => None,
        },
    )
}

fn wait_connected(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
    member: &str,
) -> Result<crate::hci_host::connection::LeConnectionSnapshot, BondSmokeRunError> {
    wait_for_report(
        reports,
        tracker,
        timeout,
        &format!("connect {member}"),
        |report, _| match report {
            HciReport::FalConnectionState(
                FalConnectionState::Connected(connection)
                | FalConnectionState::ConnectedAfterCancel(connection),
            ) => Some(Ok(connection.clone())),
            HciReport::FalConnectionState(
                FalConnectionState::Failed(failure),
            ) => Some(Err(BondSmokeRunError::ConnectFailed {
                member: member.to_string(),
                detail: format!("{failure:?}"),
            })),
            _ => None,
        },
    )
}

fn wait_cancel_terminal(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
) -> Result<Option<crate::hci_host::connection::LeConnectionSnapshot>, BondSmokeRunError> {
    wait_for_report(
        reports,
        tracker,
        timeout,
        "connection cancel terminal",
        |report, _| match report {
            HciReport::FalConnectionState(
                FalConnectionState::Connected(connection)
                | FalConnectionState::ConnectedAfterCancel(connection),
            ) => Some(Ok(Some(connection.clone()))),
            HciReport::FalConnectionState(
                FalConnectionState::Failed(_),
            ) => Some(Ok(None)),
            _ => None,
        },
    )
}

fn wait_encrypted(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
    member: &str,
    connection: ConnectionToken,
) -> Result<u8, BondSmokeRunError> {
    wait_for_report(
        reports,
        tracker,
        timeout,
        &format!("encrypt {member}"),
        |report, _| match report {
            HciReport::StoredEncryptionState(
                StoredEncryptionState::Encrypted {
                    connection: actual,
                    encryption_key_size,
                },
            ) if *actual == connection => Some(Ok(*encryption_key_size)),
            HciReport::StoredEncryptionState(
                StoredEncryptionState::Failed(failure),
            ) if failure.connection == connection => {
                Some(Err(BondSmokeRunError::EncryptionFailed {
                    member: member.to_string(),
                    detail: format!("{failure:?}"),
                }))
            }
            _ => None,
        },
    )
}

fn dual_encrypted(
    tracker: &BondSmokeTracker,
    expected: [ConnectionToken; 2],
) -> bool {
    tracker.connection_set.connections.len() == 2
        && expected
            .iter()
            .all(|token| tracker.is_encrypted(*token))
}

fn wait_dual_encrypted(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
    expected: [ConnectionToken; 2],
) -> Result<(), BondSmokeRunError> {
    if dual_encrypted(tracker, expected) {
        return Ok(());
    }

    wait_for_report(
        reports,
        tracker,
        timeout,
        "dual encrypted ACL",
        |report, tracker| match report {
            HciReport::ConnectionSet(_)
                if dual_encrypted(tracker, expected) =>
            {
                Some(Ok(()))
            }
            _ => None,
        },
    )
    .map_err(|error| match error {
        BondSmokeRunError::Timeout(_) => {
            BondSmokeRunError::DualAclNotEncrypted
        }
        other => other,
    })
}

fn wait_disconnected(
    reports: &HciReportInbox,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
    connection: ConnectionToken,
) -> Result<u8, BondSmokeRunError> {
    wait_for_report(
        reports,
        tracker,
        timeout,
        "disconnect",
        |report, _| match report {
            HciReport::DisconnectState(
                DisconnectState::Disconnected {
                    connection: actual,
                    event_reason,
                },
            ) if *actual == connection => Some(Ok(*event_reason)),
            HciReport::DisconnectState(
                DisconnectState::Failed(failure),
            ) if failure.connection == connection => {
                Some(Err(BondSmokeRunError::DisconnectFailed {
                    connection,
                    detail: format!("{failure:?}"),
                }))
            }
            _ => None,
        },
    )
}

fn send_control(
    result: Result<(), SendError>,
    stage: &str,
) -> Result<(), BondSmokeRunError> {
    result.map_err(|_| {
        BondSmokeRunError::ControlBackpressure(stage.to_string())
    })
}

fn remember_connection(
    known: &mut Vec<ConnectionToken>,
    connection: ConnectionToken,
) {
    if !known.contains(&connection) {
        known.push(connection);
    }
}

fn run_member(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    plan: &BondSmokePlan,
    index: usize,
    connect_timeout: Duration,
    known_connections: &mut Vec<ConnectionToken>,
) -> Result<BondSmokeMemberResult, BondSmokeRunError> {
    let member = &plan.members[index];

    send_control(
        stack
            .control()
            .try_begin_fal_connection(plan.connect_request(index)),
        &format!("begin FAL connection for {}", member.identity_address),
    )?;

    let connection = match wait_connected(
        stack.reports(),
        tracker,
        connect_timeout,
        &member.identity_address,
    ) {
        Ok(connection) => connection,
        Err(BondSmokeRunError::Timeout(_)) => {
            send_control(
                stack.control().try_cancel_fal_connection(),
                &format!(
                    "cancel FAL connection for {}",
                    member.identity_address
                ),
            )?;

            if let Some(late_connection) = wait_cancel_terminal(
                stack.reports(),
                tracker,
                Duration::from_secs(5),
            )? {
                remember_connection(
                    known_connections,
                    late_connection.token(),
                );
                let _ = drain_reports(stack.reports(), tracker);
            }

            return Err(BondSmokeRunError::ConnectTimeout {
                member: member.identity_address.clone(),
            });
        }
        Err(error) => return Err(error),
    };

    let token = connection.token();
    remember_connection(known_connections, token);

    send_control(
        stack.control().try_begin_stored_encryption(
            plan.encryption_request(index, token),
        ),
        &format!("begin stored encryption for {}", member.identity_address),
    )?;

    let encryption_key_size = wait_encrypted(
        stack.reports(),
        tracker,
        Duration::from_secs(10),
        &member.identity_address,
        token,
    )?;

    // EncryptionState is published before the derived ConnectionSet update.
    // Drain that update so the next stage sees current per-handle truth.
    drain_reports(stack.reports(), tracker)?;

    Ok(BondSmokeMemberResult {
        identity_address: member.identity_address.clone(),
        connection: token,
        encryption_key_size,
    })
}

fn cleanup_stack(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
) -> BondSmokeCleanupSummary {
    let mut summary = BondSmokeCleanupSummary::default();

    if let Err(error) = drain_reports(stack.reports(), tracker) {
        summary
            .errors
            .push(format!("pre-cleanup report drain: {error:?}"));
    }

    let mut live: Vec<ConnectionToken> = tracker
        .connection_set
        .connections
        .iter()
        .map(|connection| connection.token())
        .collect();
    live.reverse();

    for connection in live {
        if let Err(error) = stack.control().try_disconnect(connection) {
            summary.errors.push(format!(
                "disconnect submit {connection:?}: {error:?}"
            ));
            continue;
        }

        match wait_disconnected(
            stack.reports(),
            tracker,
            Duration::from_secs(5),
            connection,
        ) {
            Ok(reason) => summary.disconnected.push((connection, reason)),
            Err(error) => summary.errors.push(format!(
                "disconnect {connection:?}: {error:?}"
            )),
        }

        if let Err(error) = drain_reports(stack.reports(), tracker) {
            summary.errors.push(format!(
                "post-disconnect report drain: {error:?}"
            ));
        }
    }

    // Privacy mutation is only valid after resetless attach reached Ready.
    if tracker.attach == Some(OperationalAttachState::Ready) {
        match stack.control().try_clear_controller_privacy() {
            Ok(()) => match wait_privacy_idle(
                stack.reports(),
                tracker,
                Duration::from_secs(5),
            ) {
                Ok(()) => summary.privacy_cleared = true,
                Err(error) => summary
                    .errors
                    .push(format!("privacy cleanup: {error:?}")),
            },
            Err(error) => summary.errors.push(format!(
                "privacy cleanup submit: {error:?}"
            )),
        }
    }

    summary
}

#[cfg(windows)]
pub fn execute_bond_smoke(
    config: BondSmokeConfig,
) -> Result<BondSmokeResult, BondSmokeExecuteError> {
    let before = fingerprint_bond_store(&config.bond_root)
        .map_err(|error| {
            BondSmokeExecuteError::Preflight(format!(
                "cannot fingerprint bond store before HIL: {}",
                error.0
            ))
        })?;

    let material =
        load_verified_bond_set(&config.bond_root, &config.target_name)
            .map_err(|error| {
                BondSmokeExecuteError::Preflight(format!(
                    "cannot load verified stored bond: {}",
                    error.0
                ))
            })?;
    let plan = BondSmokePlan::from_material(material)
        .map_err(BondSmokeExecuteError::Preflight)?;

    let firmware = be200_firmware_port::ensure_operational(
        &config.sfi,
        &config.ddc,
    )
    .map_err(|error| {
        BondSmokeExecuteError::Preflight(format!(
            "firmware bypass failed: {error:?}"
        ))
    })?;

    let controller_identity = format_address_le(plan.controller_identity_le);
    let stack = spawn_be200_hci_stack(HciStackConfig::default());
    let mut tracker = BondSmokeTracker::default();
    let mut known_connections = Vec::new();
    let mut members = Vec::new();

    let primary = (|| -> Result<(), BondSmokeRunError> {
        send_control(
            stack.control().try_begin_operational_attach(),
            "begin resetless attach",
        )?;
        wait_attach(
            stack.reports(),
            &mut tracker,
            Duration::from_secs(5),
        )?;

        send_control(
            stack.control().try_read_controller_identity(),
            "read controller identity",
        )?;
        let identity = wait_controller_identity(
            stack.reports(),
            &mut tracker,
            Duration::from_secs(5),
        )?;
        if !plan.identity_matches(identity) {
            return Err(BondSmokeRunError::ControllerIdentityMismatch {
                expected: plan.controller_identity_le,
                actual: identity.public_address_le,
            });
        }

        send_control(
            stack
                .control()
                .try_configure_controller_privacy(plan.privacy_config()),
            "configure controller privacy",
        )?;
        wait_privacy_ready(
            stack.reports(),
            &mut tracker,
            Duration::from_secs(5),
        )?;

        let first = run_member(
            &stack,
            &mut tracker,
            &plan,
            0,
            config.connect_timeout,
            &mut known_connections,
        )?;
        members.push(first);

        let second = run_member(
            &stack,
            &mut tracker,
            &plan,
            1,
            config.connect_timeout,
            &mut known_connections,
        )?;
        members.push(second);

        wait_dual_encrypted(
            stack.reports(),
            &mut tracker,
            Duration::from_secs(5),
            [
                members[0].connection,
                members[1].connection,
            ],
        )?;

        Ok(())
    })();

    // Cleanup runs even after the first failing stage. It derives live handles
    // from the latest ConnectionSet report rather than from intended state.
    let mut cleanup = cleanup_stack(&stack, &mut tracker);
    let exit = stack.shutdown();

    let after = fingerprint_bond_store(&config.bond_root);
    let (bond_unchanged, fingerprint_error) = match after {
        Ok(after) => (after == before, None),
        Err(error) => (
            false,
            Some(format!(
                "cannot fingerprint bond store after HIL: {}",
                error.0
            )),
        ),
    };

    let mut final_error = primary.err();
    if let Some(error) = fingerprint_error {
        if final_error.is_none() {
            final_error =
                Some(BondSmokeRunError::BondFingerprint(error));
        } else {
            cleanup.errors.push(error);
        }
    } else if !bond_unchanged {
        if final_error.is_none() {
            final_error = Some(BondSmokeRunError::BondStoreChanged);
        } else {
            cleanup
                .errors
                .push("bond store fingerprint changed".into());
        }
    }

    if final_error.is_none() && !cleanup.errors.is_empty() {
        final_error = Some(BondSmokeRunError::CleanupIncomplete);
    }

    if let Some(primary) = final_error {
        return Err(BondSmokeExecuteError::Runtime {
            primary,
            cleanup,
            bond_unchanged,
            exit,
        });
    }

    Ok(BondSmokeResult {
        firmware,
        controller_identity,
        members,
        bond_unchanged,
        cleanup,
        exit,
    })
}

pub fn format_address_le(address: [u8; 6]) -> String {
    address
        .iter()
        .rev()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(identity_last: u8, key_seed: u8) -> ReviewBondMember {
        ReviewBondMember {
            identity_address: format!(
                "E8:C9:13:EF:2F:{identity_last:02X}"
            ),
            identity_address_le: [
                identity_last, 0x2f, 0xef, 0x13, 0xc9, 0xe8,
            ],
            address_type: 0,
            peer_irk: [key_seed; 16],
            long_term_key: [key_seed.wrapping_add(1); 16],
            encrypted_diversifier: 0,
            random_number: [0; 8],
        }
    }

    fn material() -> ReviewBondMaterial {
        ReviewBondMaterial {
            controller_identity: "44:A3:BB:36:94:1A".into(),
            controller_identity_le: [
                0x1a, 0x94, 0x36, 0xbb, 0xa3, 0x44,
            ],
            local_irk: [0x44; 16],
            members: vec![member(0x23, 0x11), member(0xe0, 0x33)],
        }
    }

    #[test]
    fn parser_requires_explicit_hil_consent_and_paths() {
        assert!(parse_bond_smoke_args(&[]).is_err());

        let args = vec![
            "--sfi".into(), "a.sfi".into(),
            "--ddc".into(), "a.ddc".into(),
            "--bond-root".into(), "bonding".into(),
            "--name".into(), "Galaxy Buds3 Pro (2F23)".into(),
            "--confirm-stored-bond-hil".into(),
        ];
        let config = parse_bond_smoke_args(&args).unwrap();
        assert_eq!(
            config.connect_timeout,
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECONDS)
        );
    }

    #[test]
    fn plan_requires_exactly_two_verified_members() {
        let mut value = material();
        value.members.pop();
        assert!(BondSmokePlan::from_material(value).is_err());

        let plan = BondSmokePlan::from_material(material()).unwrap();
        assert_eq!(plan.members.len(), 2);
    }

    #[test]
    fn plan_maps_bond_material_into_typed_privacy_and_security_requests() {
        let plan = BondSmokePlan::from_material(material()).unwrap();
        let privacy = plan.privacy_config();
        assert_eq!(privacy.peers.len(), 2);
        assert_eq!(privacy.peers[0].identity_address_le[0], 0x23);
        assert_eq!(privacy.local_irk, [0x44; 16]);

        let token = ConnectionToken {
            connection_handle: 0x0800,
            generation: 7,
        };
        let encryption = plan.encryption_request(0, token);
        assert_eq!(encryption.connection, token);
        assert_eq!(encryption.key.long_term_key, [0x12; 16]);
    }

    #[test]
    fn tracker_projects_connection_set_without_key_material() {
        let token = ConnectionToken {
            connection_handle: 0x0800,
            generation: 3,
        };
        let mut tracker = BondSmokeTracker::default();
        tracker.observe(&HciReport::ConnectionSet(
            LeConnectionSetSnapshot {
                connections: vec![
                    crate::hci_host::connection::LeConnectionSnapshot {
                        connection_handle: 0x0800,
                        generation: 3,
                        role: 0,
                        peer_address_le: [1, 2, 3, 4, 5, 6],
                        peer_address_type: 1,
                        connection_interval: 24,
                        peripheral_latency: 0,
                        supervision_timeout: 42,
                        encryption_enabled: 1,
                        encryption_key_size: 16,
                    },
                ],
            },
        ));
        assert!(tracker.contains_connection(token));
        assert!(tracker.is_encrypted(token));
    }

    #[test]
    fn dual_encrypted_gate_accepts_already_observed_state() {
        let first = ConnectionToken {
            connection_handle: 0x0800,
            generation: 1,
        };
        let second = ConnectionToken {
            connection_handle: 0x0801,
            generation: 2,
        };
        let mut tracker = BondSmokeTracker::default();
        for (token, peer) in [(first, [1, 2, 3, 4, 5, 6]), (second, [7, 8, 9, 10, 11, 12])] {
            tracker.connection_set.connections.push(
                crate::hci_host::connection::LeConnectionSnapshot {
                    connection_handle: token.connection_handle,
                    generation: token.generation,
                    role: 0,
                    peer_address_le: peer,
                    peer_address_type: 1,
                    connection_interval: 24,
                    peripheral_latency: 0,
                    supervision_timeout: 42,
                    encryption_enabled: 1,
                    encryption_key_size: 16,
                },
            );
        }

        assert!(dual_encrypted(&tracker, [first, second]));
    }

    #[test]
    fn queued_report_wait_updates_tracker_before_matching() {
        let (_control, _ingress, reports, ports) =
            crate::hci_host::mailbox::hci_mailbox(2, 2, 4);
        ports
            .report_tx
            .try_send(HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            ))
            .unwrap();

        let mut tracker = BondSmokeTracker::default();
        wait_privacy_ready(
            &reports,
            &mut tracker,
            Duration::from_millis(10),
        )
        .unwrap();
        assert_eq!(
            tracker.privacy,
            Some(ControllerPrivacyState::Ready)
        );
    }

    #[test]
    fn address_formatter_uses_human_order() {
        assert_eq!(
            format_address_le([
                0x1a, 0x94, 0x36, 0xbb, 0xa3, 0x44,
            ]),
            "44:A3:BB:36:94:1A"
        );
    }
}
