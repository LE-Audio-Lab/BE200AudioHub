use crate::hci_host::{
    connection::{
        ConnectionToken, DisconnectFailure, DisconnectState,
        FalConnectFailure, FalConnectRequest, FalConnectionState,
        LeConnectionSetSnapshot,
    },
    engine::{
        ControllerIdentityFailure, OperationalAttachFailure,
        OperationalAttachState, PassiveScanState,
    },
    iso_host_feature::{
        CisHostFeatureFailure, CisHostFeatureState,
    },
    gatt::{GattReport, GattRequest, GattRequestId},
    mailbox::HciReport,
    privacy::{
        ControllerPrivacyState, PrivacyFailure, ResolvingListConfig,
    },
    security::{
        StoredEncryptionFailure, StoredEncryptionRequest,
        StoredEncryptionState,
    },
};

use super::{
    audio_discovery::{
        AudioDiscoveryAction, AudioDiscoveryFailure,
        AudioDiscoveryMachine,
    },
    audio_snapshot::{
        AudioMemberSnapshot, AudioSetSnapshot,
    },
    codec_plan::{
        plan_first_stream, CodecPlanError, StereoCodecPlan,
    },
    discovery::{
        resolve_connectable_members, ControlDiscoveryError,
        ResolvedMemberPresence,
    },
    plan::{ControlPlanError, ControlSetPlan},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlPhase {
    Idle,
    Attaching,
    ReadingControllerIdentity,
    EnsuringCisHostFeature,
    StartingDiscovery,
    DiscoveringMembers,
    StoppingDiscovery,
    ConfiguringPrivacy,
    ConnectingMember { member_index: usize },
    SecuringMember { member_index: usize },
    AwaitingSetConnected,
    SetConnected,
    DiscoveringAudioMember { member_index: usize },
    AudioSetDiscovered,
    CodecPlanned,
    CancellingConnection { member_index: usize },
    RollingBack,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlTimer {
    Attach,
    ControllerIdentity,
    CisHostFeature,
    ScanStart,
    DiscoveryWindow,
    ScanStop,
    Privacy,
    Connect { member_index: usize },
    CancelConnect { member_index: usize },
    Encryption { member_index: usize },
    SetProjection,
    Gatt {
        member_index: usize,
        request_id: GattRequestId,
    },
    CleanupDisconnect(ConnectionToken),
    CleanupPrivacy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlAction {
    BeginOperationalAttach,
    ReadControllerIdentity,
    EnsureCisHostFeature,
    StartPassiveExtendedScan,
    StopPassiveExtendedScan,
    ConfigureControllerPrivacy(ResolvingListConfig),
    BeginFalConnection {
        member_index: usize,
        request: FalConnectRequest,
    },
    BeginStoredEncryption {
        member_index: usize,
        request: StoredEncryptionRequest,
    },
    CancelFalConnection,
    DisconnectAcl {
        connection: ConnectionToken,
    },
    ClearControllerPrivacy,
    BeginGatt(GattRequest),
    CancelGatt {
        connection: ConnectionToken,
        request_id: GattRequestId,
    },
    ArmTimer(ControlTimer),
    DisarmTimer(ControlTimer),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlCleanupFailure {
    Disconnect(DisconnectFailure),
    Privacy(PrivacyFailure),
    Timeout(ControlTimer),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlFailure {
    InvalidStartState(ControlPhase),
    Plan(ControlPlanError),
    Attach(OperationalAttachFailure),
    ControllerIdentity(ControllerIdentityFailure),
    ControllerIdentityMismatch {
        expected: [u8; 6],
        actual: [u8; 6],
    },
    CisHostFeature(CisHostFeatureFailure),
    Scan {
        opcode: u16,
        status: u8,
    },
    Discovery(ControlDiscoveryError),
    Privacy(PrivacyFailure),
    Connection {
        member_index: usize,
        failure: FalConnectFailure,
    },
    Encryption {
        member_index: usize,
        failure: StoredEncryptionFailure,
    },
    ConnectionTokenMismatch {
        member_index: usize,
        expected: ConnectionToken,
        actual: ConnectionToken,
    },
    ConnectionSetMismatch,
    AudioDiscovery {
        member_index: usize,
        failure: AudioDiscoveryFailure,
    },
    AudioDiscoveryTimeout {
        member_index: usize,
        request_id: GattRequestId,
    },
    AudioSet(String),
    CodecPlan(CodecPlanError),
    Timeout(ControlTimer),
    RollbackFailed {
        primary: Box<ControlFailure>,
        cleanup: Vec<ControlCleanupFailure>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    DiscoveryWindowElapsed,
    TimerExpired(ControlTimer),
}

pub struct ControlMachine {
    plan: ControlSetPlan,
    phase: ControlPhase,
    resolved_members: Vec<ResolvedMemberPresence>,
    connections: [Option<ConnectionToken>; 2],
    encryption_key_sizes: [Option<u8>; 2],
    privacy_active: bool,
    rollback_queue: Vec<ConnectionToken>,
    rollback_current: Option<ConnectionToken>,
    cleanup_failures: Vec<ControlCleanupFailure>,
    next_gatt_request_id: GattRequestId,
    audio_discovery: Option<AudioDiscoveryMachine>,
    audio_members: Vec<AudioMemberSnapshot>,
    audio_set: Option<AudioSetSnapshot>,
    codec_plan: Option<StereoCodecPlan>,
    failure: Option<ControlFailure>,
}

impl ControlMachine {
    pub fn new(plan: ControlSetPlan) -> Result<Self, ControlPlanError> {
        plan.validate()?;
        Ok(Self {
            plan,
            phase: ControlPhase::Idle,
            resolved_members: Vec::new(),
            connections: [None, None],
            encryption_key_sizes: [None, None],
            privacy_active: false,
            rollback_queue: Vec::new(),
            rollback_current: None,
            cleanup_failures: Vec::new(),
            next_gatt_request_id: 1,
            audio_discovery: None,
            audio_members: Vec::new(),
            audio_set: None,
            codec_plan: None,
            failure: None,
        })
    }

    pub fn phase(&self) -> ControlPhase {
        self.phase
    }

    pub fn resolved_members(&self) -> &[ResolvedMemberPresence] {
        &self.resolved_members
    }

    pub fn failure(&self) -> Option<&ControlFailure> {
        self.failure.as_ref()
    }

    pub fn connections(&self) -> [Option<ConnectionToken>; 2] {
        self.connections
    }

    pub fn audio_set(&self) -> Option<&AudioSetSnapshot> {
        self.audio_set.as_ref()
    }

    pub fn codec_plan(&self) -> Option<&StereoCodecPlan> {
        self.codec_plan.as_ref()
    }

    pub fn plan_codec(
        &mut self,
    ) -> Result<(), ControlFailure> {
        if self.phase != ControlPhase::AudioSetDiscovered {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let set = self
            .audio_set
            .as_ref()
            .expect("AudioSetDiscovered requires snapshot");
        let plan = plan_first_stream(set)
            .map_err(ControlFailure::CodecPlan)?;
        self.codec_plan = Some(plan);
        self.phase = ControlPhase::CodecPlanned;
        Ok(())
    }

    pub fn begin_audio_discovery(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::SetConnected {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.audio_members.clear();
        self.audio_set = None;
        self.codec_plan = None;
        self.start_audio_member_discovery(0)
    }

    pub fn start(&mut self) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::Idle {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.plan.validate().map_err(ControlFailure::Plan)?;
        self.phase = ControlPhase::Attaching;
        Ok(vec![
            ControlAction::BeginOperationalAttach,
            ControlAction::ArmTimer(ControlTimer::Attach),
        ])
    }

    pub fn on_event(
        &mut self,
        event: ControlEvent,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        match event {
            ControlEvent::DiscoveryWindowElapsed
                if self.phase == ControlPhase::DiscoveringMembers =>
            {
                self.phase = ControlPhase::StoppingDiscovery;
                Ok(vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::DiscoveryWindow,
                    ),
                    ControlAction::StopPassiveExtendedScan,
                    ControlAction::ArmTimer(ControlTimer::ScanStop),
                ])
            }
            ControlEvent::TimerExpired(
                ControlTimer::Connect { member_index },
            ) if self.phase
                == ControlPhase::ConnectingMember { member_index } =>
            {
                self.failure = Some(ControlFailure::Timeout(
                    ControlTimer::Connect { member_index },
                ));
                self.phase =
                    ControlPhase::CancellingConnection { member_index };
                Ok(vec![
                    ControlAction::CancelFalConnection,
                    ControlAction::ArmTimer(
                        ControlTimer::CancelConnect { member_index },
                    ),
                ])
            }
            ControlEvent::TimerExpired(
                ControlTimer::Encryption { member_index },
            ) if self.phase
                == ControlPhase::SecuringMember { member_index } =>
            {
                self.begin_rollback(ControlFailure::Timeout(
                    ControlTimer::Encryption { member_index },
                ))
            }
            ControlEvent::TimerExpired(ControlTimer::SetProjection)
                if self.phase == ControlPhase::AwaitingSetConnected =>
            {
                self.begin_rollback(
                    ControlFailure::ConnectionSetMismatch,
                )
            }
            ControlEvent::TimerExpired(
                ControlTimer::CancelConnect { member_index },
            ) if self.phase
                == ControlPhase::CancellingConnection { member_index } =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CancelConnect { member_index },
                    ),
                );
                self.finish_rollback()
            }
            ControlEvent::TimerExpired(
                ControlTimer::Gatt {
                    member_index,
                    request_id,
                },
            ) if self.phase
                == ControlPhase::DiscoveringAudioMember {
                    member_index,
                }
                && self
                    .audio_discovery
                    .as_ref()
                    .and_then(|value| value.pending_request_id())
                    == Some(request_id) =>
            {
                let connection = self.connections[member_index]
                    .expect("audio discovery requires live connection");
                self.audio_discovery = None;
                let mut actions = vec![
                    ControlAction::CancelGatt {
                        connection,
                        request_id,
                    },
                    ControlAction::DisarmTimer(
                        ControlTimer::Gatt {
                            member_index,
                            request_id,
                        },
                    ),
                ];
                actions.extend(self.begin_rollback(
                    ControlFailure::AudioDiscoveryTimeout {
                        member_index,
                        request_id,
                    },
                )?);
                Ok(actions)
            }
            ControlEvent::TimerExpired(
                ControlTimer::CleanupDisconnect(connection),
            ) if self.phase == ControlPhase::RollingBack
                && self.rollback_current == Some(connection) =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupDisconnect(connection),
                    ),
                );
                self.finish_rollback()
            }
            ControlEvent::TimerExpired(ControlTimer::CleanupPrivacy)
                if self.phase == ControlPhase::RollingBack =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupPrivacy,
                    ),
                );
                self.finish_rollback()
            }
            ControlEvent::TimerExpired(timer)
                if self.timer_matches_phase(timer) =>
            {
                self.fail(ControlFailure::Timeout(timer))
            }
            _ => Ok(Vec::new()),
        }
    }

    pub fn on_hci_report(
        &mut self,
        report: &HciReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        match (self.phase, report) {
            (
                ControlPhase::Attaching,
                HciReport::CapabilitiesReady(_),
            ) => {
                self.phase = ControlPhase::ReadingControllerIdentity;
                Ok(vec![
                    ControlAction::DisarmTimer(ControlTimer::Attach),
                    ControlAction::ReadControllerIdentity,
                    ControlAction::ArmTimer(
                        ControlTimer::ControllerIdentity,
                    ),
                ])
            }
            (
                ControlPhase::Attaching,
                HciReport::AttachState(
                    OperationalAttachState::Failed(failure),
                ),
            ) => self.fail(ControlFailure::Attach(*failure)),

            (
                ControlPhase::ReadingControllerIdentity,
                HciReport::ControllerIdentity(Ok(identity)),
            ) => {
                if identity.public_address_le
                    != self.plan.controller_identity_le
                {
                    return self.fail(
                        ControlFailure::ControllerIdentityMismatch {
                            expected: self.plan.controller_identity_le,
                            actual: identity.public_address_le,
                        },
                    );
                }
                self.phase = ControlPhase::EnsuringCisHostFeature;
                Ok(vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::ControllerIdentity,
                    ),
                    ControlAction::EnsureCisHostFeature,
                    ControlAction::ArmTimer(
                        ControlTimer::CisHostFeature,
                    ),
                ])
            }
            (
                ControlPhase::ReadingControllerIdentity,
                HciReport::ControllerIdentity(Err(failure)),
            ) => self.fail(ControlFailure::ControllerIdentity(*failure)),

            (
                ControlPhase::EnsuringCisHostFeature,
                HciReport::CisHostFeatureState(
                    CisHostFeatureState::Ready { .. },
                ),
            ) => {
                self.phase = ControlPhase::StartingDiscovery;
                Ok(vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::CisHostFeature,
                    ),
                    ControlAction::StartPassiveExtendedScan,
                    ControlAction::ArmTimer(ControlTimer::ScanStart),
                ])
            }
            (
                ControlPhase::EnsuringCisHostFeature,
                HciReport::CisHostFeatureState(
                    CisHostFeatureState::Failed(failure),
                ),
            ) => self.fail(ControlFailure::CisHostFeature(*failure)),

            (
                ControlPhase::StartingDiscovery,
                HciReport::PassiveScanState(
                    PassiveScanState::Scanning,
                ),
            ) => {
                self.phase = ControlPhase::DiscoveringMembers;
                Ok(vec![
                    ControlAction::DisarmTimer(ControlTimer::ScanStart),
                    ControlAction::ArmTimer(
                        ControlTimer::DiscoveryWindow,
                    ),
                ])
            }
            (
                ControlPhase::StartingDiscovery
                | ControlPhase::DiscoveringMembers
                | ControlPhase::StoppingDiscovery,
                HciReport::PassiveScanState(
                    PassiveScanState::Failed { opcode, status },
                ),
            ) => self.fail(ControlFailure::Scan {
                opcode: *opcode,
                status: *status,
            }),

            (
                ControlPhase::StoppingDiscovery,
                HciReport::ScanComplete(scan),
            ) => {
                let resolved =
                    resolve_connectable_members(&self.plan, scan)
                        .map_err(ControlFailure::Discovery);
                let resolved = match resolved {
                    Ok(resolved) => resolved,
                    Err(failure) => return self.fail(failure),
                };
                let privacy = self
                    .plan
                    .privacy_config()
                    .map_err(ControlFailure::Plan);
                let privacy = match privacy {
                    Ok(privacy) => privacy,
                    Err(failure) => return self.fail(failure),
                };

                self.resolved_members = resolved;
                self.phase = ControlPhase::ConfiguringPrivacy;
                Ok(vec![
                    ControlAction::DisarmTimer(ControlTimer::ScanStop),
                    ControlAction::ConfigureControllerPrivacy(privacy),
                    ControlAction::ArmTimer(ControlTimer::Privacy),
                ])
            }
            (
                ControlPhase::ConfiguringPrivacy,
                HciReport::ControllerPrivacyState(
                    ControllerPrivacyState::Ready,
                ),
            ) => {
                self.privacy_active = true;
                let mut actions = vec![
                    ControlAction::DisarmTimer(ControlTimer::Privacy),
                ];
                actions.extend(self.begin_member_connection(0)?);
                Ok(actions)
            },
            (
                ControlPhase::ConfiguringPrivacy,
                HciReport::ControllerPrivacyState(
                    ControllerPrivacyState::Failed(failure),
                ),
            ) => self.fail(ControlFailure::Privacy(failure.clone())),

            (
                ControlPhase::ConnectingMember { member_index },
                HciReport::FalConnectionState(
                    FalConnectionState::Connected(connection),
                ),
            ) => {
                let token = connection.token();
                self.connections[member_index] = Some(token);
                self.phase = ControlPhase::SecuringMember {
                    member_index,
                };
                let request = self
                    .plan
                    .encryption_request(member_index, token)
                    .map_err(ControlFailure::Plan)?;
                Ok(vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::Connect { member_index },
                    ),
                    ControlAction::BeginStoredEncryption {
                        member_index,
                        request,
                    },
                    ControlAction::ArmTimer(
                        ControlTimer::Encryption { member_index },
                    ),
                ])
            }
            (
                ControlPhase::ConnectingMember { member_index },
                HciReport::FalConnectionState(
                    FalConnectionState::Failed(failure),
                ),
            ) => self.begin_rollback(
                ControlFailure::Connection {
                    member_index,
                    failure: failure.clone(),
                },
            ),

            (
                ControlPhase::SecuringMember { member_index },
                HciReport::StoredEncryptionState(
                    StoredEncryptionState::Encrypted {
                        connection,
                        encryption_key_size,
                    },
                ),
            ) => {
                let expected = self.connections[member_index]
                    .expect("securing phase requires a connection token");
                if *connection != expected {
                    return self.fail(
                        ControlFailure::ConnectionTokenMismatch {
                            member_index,
                            expected,
                            actual: *connection,
                        },
                    );
                }
                self.encryption_key_sizes[member_index] =
                    Some(*encryption_key_size);

                let mut actions = vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::Encryption { member_index },
                    ),
                ];
                if member_index + 1 < self.plan.members.len() {
                    actions.extend(
                        self.begin_member_connection(member_index + 1)?,
                    );
                } else {
                    self.phase = ControlPhase::AwaitingSetConnected;
                    actions.push(ControlAction::ArmTimer(
                        ControlTimer::SetProjection,
                    ));
                }
                Ok(actions)
            }
            (
                ControlPhase::SecuringMember { member_index },
                HciReport::StoredEncryptionState(
                    StoredEncryptionState::Failed(failure),
                ),
            ) => self.begin_rollback(
                ControlFailure::Encryption {
                    member_index,
                    failure: *failure,
                },
            ),

            (
                ControlPhase::AwaitingSetConnected,
                HciReport::ConnectionSet(snapshot),
            ) => {
                if !self.connection_set_matches(snapshot) {
                    return Ok(Vec::new());
                }
                self.phase = ControlPhase::SetConnected;
                Ok(vec![ControlAction::DisarmTimer(
                    ControlTimer::SetProjection,
                )])
            }

            (
                ControlPhase::CancellingConnection { member_index },
                HciReport::FalConnectionState(
                    FalConnectionState::Failed(_),
                ),
            ) => {
                let mut actions = vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::CancelConnect { member_index },
                    ),
                ];
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
            (
                ControlPhase::CancellingConnection { member_index },
                HciReport::FalConnectionState(
                    FalConnectionState::ConnectedAfterCancel(connection)
                    | FalConnectionState::Connected(connection),
                ),
            ) => {
                self.connections[member_index] =
                    Some(connection.token());
                let mut actions = vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::CancelConnect { member_index },
                    ),
                ];
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }

            (
                ControlPhase::RollingBack,
                HciReport::DisconnectState(
                    DisconnectState::Disconnected {
                        connection,
                        ..
                    },
                ),
            ) if self.rollback_current == Some(*connection) => {
                self.remove_known_connection(*connection);
                self.rollback_current = None;
                let mut actions = vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::CleanupDisconnect(*connection),
                    ),
                ];
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
            (
                ControlPhase::RollingBack,
                HciReport::DisconnectState(
                    DisconnectState::Failed(failure),
                ),
            ) if self.rollback_current == Some(failure.connection) => {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Disconnect(
                        failure.clone(),
                    ),
                );
                self.rollback_current = None;
                self.rollback_queue.clear();
                let mut actions = vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::CleanupDisconnect(
                            failure.connection,
                        ),
                    ),
                ];
                if self.privacy_active {
                    actions.push(ControlAction::ClearControllerPrivacy);
                    actions.push(ControlAction::ArmTimer(
                        ControlTimer::CleanupPrivacy,
                    ));
                } else {
                    return self.finish_rollback();
                }
                Ok(actions)
            }
            (
                ControlPhase::RollingBack,
                HciReport::ControllerPrivacyState(
                    ControllerPrivacyState::Idle,
                ),
            ) if self.rollback_current.is_none()
                && self.rollback_queue.is_empty()
                && self.privacy_active =>
            {
                self.privacy_active = false;
                self.finish_rollback()
            }
            (
                ControlPhase::RollingBack,
                HciReport::ControllerPrivacyState(
                    ControllerPrivacyState::Failed(failure),
                ),
            ) if self.rollback_current.is_none()
                && self.rollback_queue.is_empty() =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Privacy(failure.clone()),
                );
                self.privacy_active = false;
                self.finish_rollback()
            }

            (
                ControlPhase::DiscoveringAudioMember { member_index },
                HciReport::Gatt(report),
            ) => self.on_audio_gatt_report(member_index, report),

            _ => Ok(Vec::new()),
        }
    }

    fn start_audio_member_discovery(
        &mut self,
        member_index: usize,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let connection = self.connections[member_index]
            .expect("audio discovery requires connected member");
        let mut discovery = AudioDiscoveryMachine::new(
            member_index,
            connection,
            self.next_gatt_request_id,
        );
        let action = discovery.start().map_err(|failure| {
            ControlFailure::AudioDiscovery {
                member_index,
                failure,
            }
        })?;
        self.next_gatt_request_id = discovery.next_request_id();
        self.audio_discovery = Some(discovery);
        self.phase = ControlPhase::DiscoveringAudioMember {
            member_index,
        };
        self.translate_audio_action(member_index, None, action)
    }

    fn on_audio_gatt_report(
        &mut self,
        member_index: usize,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut discovery = self
            .audio_discovery
            .take()
            .expect("audio discovery phase missing member FSM");
        let pending = discovery.pending_request_id();
        let outcome = discovery.on_gatt_report(report);
        self.next_gatt_request_id = discovery.next_request_id();

        match outcome {
            Ok(AudioDiscoveryAction::Ignored) => {
                self.audio_discovery = Some(discovery);
                Ok(Vec::new())
            }
            Ok(action) => {
                self.audio_discovery = if matches!(
                    action,
                    AudioDiscoveryAction::Complete(_)
                ) {
                    None
                } else {
                    Some(discovery)
                };
                self.translate_audio_action(
                    member_index,
                    pending,
                    action,
                )
            }
            Err(failure) => {
                self.audio_discovery = None;
                let mut actions = Vec::new();
                if let Some(request_id) = pending {
                    actions.push(ControlAction::DisarmTimer(
                        ControlTimer::Gatt {
                            member_index,
                            request_id,
                        },
                    ));
                }
                actions.extend(self.begin_rollback(
                    ControlFailure::AudioDiscovery {
                        member_index,
                        failure,
                    },
                )?);
                Ok(actions)
            }
        }
    }

    fn translate_audio_action(
        &mut self,
        member_index: usize,
        completed_request: Option<GattRequestId>,
        action: AudioDiscoveryAction,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut actions = Vec::new();
        if let Some(request_id) = completed_request {
            actions.push(ControlAction::DisarmTimer(
                ControlTimer::Gatt {
                    member_index,
                    request_id,
                },
            ));
        }

        match action {
            AudioDiscoveryAction::BeginGatt(request) => {
                let request_id = request.request_id;
                actions.push(ControlAction::BeginGatt(request));
                actions.push(ControlAction::ArmTimer(
                    ControlTimer::Gatt {
                        member_index,
                        request_id,
                    },
                ));
                Ok(actions)
            }
            AudioDiscoveryAction::Complete(snapshot) => {
                self.audio_members.push(snapshot);
                if member_index + 1 < self.plan.members.len() {
                    actions.extend(
                        self.start_audio_member_discovery(
                            member_index + 1,
                        )?,
                    );
                    return Ok(actions);
                }

                let set = AudioSetSnapshot {
                    members: std::mem::take(&mut self.audio_members),
                };
                if let Err(error) = set.validate_dual_sink() {
                    actions.extend(self.begin_rollback(
                        ControlFailure::AudioSet(error),
                    )?);
                    return Ok(actions);
                }
                self.audio_set = Some(set);
                self.phase = ControlPhase::AudioSetDiscovered;
                Ok(actions)
            }
            AudioDiscoveryAction::Ignored => Ok(actions),
        }
    }

    fn begin_member_connection(
        &mut self,
        member_index: usize,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let request = self
            .plan
            .connect_request(member_index)
            .map_err(ControlFailure::Plan)?;
        self.phase = ControlPhase::ConnectingMember {
            member_index,
        };
        Ok(vec![
            ControlAction::BeginFalConnection {
                member_index,
                request,
            },
            ControlAction::ArmTimer(
                ControlTimer::Connect { member_index },
            ),
        ])
    }

    fn begin_rollback(
        &mut self,
        failure: ControlFailure,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.failure = Some(failure);
        self.rollback_queue = self
            .connections
            .iter()
            .flatten()
            .copied()
            .collect();
        self.rollback_current = None;
        self.cleanup_failures.clear();
        self.phase = ControlPhase::RollingBack;
        self.continue_rollback()
    }

    fn continue_rollback(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.phase = ControlPhase::RollingBack;

        if self.rollback_current.is_none() {
            if let Some(connection) = self.rollback_queue.pop() {
                self.rollback_current = Some(connection);
                return Ok(vec![
                    ControlAction::DisconnectAcl { connection },
                    ControlAction::ArmTimer(
                        ControlTimer::CleanupDisconnect(connection),
                    ),
                ]);
            }
        }

        if self.rollback_current.is_some() {
            return Ok(Vec::new());
        }

        if self.privacy_active {
            return Ok(vec![
                ControlAction::ClearControllerPrivacy,
                ControlAction::ArmTimer(
                    ControlTimer::CleanupPrivacy,
                ),
            ]);
        }

        self.finish_rollback()
    }

    fn finish_rollback(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.phase = ControlPhase::Faulted;
        let primary = self.failure.take().unwrap_or(
            ControlFailure::ConnectionSetMismatch,
        );
        let final_failure = if self.cleanup_failures.is_empty() {
            primary
        } else {
            ControlFailure::RollbackFailed {
                primary: Box::new(primary),
                cleanup: std::mem::take(
                    &mut self.cleanup_failures,
                ),
            }
        };
        self.failure = Some(final_failure.clone());
        Err(final_failure)
    }

    fn remove_known_connection(
        &mut self,
        connection: ConnectionToken,
    ) {
        for slot in &mut self.connections {
            if *slot == Some(connection) {
                *slot = None;
            }
        }
    }


    fn connection_set_matches(
        &self,
        snapshot: &LeConnectionSetSnapshot,
    ) -> bool {
        if snapshot.connections.len() != self.plan.members.len() {
            return false;
        }

        self.connections.iter().all(|expected| {
            let Some(expected) = expected else {
                return false;
            };
            snapshot.connections.iter().any(|actual| {
                actual.token() == *expected
                    && actual.encryption_enabled != 0
            })
        })
    }

    fn timer_matches_phase(&self, timer: ControlTimer) -> bool {
        matches!(
            (self.phase, timer),
            (ControlPhase::Attaching, ControlTimer::Attach)
                | (
                    ControlPhase::ReadingControllerIdentity,
                    ControlTimer::ControllerIdentity
                )
                | (
                    ControlPhase::EnsuringCisHostFeature,
                    ControlTimer::CisHostFeature
                )
                | (
                    ControlPhase::StartingDiscovery,
                    ControlTimer::ScanStart
                )
                | (
                    ControlPhase::DiscoveringMembers,
                    ControlTimer::DiscoveryWindow
                )
                | (
                    ControlPhase::StoppingDiscovery,
                    ControlTimer::ScanStop
                )
                | (
                    ControlPhase::ConfiguringPrivacy,
                    ControlTimer::Privacy
                )
        )
    }

    fn fail(
        &mut self,
        failure: ControlFailure,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.phase = ControlPhase::Faulted;
        self.failure = Some(failure.clone());
        Err(failure)
    }
}

#[cfg(test)]
mod tests {
    use bumble::Address;
    use bumble_smp::privacy::resolvable_private_address;

    use crate::{
        control::plan::ControlMemberPlan,
        hci_host::{
            engine::{
                ControllerCapabilities, ControllerIdentity,
                PacketPoolSnapshot,
            },
            scan::{ScanObservation, ScanSummary},
            security::StoredLeKeyMaterial,
        },
    };

    use super::*;

    fn member(last: u8, irk: [u8; 16]) -> ControlMemberPlan {
        ControlMemberPlan {
            identity_address_le: [last, 5, 4, 3, 2, 1],
            identity_address_type: 1,
            peer_irk: irk,
            stored_key: StoredLeKeyMaterial::secure_connections(
                [last; 16],
            ),
        }
    }

    fn plan() -> ControlSetPlan {
        ControlSetPlan {
            controller_identity_le: [1, 2, 3, 4, 5, 6],
            local_irk: [0x44; 16],
            members: vec![
                member(0x10, [0x21; 16]),
                member(0x20, [0x31; 16]),
            ],
        }
    }

    fn capabilities() -> ControllerCapabilities {
        ControllerCapabilities {
            supported_commands: [0; 64],
            hci_version: Some(13),
            hci_subversion: Some(1),
            manufacturer: Some(2),
            local_le_features: vec![0; 8],
            local_le_features_max_page: None,
            le_acl_pool: Some(PacketPoolSnapshot {
                data_packet_length: 251,
                total_num_data_packets: 8,
            }),
            iso_pool: Some(PacketPoolSnapshot {
                data_packet_length: 248,
                total_num_data_packets: 8,
            }),
            number_of_supported_advertising_sets: 1,
            maximum_advertising_data_length: 1650,
        }
    }

    fn observation(address: Address, rssi: i8) -> ScanObservation {
        ScanObservation {
            address: address.to_string(true),
            address_type: address.address_type().0,
            name: None,
            rsi: None,
            rssi,
            primary_phy: 1,
            secondary_phy: 0,
            sid: 0,
            is_legacy: false,
            is_connectable: true,
            is_scannable: false,
            raw_data: vec![],
        }
    }

    #[test]
    fn happy_path_reaches_privacy_configuration_without_blocking_waits() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();

        assert_eq!(
            machine.start().unwrap(),
            vec![
                ControlAction::BeginOperationalAttach,
                ControlAction::ArmTimer(ControlTimer::Attach),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::CapabilitiesReady(
                capabilities(),
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::Attach),
                ControlAction::ReadControllerIdentity,
                ControlAction::ArmTimer(
                    ControlTimer::ControllerIdentity
                ),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::ControllerIdentity(Ok(
                ControllerIdentity {
                    public_address_le:
                        value.controller_identity_le,
                },
            )))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::DisarmTimer(
                    ControlTimer::ControllerIdentity
                ),
                ControlAction::EnsureCisHostFeature,
                ControlAction::ArmTimer(
                    ControlTimer::CisHostFeature
                )
            ]
        ));

        let actions = machine
            .on_hci_report(&HciReport::CisHostFeatureState(
                CisHostFeatureState::Ready {
                    initially_enabled: false,
                    set_command_sent: true,
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::CisHostFeature
                ),
                ControlAction::StartPassiveExtendedScan,
                ControlAction::ArmTimer(ControlTimer::ScanStart),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::PassiveScanState(
                PassiveScanState::Scanning,
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::ScanStart),
                ControlAction::ArmTimer(
                    ControlTimer::DiscoveryWindow
                ),
            ]
        );

        let actions = machine
            .on_event(ControlEvent::DiscoveryWindowElapsed)
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::DiscoveryWindow
                ),
                ControlAction::StopPassiveExtendedScan,
                ControlAction::ArmTimer(ControlTimer::ScanStop),
            ]
        );

        let left = resolvable_private_address(
            &value.members[0].peer_irk,
            [1, 2, 3],
        );
        let right = resolvable_private_address(
            &value.members[1].peer_irk,
            [4, 5, 6],
        );
        let scan = ScanSummary {
            reports_seen: 2,
            observations: vec![
                observation(left, -40),
                observation(right, -35),
            ],
            truncated: false,
        };

        let actions = machine
            .on_hci_report(&HciReport::ScanComplete(scan))
            .unwrap();
        assert_eq!(machine.phase(), ControlPhase::ConfiguringPrivacy);
        assert_eq!(machine.resolved_members().len(), 2);
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::DisarmTimer(ControlTimer::ScanStop),
                ControlAction::ConfigureControllerPrivacy(_),
                ControlAction::ArmTimer(ControlTimer::Privacy),
            ]
        ));
    }

    fn drive_to_configuring_privacy(
        machine: &mut ControlMachine,
        value: &ControlSetPlan,
    ) {
        machine.start().unwrap();
        machine
            .on_hci_report(&HciReport::CapabilitiesReady(
                capabilities(),
            ))
            .unwrap();
        machine
            .on_hci_report(&HciReport::ControllerIdentity(Ok(
                ControllerIdentity {
                    public_address_le:
                        value.controller_identity_le,
                },
            )))
            .unwrap();
        machine
            .on_hci_report(&HciReport::CisHostFeatureState(
                CisHostFeatureState::Ready {
                    initially_enabled: true,
                    set_command_sent: false,
                },
            ))
            .unwrap();
        machine
            .on_hci_report(&HciReport::PassiveScanState(
                PassiveScanState::Scanning,
            ))
            .unwrap();
        machine
            .on_event(ControlEvent::DiscoveryWindowElapsed)
            .unwrap();

        let left = resolvable_private_address(
            &value.members[0].peer_irk,
            [1, 2, 3],
        );
        let right = resolvable_private_address(
            &value.members[1].peer_irk,
            [4, 5, 6],
        );
        machine
            .on_hci_report(&HciReport::ScanComplete(ScanSummary {
                reports_seen: 2,
                observations: vec![
                    observation(left, -40),
                    observation(right, -35),
                ],
                truncated: false,
            }))
            .unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringPrivacy
        );
    }

    fn connection(
        handle: u16,
        generation: u64,
        encrypted: u8,
    ) -> crate::hci_host::connection::LeConnectionSnapshot {
        crate::hci_host::connection::LeConnectionSnapshot {
            connection_handle: handle,
            generation,
            role: 0,
            peer_address_le: [0; 6],
            peer_address_type: 1,
            connection_interval: 24,
            peripheral_latency: 0,
            supervision_timeout: 42,
            encryption_enabled: encrypted,
            encryption_key_size: 0,
        }
    }

    #[test]
    fn privacy_dual_acl_and_stored_ltk_reach_set_connected() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();
        drive_to_configuring_privacy(&mut machine, &value);

        let actions = machine
            .on_hci_report(&HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            ))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::DisarmTimer(ControlTimer::Privacy),
                ControlAction::BeginFalConnection {
                    member_index: 0,
                    ..
                },
                ControlAction::ArmTimer(
                    ControlTimer::Connect { member_index: 0 }
                )
            ]
        ));

        let first = connection(0x0800, 1, 0);
        let first_token = first.token();
        let actions = machine
            .on_hci_report(&HciReport::FalConnectionState(
                FalConnectionState::Connected(first),
            ))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::DisarmTimer(
                    ControlTimer::Connect { member_index: 0 }
                ),
                ControlAction::BeginStoredEncryption {
                    member_index: 0,
                    request,
                },
                ControlAction::ArmTimer(
                    ControlTimer::Encryption { member_index: 0 }
                )
            ] if request.connection == first_token
        ));

        let actions = machine
            .on_hci_report(&HciReport::StoredEncryptionState(
                StoredEncryptionState::Encrypted {
                    connection: first_token,
                    encryption_key_size: 0,
                },
            ))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::DisarmTimer(
                    ControlTimer::Encryption { member_index: 0 }
                ),
                ControlAction::BeginFalConnection {
                    member_index: 1,
                    ..
                },
                ControlAction::ArmTimer(
                    ControlTimer::Connect { member_index: 1 }
                )
            ]
        ));

        let second = connection(0x0801, 2, 0);
        let second_token = second.token();
        let actions = machine
            .on_hci_report(&HciReport::FalConnectionState(
                FalConnectionState::Connected(second),
            ))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::DisarmTimer(
                    ControlTimer::Connect { member_index: 1 }
                ),
                ControlAction::BeginStoredEncryption {
                    member_index: 1,
                    request,
                },
                ControlAction::ArmTimer(
                    ControlTimer::Encryption { member_index: 1 }
                )
            ] if request.connection == second_token
        ));

        assert_eq!(
            machine
                .on_hci_report(&HciReport::StoredEncryptionState(
                    StoredEncryptionState::Encrypted {
                        connection: second_token,
                        encryption_key_size: 0,
                    },
                ))
                .unwrap(),
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::Encryption { member_index: 1 },
                ),
                ControlAction::ArmTimer(
                    ControlTimer::SetProjection,
                ),
            ]
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::AwaitingSetConnected
        );

        let snapshot = LeConnectionSetSnapshot {
            connections: vec![
                connection(0x0800, 1, 1),
                connection(0x0801, 2, 1),
            ],
        };
        assert_eq!(
            machine
                .on_hci_report(&HciReport::ConnectionSet(snapshot))
                .unwrap(),
            vec![ControlAction::DisarmTimer(
                ControlTimer::SetProjection,
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::SetConnected);
        assert_eq!(
            machine.connections(),
            [Some(first_token), Some(second_token)]
        );
    }

    #[test]
    fn second_member_encryption_failure_disconnects_known_acls_reverse_then_clears_privacy() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();
        drive_to_configuring_privacy(&mut machine, &value);
        machine
            .on_hci_report(&HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            ))
            .unwrap();

        let first = connection(0x0800, 1, 0);
        let first_token = first.token();
        machine
            .on_hci_report(&HciReport::FalConnectionState(
                FalConnectionState::Connected(first),
            ))
            .unwrap();
        machine
            .on_hci_report(&HciReport::StoredEncryptionState(
                StoredEncryptionState::Encrypted {
                    connection: first_token,
                    encryption_key_size: 0,
                },
            ))
            .unwrap();

        let second = connection(0x0801, 2, 0);
        let second_token = second.token();
        machine
            .on_hci_report(&HciReport::FalConnectionState(
                FalConnectionState::Connected(second),
            ))
            .unwrap();

        let failure = StoredEncryptionFailure {
            connection: second_token,
            kind: crate::hci_host::security::
                StoredEncryptionFailureKind::EncryptionNotEnabled,
        };
        let actions = machine
            .on_hci_report(&HciReport::StoredEncryptionState(
                StoredEncryptionState::Failed(failure),
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisconnectAcl {
                    connection: second_token,
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupDisconnect(second_token),
                ),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::DisconnectState(
                DisconnectState::Disconnected {
                    connection: second_token,
                    event_reason: 0x16,
                },
            ))
            .unwrap();
        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupDisconnect(second_token),
            )
        );
        assert!(matches!(
            actions[1],
            ControlAction::DisconnectAcl {
                connection
            } if connection == first_token
        ));

        let actions = machine
            .on_hci_report(&HciReport::DisconnectState(
                DisconnectState::Disconnected {
                    connection: first_token,
                    event_reason: 0x16,
                },
            ))
            .unwrap();
        assert!(actions.contains(
            &ControlAction::ClearControllerPrivacy
        ));

        let final_error = machine
            .on_hci_report(&HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Idle,
            ))
            .unwrap_err();
        assert_eq!(
            final_error,
            ControlFailure::Encryption {
                member_index: 1,
                failure,
            }
        );
        assert_eq!(machine.phase(), ControlPhase::Faulted);
        assert_eq!(machine.connections(), [None, None]);
    }

    #[test]
    fn connect_timeout_cancels_before_acl_cleanup() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();
        drive_to_configuring_privacy(&mut machine, &value);
        machine
            .on_hci_report(&HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            ))
            .unwrap();

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::Connect { member_index: 0 },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::CancelFalConnection,
                ControlAction::ArmTimer(
                    ControlTimer::CancelConnect {
                        member_index: 0,
                    },
                ),
            ]
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::CancellingConnection {
                member_index: 0,
            }
        );
    }

    fn machine_at_set_connected() -> ControlMachine {
        let mut machine = ControlMachine::new(plan()).unwrap();
        machine.phase = ControlPhase::SetConnected;
        machine.privacy_active = true;
        machine.connections = [
            Some(ConnectionToken {
                connection_handle: 0x0800,
                generation: 1,
            }),
            Some(ConnectionToken {
                connection_handle: 0x0801,
                generation: 2,
            }),
        ];
        machine
    }

    #[test]
    fn audio_set_checkpoint_plans_first_stereo_stream() {
        use bumble_hci::CodingFormat;
        use bumble_profiles::{
            ascs::{AseState, AseStatus, AseStatusDetails},
            bap::{
                AudioLocation, CodecSpecificCapabilities, ContextType,
                SupportedFrameDuration, SupportedSamplingFrequency,
            },
            le_audio::Metadata,
            pacs::{AudioContexts, PacCodecCapabilities, PacRecord},
        };
        use crate::control::audio_snapshot::{
            AscsSinkSnapshot, CasSnapshot, GattServiceRange,
            PacsSinkSnapshot, SinkAseSnapshot,
        };

        let service = GattServiceRange {
            start_handle: 1,
            end_handle: 20,
        };
        let member = |member_index: usize, location: AudioLocation| {
            let contexts = AudioContexts {
                sink: ContextType::MEDIA,
                source: ContextType::PROHIBITED,
            };
            AudioMemberSnapshot {
                member_index,
                connection: ConnectionToken {
                    connection_handle: 0x0800 + member_index as u16,
                    generation: member_index as u64 + 1,
                },
                mtu: 247,
                pacs: PacsSinkSnapshot {
                    service,
                    sink_pac_handle: 2,
                    sink_audio_location_handle: 3,
                    available_contexts_handle: 4,
                    supported_contexts_handle: 5,
                    sink_pac_records: vec![PacRecord {
                        coding_format: CodingFormat::LC3,
                        codec_specific_capabilities:
                            PacCodecCapabilities::Standard(
                                CodecSpecificCapabilities {
                                    supported_sampling_frequencies:
                                        SupportedSamplingFrequency::FREQ_48000,
                                    supported_frame_durations:
                                        SupportedFrameDuration::
                                            DURATION_10000_US_SUPPORTED,
                                    supported_audio_channel_count: vec![1],
                                    min_octets_per_codec_frame: 26,
                                    max_octets_per_codec_frame: 155,
                                    supported_max_codec_frames_per_sdu: 1,
                                },
                            ),
                        metadata: Metadata::default(),
                    }],
                    sink_audio_location: location,
                    available_contexts: contexts,
                    supported_contexts: contexts,
                },
                ascs: AscsSinkSnapshot {
                    service,
                    sink_ases: vec![SinkAseSnapshot {
                        declaration_handle: 6,
                        value_handle: 7,
                        end_group_handle: 8,
                        properties: 0x12,
                        status: AseStatus {
                            ase_id: 1,
                            state: AseState::IDLE,
                            details: AseStatusDetails::None,
                        },
                    }],
                    control_point_handle: 9,
                },
                cas: CasSnapshot {
                    service,
                    included_csis: service,
                },
            }
        };

        let mut machine = machine_at_set_connected();
        machine.phase = ControlPhase::AudioSetDiscovered;
        machine.audio_set = Some(AudioSetSnapshot {
            members: vec![
                member(0, AudioLocation::FRONT_RIGHT),
                member(1, AudioLocation::FRONT_LEFT),
            ],
        });

        machine.plan_codec().unwrap();
        assert_eq!(machine.phase(), ControlPhase::CodecPlanned);
        let plan = machine.codec_plan().unwrap();
        assert_eq!(plan.left.member_index, 1);
        assert_eq!(plan.right.member_index, 0);
        assert_eq!(plan.format.sample_rate_hz, 48_000);
        assert_eq!(plan.format.frame_duration_us, 10_000);
        assert_eq!(plan.format.octets_per_sdu, 100);
    }

    #[test]
    fn audio_discovery_starts_from_set_connected_on_member_zero() {
        let mut machine = machine_at_set_connected();
        let actions = machine.begin_audio_discovery().unwrap();

        assert_eq!(
            machine.phase(),
            ControlPhase::DiscoveringAudioMember {
                member_index: 0,
            }
        );
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::BeginGatt(GattRequest {
                    connection,
                    operation: crate::hci_host::gatt::GattOperation::
                        ExchangeMtu { client_rx_mtu: 247 },
                    ..
                }),
                ControlAction::ArmTimer(ControlTimer::Gatt {
                    member_index: 0,
                    ..
                })
            ] if connection.connection_handle == 0x0800
        ));
    }

    #[test]
    fn audio_discovery_timeout_cancels_gatt_before_acl_rollback() {
        let mut machine = machine_at_set_connected();
        let actions = machine.begin_audio_discovery().unwrap();
        let request = match &actions[0] {
            ControlAction::BeginGatt(request) => request.clone(),
            other => panic!("unexpected action {other:?}"),
        };

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::Gatt {
                    member_index: 0,
                    request_id: request.request_id,
                },
            ))
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::CancelGatt {
                connection: request.connection,
                request_id: request.request_id,
            }
        );
        assert_eq!(
            actions[1],
            ControlAction::DisarmTimer(ControlTimer::Gatt {
                member_index: 0,
                request_id: request.request_id,
            })
        );
        assert!(matches!(
            actions[2],
            ControlAction::DisconnectAcl { connection }
                if connection.connection_handle == 0x0801
        ));
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::AudioDiscoveryTimeout {
                member_index: 0,
                request_id,
            }) if *request_id == request.request_id
        ));
    }

    #[test]
    fn audio_discovery_gatt_failure_rolls_back_connected_set() {
        let mut machine = machine_at_set_connected();
        let actions = machine.begin_audio_discovery().unwrap();
        let request = match &actions[0] {
            ControlAction::BeginGatt(request) => request.clone(),
            other => panic!("unexpected action {other:?}"),
        };

        let failure = crate::hci_host::gatt::GattFailure {
            request_id: request.request_id,
            connection: request.connection,
            kind: crate::hci_host::gatt::GattFailureKind::Att {
                request_opcode: 0x02,
                attribute_handle: 0,
                error_code: 0x0E,
            },
        };
        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Failed(failure.clone()),
            ))
            .unwrap();

        assert!(matches!(
            actions.first(),
            Some(ControlAction::DisarmTimer(
                ControlTimer::Gatt {
                    member_index: 0,
                    request_id,
                }
            )) if *request_id == request.request_id
        ));
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisconnectAcl { connection }
                if connection.connection_handle == 0x0801
        )));
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::AudioDiscovery {
                member_index: 0,
                failure: AudioDiscoveryFailure::Gatt(actual),
            }) if actual == &failure
        ));
    }

    #[test]
    fn controller_identity_mismatch_faults_before_radio_session_work() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();
        machine.start().unwrap();
        machine
            .on_hci_report(&HciReport::CapabilitiesReady(
                capabilities(),
            ))
            .unwrap();

        let error = machine
            .on_hci_report(&HciReport::ControllerIdentity(Ok(
                ControllerIdentity {
                    public_address_le: [9; 6],
                },
            )))
            .unwrap_err();

        assert_eq!(
            error,
            ControlFailure::ControllerIdentityMismatch {
                expected: value.controller_identity_le,
                actual: [9; 6],
            }
        );
        assert_eq!(machine.phase(), ControlPhase::Faulted);
    }

    #[test]
    fn stale_timer_event_is_ignored_after_phase_advance() {
        let mut machine = ControlMachine::new(plan()).unwrap();
        machine.start().unwrap();
        machine
            .on_hci_report(&HciReport::CapabilitiesReady(
                capabilities(),
            ))
            .unwrap();

        assert_eq!(
            machine
                .on_event(ControlEvent::TimerExpired(
                    ControlTimer::Attach
                ))
                .unwrap(),
            Vec::<ControlAction>::new()
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::ReadingControllerIdentity
        );
    }

    #[test]
    fn active_stage_timeout_is_explicit_fault() {
        let mut machine = ControlMachine::new(plan()).unwrap();
        machine.start().unwrap();

        let error = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::Attach,
            ))
            .unwrap_err();
        assert_eq!(
            error,
            ControlFailure::Timeout(ControlTimer::Attach)
        );
        assert_eq!(machine.phase(), ControlPhase::Faulted);
    }

}
