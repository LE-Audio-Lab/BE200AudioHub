use bumble_profiles::ascs::{
    AseOperation, AseState, AseStatus, AseStatusDetails,
};

use crate::{
    architecture::types::AudioSide,
    hci_host::{
    connection::{
        ConnectionToken, DisconnectFailure, DisconnectState,
        FalConnectFailure, FalConnectRequest, FalConnectionState,
        LeConnectionSetSnapshot,
    },
    engine::{
        ControllerIdentityFailure, OperationalAttachFailure,
        OperationalAttachState, PassiveScanState,
    },
    iso::{
        CigAllocation, CigConfig, CigConfigureFailure,
        CigConfigureState,
    },
    iso_host_feature::{
        CisHostFeatureFailure, CisHostFeatureState,
    },
    iso_lifecycle::{
        CigRemoveFailure, CigRemoveState,
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
    ascs_control::{
        AseOperationAction, AseOperationFailure, AseOperationMachine,
        AsePrepareAction, AsePrepareFailure, AsePrepareMachine,
        PreparedAseControl,
    },
    codec_plan::{
        plan_first_stream, CodecPlanError, StereoCodecPlan,
    },
    discovery::{
        resolve_connectable_members, ControlDiscoveryError,
        ResolvedMemberPresence,
    },
    enable_plan::{
        plan_first_enable, EnablePlanError, StereoEnablePlan,
    },
    iso_plan::{
        plan_first_iso, IsoPlanError, StereoIsoPlan,
    },
    plan::{ControlPlanError, ControlSetPlan},
    qos_plan::{
        plan_first_qos, QosPlanError, StereoQosPlan,
    },
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
    PreparingCodecAse { side: AudioSide },
    ConfiguringCodec { side: AudioSide },
    CodecConfigured,
    IsoPlanned,
    ConfiguringCig,
    CigConfigured,
    QosPlanned,
    ConfiguringQos { side: AudioSide },
    QosConfigured,
    EnablePlanned,
    Enabling { side: AudioSide },
    Enabled,
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
    AseOperation { side: AudioSide },
    CigConfig,
    CleanupCig { cig_id: u8 },
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
    ConfigureCig(CigConfig),
    RemoveCig { cig_id: u8 },
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
    CigRemove(CigRemoveFailure),
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
    AsePrepare {
        side: AudioSide,
        failure: AsePrepareFailure,
    },
    AsePrepareTimeout {
        side: AudioSide,
        request_id: GattRequestId,
    },
    AseOperation {
        side: AudioSide,
        failure: AseOperationFailure,
    },
    AseOperationTimeout {
        side: AudioSide,
    },
    CodecReadbackMismatch {
        side: AudioSide,
        detail: String,
    },
    IsoPlan(IsoPlanError),
    CigAllocationMismatch {
        expected_cig_id: u8,
        actual_cig_id: u8,
        expected_cis_ids: Vec<u8>,
        actual_cis_ids: Vec<u8>,
    },
    CigConfigure(CigConfigureFailure),
    CigConfigureTimeout {
        cig_id: u8,
    },
    QosPlan(QosPlanError),
    QosReadbackMismatch {
        side: AudioSide,
        detail: String,
    },
    EnablePlan(EnablePlanError),
    EnableReadbackMismatch {
        side: AudioSide,
        detail: String,
    },
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
    ase_prepare: Option<AsePrepareMachine>,
    ase_operation: Option<AseOperationMachine>,
    prepared_ases: [Option<PreparedAseControl>; 2],
    codec_statuses: [Option<AseStatus>; 2],
    iso_plan: Option<StereoIsoPlan>,
    qos_plan: Option<StereoQosPlan>,
    qos_statuses: [Option<AseStatus>; 2],
    enable_plan: Option<StereoEnablePlan>,
    enable_statuses: [Option<AseStatus>; 2],
    cig_may_exist: bool,
    rollback_cig_pending: bool,
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
            ase_prepare: None,
            ase_operation: None,
            prepared_ases: [None, None],
            codec_statuses: [None, None],
            iso_plan: None,
            qos_plan: None,
            qos_statuses: [None, None],
            enable_plan: None,
            enable_statuses: [None, None],
            cig_may_exist: false,
            rollback_cig_pending: false,
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

    pub fn prepared_ase(
        &self,
        side: AudioSide,
    ) -> Option<&PreparedAseControl> {
        self.prepared_ases[side_index(side)].as_ref()
    }

    pub fn codec_status(
        &self,
        side: AudioSide,
    ) -> Option<&AseStatus> {
        self.codec_statuses[side_index(side)].as_ref()
    }

    pub fn iso_plan(&self) -> Option<&StereoIsoPlan> {
        self.iso_plan.as_ref()
    }

    pub fn qos_plan(&self) -> Option<&StereoQosPlan> {
        self.qos_plan.as_ref()
    }

    pub fn qos_status(
        &self,
        side: AudioSide,
    ) -> Option<&AseStatus> {
        self.qos_statuses[side_index(side)].as_ref()
    }

    pub fn enable_plan(&self) -> Option<&StereoEnablePlan> {
        self.enable_plan.as_ref()
    }

    pub fn enable_status(
        &self,
        side: AudioSide,
    ) -> Option<&AseStatus> {
        self.enable_statuses[side_index(side)].as_ref()
    }

    pub fn plan_enable(
        &mut self,
    ) -> Result<(), ControlFailure> {
        if self.phase != ControlPhase::QosConfigured {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let codec = self
            .codec_plan
            .as_ref()
            .expect("QosConfigured requires codec plan");
        let iso = self
            .iso_plan
            .as_ref()
            .expect("QosConfigured requires ISO plan");
        let qos = self
            .qos_plan
            .as_ref()
            .expect("QosConfigured requires QoS plan");
        let plan = plan_first_enable(codec, iso, qos)
            .map_err(ControlFailure::EnablePlan)?;
        self.enable_plan = Some(plan);
        self.enable_statuses = [None, None];
        self.phase = ControlPhase::EnablePlanned;
        Ok(())
    }

    pub fn begin_enable(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::EnablePlanned {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.start_enable(AudioSide::Left)
    }

    pub fn plan_qos(
        &mut self,
    ) -> Result<(), ControlFailure> {
        if self.phase != ControlPhase::CigConfigured {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let codec = self
            .codec_plan
            .as_ref()
            .expect("CigConfigured requires codec plan");
        let iso = self
            .iso_plan
            .as_ref()
            .expect("CigConfigured requires ISO plan");
        let plan = plan_first_qos(codec, iso)
            .map_err(ControlFailure::QosPlan)?;
        self.qos_plan = Some(plan);
        self.qos_statuses = [None, None];
        self.enable_plan = None;
        self.enable_statuses = [None, None];
        self.phase = ControlPhase::QosPlanned;
        Ok(())
    }

    pub fn begin_qos_configuration(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::QosPlanned {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.start_config_qos(AudioSide::Left)
    }

    pub fn plan_iso(
        &mut self,
    ) -> Result<(), ControlFailure> {
        if self.phase != ControlPhase::CodecConfigured {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let codec = self
            .codec_plan
            .as_ref()
            .expect("CodecConfigured requires codec plan");
        let plan = plan_first_iso(codec)
            .map_err(ControlFailure::IsoPlan)?;
        self.iso_plan = Some(plan);
        self.qos_plan = None;
        self.qos_statuses = [None, None];
        self.enable_plan = None;
        self.enable_statuses = [None, None];
        self.phase = ControlPhase::IsoPlanned;
        Ok(())
    }

    pub fn begin_cig_configuration(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::IsoPlanned {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let plan = self
            .iso_plan
            .as_ref()
            .expect("IsoPlanned requires ISO plan");
        self.phase = ControlPhase::ConfiguringCig;
        Ok(vec![
            ControlAction::ConfigureCig(plan.cig_config()),
            ControlAction::ArmTimer(ControlTimer::CigConfig),
        ])
    }

    pub fn begin_codec_configuration(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::CodecPlanned {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.ase_prepare = None;
        self.ase_operation = None;
        self.prepared_ases = [None, None];
        self.codec_statuses = [None, None];
        self.start_ase_prepare(AudioSide::Left)
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
        self.iso_plan = None;
        self.qos_plan = None;
        self.qos_statuses = [None, None];
        self.enable_plan = None;
        self.enable_statuses = [None, None];
        self.cig_may_exist = false;
        self.rollback_cig_pending = false;
        self.ase_prepare = None;
        self.ase_operation = None;
        self.prepared_ases = [None, None];
        self.codec_statuses = [None, None];
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
        self.iso_plan = None;
        self.cig_may_exist = false;
        self.rollback_cig_pending = false;
        self.ase_prepare = None;
        self.ase_operation = None;
        self.prepared_ases = [None, None];
        self.codec_statuses = [None, None];
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
                ControlTimer::Gatt {
                    member_index,
                    request_id,
                },
            ) if matches!(
                self.phase,
                ControlPhase::PreparingCodecAse { side }
                    if self.planned_sink(side)
                        .map(|sink| sink.member_index)
                        .ok() == Some(member_index)
            )
                && self
                    .ase_prepare
                    .as_ref()
                    .and_then(|value| value.pending_request_id())
                    == Some(request_id) =>
            {
                let side = match self.phase {
                    ControlPhase::PreparingCodecAse { side } => side,
                    _ => unreachable!(),
                };
                let connection = self.planned_sink(side)?.connection;
                self.ase_prepare = None;
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
                    ControlFailure::AsePrepareTimeout {
                        side,
                        request_id,
                    },
                )?);
                Ok(actions)
            }
            ControlEvent::TimerExpired(
                ControlTimer::Gatt {
                    member_index,
                    request_id,
                },
            ) if self
                .active_ase_operation_side()
                .and_then(|side| {
                    self.planned_sink(side)
                        .map(|sink| (side, sink.member_index))
                        .ok()
                })
                .map(|(_, active_member)| active_member)
                == Some(member_index)
                && self
                    .ase_operation
                    .as_ref()
                    .and_then(|value| value.pending_request_id())
                    == Some(request_id) =>
            {
                let side = self
                    .active_ase_operation_side()
                    .expect("GATT timer requires active ASE operation");
                let connection = self.planned_sink(side)?.connection;
                self.ase_operation = None;
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
                    ControlAction::DisarmTimer(
                        ControlTimer::AseOperation { side },
                    ),
                ];
                actions.extend(self.begin_rollback(
                    ControlFailure::AseOperationTimeout { side },
                )?);
                Ok(actions)
            }
            ControlEvent::TimerExpired(
                ControlTimer::AseOperation { side },
            ) if self.active_ase_operation_side() == Some(side) =>
            {
                let connection = self.planned_sink(side)?.connection;
                let member_index = self.planned_sink(side)?.member_index;
                let pending = self
                    .ase_operation
                    .as_ref()
                    .and_then(|value| value.pending_request_id());
                self.ase_operation = None;

                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::AseOperation { side },
                )];
                if let Some(request_id) = pending {
                    actions.push(ControlAction::CancelGatt {
                        connection,
                        request_id,
                    });
                    actions.push(ControlAction::DisarmTimer(
                        ControlTimer::Gatt {
                            member_index,
                            request_id,
                        },
                    ));
                }
                actions.extend(self.begin_rollback(
                    ControlFailure::AseOperationTimeout { side },
                )?);
                Ok(actions)
            }
            ControlEvent::TimerExpired(
                ControlTimer::CigConfig,
            ) if self.phase == ControlPhase::ConfiguringCig =>
            {
                let cig_id = self
                    .iso_plan
                    .as_ref()
                    .expect("ConfiguringCig requires ISO plan")
                    .cig_id;
                // The command may have reached the controller even if the
                // completion is lost. Treat allocation as possible.
                self.cig_may_exist = true;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CigConfig,
                )];
                actions.extend(self.begin_rollback(
                    ControlFailure::CigConfigureTimeout { cig_id },
                )?);
                Ok(actions)
            }
            ControlEvent::TimerExpired(
                ControlTimer::CleanupCig { cig_id },
            ) if self.phase == ControlPhase::RollingBack
                && self.rollback_cig_pending =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupCig { cig_id },
                    ),
                );
                self.rollback_cig_pending = false;
                self.cig_may_exist = false;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CleanupCig { cig_id },
                )];
                actions.extend(self.continue_rollback()?);
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
                ControlPhase::ConfiguringCig,
                HciReport::CigConfigureState(
                    CigConfigureState::Ready(allocation),
                ),
            ) => {
                let plan = self
                    .iso_plan
                    .as_ref()
                    .expect("ConfiguringCig requires ISO plan");
                if !cig_allocation_matches_plan(allocation, plan) {
                    let expected_cis_ids =
                        vec![plan.left.cis_id, plan.right.cis_id];
                    let actual_cis_ids = allocation
                        .cis
                        .iter()
                        .map(|cis| cis.cis_id)
                        .collect::<Vec<_>>();
                    self.cig_may_exist = true;
                    let mut actions = vec![
                        ControlAction::DisarmTimer(
                            ControlTimer::CigConfig,
                        ),
                    ];
                    actions.extend(self.begin_rollback(
                        ControlFailure::CigAllocationMismatch {
                            expected_cig_id: plan.cig_id,
                            actual_cig_id: allocation.cig_id,
                            expected_cis_ids,
                            actual_cis_ids,
                        },
                    )?);
                    return Ok(actions);
                }

                self.cig_may_exist = true;
                self.phase = ControlPhase::CigConfigured;
                Ok(vec![ControlAction::DisarmTimer(
                    ControlTimer::CigConfig,
                )])
            }
            (
                ControlPhase::ConfiguringCig,
                HciReport::CigConfigureState(
                    CigConfigureState::Failed(failure),
                ),
            ) => {
                self.cig_may_exist = failure.may_have_allocated;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CigConfig,
                )];
                actions.extend(self.begin_rollback(
                    ControlFailure::CigConfigure(failure.clone()),
                )?);
                Ok(actions)
            }
            (
                ControlPhase::RollingBack,
                HciReport::CigRemoveState(
                    CigRemoveState::Removed { cig_id },
                ),
            ) if self.rollback_cig_pending
                && self
                    .iso_plan
                    .as_ref()
                    .map(|plan| plan.cig_id)
                    == Some(*cig_id) =>
            {
                self.rollback_cig_pending = false;
                self.cig_may_exist = false;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CleanupCig { cig_id: *cig_id },
                )];
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
            (
                ControlPhase::RollingBack,
                HciReport::CigRemoveState(
                    CigRemoveState::Failed(failure),
                ),
            ) if self.rollback_cig_pending =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::CigRemove(
                        failure.clone(),
                    ),
                );
                self.rollback_cig_pending = false;
                self.cig_may_exist = false;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CleanupCig {
                        cig_id: failure.cig_id,
                    },
                )];
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
            (
                ControlPhase::PreparingCodecAse { side },
                HciReport::Gatt(report),
            ) => self.on_ase_prepare_gatt_report(side, report),
            (
                ControlPhase::ConfiguringCodec { side }
                | ControlPhase::ConfiguringQos { side }
                | ControlPhase::Enabling { side },
                HciReport::Gatt(report),
            ) => self.on_ase_operation_gatt_report(side, report),

            _ => Ok(Vec::new()),
        }
    }

    fn start_ase_prepare(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let sink = self.planned_sink(side)?.clone();
        let mut machine = AsePrepareMachine::new(
            sink,
            self.next_gatt_request_id,
        );
        let action = machine.start().map_err(|failure| {
            ControlFailure::AsePrepare { side, failure }
        })?;
        self.next_gatt_request_id = machine.next_request_id();
        self.ase_prepare = Some(machine);
        self.phase = ControlPhase::PreparingCodecAse { side };
        self.translate_ase_prepare_action(side, None, action)
    }

    fn start_config_codec(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.prepared_ases[side_index(side)]
            .as_ref()
            .expect("prepared ASE must exist before ConfigCodec")
            .clone();
        let operation = AseOperation::ConfigCodec(vec![
            prepared.sink.config_codec_parameters(),
        ]);
        let mut machine = AseOperationMachine::new(
            prepared,
            operation,
            AseState::CODEC_CONFIGURED,
            self.next_gatt_request_id,
        );
        let action = machine.start().map_err(|failure| {
            ControlFailure::AseOperation { side, failure }
        })?;
        self.next_gatt_request_id = machine.next_request_id();
        self.ase_operation = Some(machine);
        self.phase = ControlPhase::ConfiguringCodec { side };

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::AseOperation { side },
        )];
        actions.extend(
            self.translate_ase_operation_action(side, None, action)?,
        );
        Ok(actions)
    }

    fn start_config_qos(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.prepared_ases[side_index(side)]
            .as_ref()
            .expect("QoS requires prepared ASE control")
            .clone();
        let planned = match side {
            AudioSide::Left => &self
                .qos_plan
                .as_ref()
                .expect("QosPlanned requires QoS plan")
                .left,
            AudioSide::Right => &self
                .qos_plan
                .as_ref()
                .expect("QosPlanned requires QoS plan")
                .right,
        };
        let operation = AseOperation::ConfigQos(vec![
            planned.parameters.clone(),
        ]);
        let mut machine = AseOperationMachine::new(
            prepared,
            operation,
            AseState::QOS_CONFIGURED,
            self.next_gatt_request_id,
        );
        let action = machine.start().map_err(|failure| {
            ControlFailure::AseOperation { side, failure }
        })?;
        self.next_gatt_request_id = machine.next_request_id();
        self.ase_operation = Some(machine);
        self.phase = ControlPhase::ConfiguringQos { side };

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::AseOperation { side },
        )];
        actions.extend(
            self.translate_ase_operation_action(side, None, action)?,
        );
        Ok(actions)
    }

    fn start_enable(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.prepared_ases[side_index(side)]
            .as_ref()
            .expect("Enable requires prepared ASE control")
            .clone();
        let planned = match side {
            AudioSide::Left => &self
                .enable_plan
                .as_ref()
                .expect("EnablePlanned requires Enable plan")
                .left,
            AudioSide::Right => &self
                .enable_plan
                .as_ref()
                .expect("EnablePlanned requires Enable plan")
                .right,
        };
        let operation = AseOperation::Enable(vec![
            planned.parameters(),
        ]);
        let mut machine = AseOperationMachine::new(
            prepared,
            operation,
            AseState::ENABLING,
            self.next_gatt_request_id,
        );
        let action = machine.start().map_err(|failure| {
            ControlFailure::AseOperation { side, failure }
        })?;
        self.next_gatt_request_id = machine.next_request_id();
        self.ase_operation = Some(machine);
        self.phase = ControlPhase::Enabling { side };

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::AseOperation { side },
        )];
        actions.extend(
            self.translate_ase_operation_action(side, None, action)?,
        );
        Ok(actions)
    }

    fn on_ase_prepare_gatt_report(
        &mut self,
        side: AudioSide,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut machine = self
            .ase_prepare
            .take()
            .expect("prepare phase missing ASE preparation FSM");
        let pending_before = machine.pending_request_id();
        let outcome = machine.on_gatt_report(report);
        let pending_after = machine.pending_request_id();
        self.next_gatt_request_id = machine.next_request_id();
        let completed_request = if pending_before.is_some()
            && pending_before != pending_after
        {
            pending_before
        } else {
            None
        };

        match outcome {
            Ok(AsePrepareAction::Ignored) => {
                self.ase_prepare = Some(machine);
                Ok(Vec::new())
            }
            Ok(action) => {
                self.ase_prepare = if matches!(
                    action,
                    AsePrepareAction::Complete(_)
                ) {
                    None
                } else {
                    Some(machine)
                };
                self.translate_ase_prepare_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Err(failure) => {
                self.ase_prepare = None;
                let mut actions =
                    self.disarm_gatt_request(side, completed_request)?;
                actions.extend(self.begin_rollback(
                    ControlFailure::AsePrepare { side, failure },
                )?);
                Ok(actions)
            }
        }
    }

    fn translate_ase_prepare_action(
        &mut self,
        side: AudioSide,
        completed_request: Option<GattRequestId>,
        action: AsePrepareAction,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut actions =
            self.disarm_gatt_request(side, completed_request)?;

        match action {
            AsePrepareAction::BeginGatt(request) => {
                let member_index = self.planned_sink(side)?.member_index;
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
            AsePrepareAction::Complete(prepared) => {
                self.prepared_ases[side_index(side)] = Some(prepared);
                actions.extend(self.start_config_codec(side)?);
                Ok(actions)
            }
            AsePrepareAction::Ignored => Ok(actions),
        }
    }

    fn on_ase_operation_gatt_report(
        &mut self,
        side: AudioSide,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut machine = self
            .ase_operation
            .take()
            .expect("ConfigCodec phase missing ASCS operation FSM");
        let pending_before = machine.pending_request_id();
        let outcome = machine.on_gatt_report(report);
        let pending_after = machine.pending_request_id();
        self.next_gatt_request_id = machine.next_request_id();
        let completed_request = if pending_before.is_some()
            && pending_before != pending_after
        {
            pending_before
        } else {
            None
        };

        match outcome {
            Ok(AseOperationAction::Ignored) => {
                self.ase_operation = Some(machine);
                Ok(Vec::new())
            }
            Ok(AseOperationAction::Await) => {
                self.ase_operation = Some(machine);
                self.disarm_gatt_request(side, completed_request)
            }
            Ok(action @ AseOperationAction::BeginGatt(_)) => {
                self.ase_operation = Some(machine);
                self.translate_ase_operation_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Ok(action @ AseOperationAction::Complete(_)) => {
                self.ase_operation = None;
                self.translate_ase_operation_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Err(failure) => {
                self.ase_operation = None;
                let mut actions =
                    self.disarm_gatt_request(side, completed_request)?;

                // CP/ASE notification may fail the semantic operation while
                // the ATT Write Request is still outstanding.
                if let Some(request_id) = pending_after {
                    actions.push(ControlAction::CancelGatt {
                        connection: self.planned_sink(side)?.connection,
                        request_id,
                    });
                    actions.extend(
                        self.disarm_gatt_request(
                            side,
                            Some(request_id),
                        )?,
                    );
                }

                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::AseOperation { side },
                ));
                actions.extend(self.begin_rollback(
                    ControlFailure::AseOperation { side, failure },
                )?);
                Ok(actions)
            }
        }
    }

    fn translate_ase_operation_action(
        &mut self,
        side: AudioSide,
        completed_request: Option<GattRequestId>,
        action: AseOperationAction,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut actions =
            self.disarm_gatt_request(side, completed_request)?;

        match action {
            AseOperationAction::BeginGatt(request) => {
                let member_index = self.planned_sink(side)?.member_index;
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
            AseOperationAction::Await
            | AseOperationAction::Ignored => Ok(actions),
            AseOperationAction::Complete(status) => {
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::AseOperation { side },
                ));

                match self.phase {
                    ControlPhase::ConfiguringCodec {
                        side: active_side,
                    } if active_side == side => {
                        if let Err(detail) =
                            self.verify_codec_readback(side, &status)
                        {
                            actions.extend(self.begin_rollback(
                                ControlFailure::CodecReadbackMismatch {
                                    side,
                                    detail,
                                },
                            )?);
                            return Ok(actions);
                        }

                        self.codec_statuses[side_index(side)] =
                            Some(status);
                        match side {
                            AudioSide::Left => {
                                actions.extend(
                                    self.start_ase_prepare(
                                        AudioSide::Right,
                                    )?,
                                );
                            }
                            AudioSide::Right => {
                                self.phase =
                                    ControlPhase::CodecConfigured;
                            }
                        }
                    }
                    ControlPhase::ConfiguringQos {
                        side: active_side,
                    } if active_side == side => {
                        if let Err(detail) =
                            self.verify_qos_readback(side, &status)
                        {
                            actions.extend(self.begin_rollback(
                                ControlFailure::QosReadbackMismatch {
                                    side,
                                    detail,
                                },
                            )?);
                            return Ok(actions);
                        }

                        self.qos_statuses[side_index(side)] =
                            Some(status);
                        match side {
                            AudioSide::Left => {
                                actions.extend(
                                    self.start_config_qos(
                                        AudioSide::Right,
                                    )?,
                                );
                            }
                            AudioSide::Right => {
                                self.phase =
                                    ControlPhase::QosConfigured;
                            }
                        }
                    }
                    ControlPhase::Enabling {
                        side: active_side,
                    } if active_side == side => {
                        if let Err(detail) =
                            self.verify_enable_readback(side, &status)
                        {
                            actions.extend(self.begin_rollback(
                                ControlFailure::EnableReadbackMismatch {
                                    side,
                                    detail,
                                },
                            )?);
                            return Ok(actions);
                        }

                        self.enable_statuses[side_index(side)] =
                            Some(status);
                        match side {
                            AudioSide::Left => {
                                actions.extend(
                                    self.start_enable(
                                        AudioSide::Right,
                                    )?,
                                );
                            }
                            AudioSide::Right => {
                                self.phase = ControlPhase::Enabled;
                            }
                        }
                    }
                    other => {
                        return Err(
                            ControlFailure::InvalidStartState(other),
                        )
                    }
                }
                Ok(actions)
            }
        }
    }

    fn verify_codec_readback(
        &self,
        side: AudioSide,
        status: &AseStatus,
    ) -> Result<(), String> {
        let planned = self
            .planned_sink(side)
            .map_err(|failure| format!("{failure:?}"))?;
        let expected =
            planned.codec_specific_configuration.to_bytes();

        match &status.details {
            AseStatusDetails::CodecConfigured {
                codec_id,
                codec_specific_configuration,
                ..
            } if *codec_id == planned.codec_id
                && codec_specific_configuration == &expected =>
            {
                Ok(())
            }
            other => Err(format!(
                "{side:?} Codec Configured readback mismatch: {other:?}"
            )),
        }
    }

    fn verify_qos_readback(
        &self,
        side: AudioSide,
        status: &AseStatus,
    ) -> Result<(), String> {
        let plan = self
            .qos_plan
            .as_ref()
            .ok_or_else(|| "missing QoS plan".to_string())?;
        let planned = match side {
            AudioSide::Left => &plan.left,
            AudioSide::Right => &plan.right,
        };
        let expected = planned.expected_readback();

        match &status.details {
            AseStatusDetails::QosConfigured(actual)
                if actual == &expected =>
            {
                Ok(())
            }
            other => Err(format!(
                "{side:?} QoS Configured readback mismatch: {other:?}"
            )),
        }
    }

    fn verify_enable_readback(
        &self,
        side: AudioSide,
        status: &AseStatus,
    ) -> Result<(), String> {
        use bumble_profiles::le_audio::MetadataTag;

        let plan = self
            .enable_plan
            .as_ref()
            .ok_or_else(|| "missing Enable plan".to_string())?;
        let planned = match side {
            AudioSide::Left => &plan.left,
            AudioSide::Right => &plan.right,
        };

        match &status.details {
            AseStatusDetails::Metadata {
                cig_id,
                cis_id,
                metadata,
            } if *cig_id == planned.cig_id
                && *cis_id == planned.cis_id
                && metadata.entries.iter().any(|entry| {
                    entry.tag == MetadataTag::STREAMING_AUDIO_CONTEXTS
                        && entry.data == plan.context.0.to_le_bytes()
                }) =>
            {
                Ok(())
            }
            other => Err(format!(
                "{side:?} Enable readback mismatch: {other:?}"
            )),
        }
    }

    fn active_ase_operation_side(&self) -> Option<AudioSide> {
        match self.phase {
            ControlPhase::ConfiguringCodec { side }
            | ControlPhase::ConfiguringQos { side }
            | ControlPhase::Enabling { side } => Some(side),
            _ => None,
        }
    }

    fn disarm_gatt_request(
        &self,
        side: AudioSide,
        request_id: Option<GattRequestId>,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let Some(request_id) = request_id else {
            return Ok(Vec::new());
        };
        Ok(vec![ControlAction::DisarmTimer(
            ControlTimer::Gatt {
                member_index: self.planned_sink(side)?.member_index,
                request_id,
            },
        )])
    }

    fn planned_sink(
        &self,
        side: AudioSide,
    ) -> Result<&crate::control::codec_plan::PlannedSink, ControlFailure> {
        let plan = self.codec_plan.as_ref().ok_or(
            ControlFailure::InvalidStartState(self.phase),
        )?;
        Ok(match side {
            AudioSide::Left => &plan.left,
            AudioSide::Right => &plan.right,
        })
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

        if self.cig_may_exist {
            if self.rollback_cig_pending {
                return Ok(Vec::new());
            }
            let cig_id = self
                .iso_plan
                .as_ref()
                .expect("CIG cleanup requires ISO plan")
                .cig_id;
            self.rollback_cig_pending = true;
            return Ok(vec![
                ControlAction::RemoveCig { cig_id },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupCig { cig_id },
                ),
            ]);
        }

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

fn cig_allocation_matches_plan(
    allocation: &CigAllocation,
    plan: &StereoIsoPlan,
) -> bool {
    if allocation.cig_id != plan.cig_id
        || allocation.cis.len() != 2
    {
        return false;
    }

    let mut actual = allocation
        .cis
        .iter()
        .map(|cis| cis.cis_id)
        .collect::<Vec<_>>();
    actual.sort_unstable();

    let mut expected =
        vec![plan.left.cis_id, plan.right.cis_id];
    expected.sort_unstable();
    actual == expected
}

fn side_index(side: AudioSide) -> usize {
    match side {
        AudioSide::Left => 0,
        AudioSide::Right => 1,
    }
}

#[cfg(test)]
mod tests {
    use bumble::{Address, Uuid};
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

    fn integration_planned_sink(
        side: AudioSide,
        member_index: usize,
        connection_handle: u16,
        generation: u64,
        location: bumble_profiles::bap::AudioLocation,
    ) -> crate::control::codec_plan::PlannedSink {
        use bumble_hci::CodingFormat;
        use bumble_profiles::bap::{
            CodecSpecificConfiguration, FrameDuration,
            SamplingFrequency,
        };
        use crate::control::audio_snapshot::GattCharacteristicRange;

        crate::control::codec_plan::PlannedSink {
            side,
            member_index,
            connection: ConnectionToken {
                connection_handle,
                generation,
            },
            ase_id: 1,
            ase_value_handle: 0x0046,
            ase_end_group_handle: 0x0048,
            ascs_control_point: GattCharacteristicRange {
                declaration_handle: 0x0042,
                value_handle: 0x0043,
                end_group_handle: 0x0045,
                properties: 0x12,
            },
            audio_location: location,
            codec_id: CodingFormat::LC3,
            codec_specific_configuration:
                CodecSpecificConfiguration {
                    sampling_frequency: Some(
                        SamplingFrequency::FREQ_48000,
                    ),
                    frame_duration: Some(
                        FrameDuration::DURATION_10000_US,
                    ),
                    audio_channel_allocation: Some(location),
                    octets_per_codec_frame: Some(100),
                    codec_frames_per_sdu: Some(1),
                },
        }
    }

    fn machine_at_codec_planned() -> ControlMachine {
        use bumble_profiles::bap::{
            AudioLocation, ContextType,
        };
        use crate::architecture::control::LeAudioFormat;

        let mut machine = machine_at_set_connected();
        machine.phase = ControlPhase::CodecPlanned;
        machine.codec_plan = Some(StereoCodecPlan {
            format: LeAudioFormat {
                sample_rate_hz: 48_000,
                frame_duration_us: 10_000,
                octets_per_sdu: 100,
            },
            context: ContextType::MEDIA,
            left: integration_planned_sink(
                AudioSide::Left,
                1,
                0x0801,
                2,
                AudioLocation::FRONT_LEFT,
            ),
            right: integration_planned_sink(
                AudioSide::Right,
                0,
                0x0800,
                1,
                AudioLocation::FRONT_RIGHT,
            ),
        });
        machine
    }

    fn only_begin_gatt(actions: &[ControlAction]) -> GattRequest {
        let requests = actions
            .iter()
            .filter_map(|action| match action {
                ControlAction::BeginGatt(request) => {
                    Some(request.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1, "expected one BeginGatt: {actions:?}");
        requests.into_iter().next().unwrap()
    }

    fn complete_gatt(
        machine: &mut ControlMachine,
        request: &GattRequest,
        result: crate::hci_host::gatt::GattResult,
    ) -> Vec<ControlAction> {
        machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Complete {
                    request_id: request.request_id,
                    connection: request.connection,
                    result,
                },
            ))
            .unwrap()
    }

    fn codec_status_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseEndpoint, AudioRole,
        };

        let planned = machine.planned_sink(side).unwrap();
        let mut endpoint =
            AseEndpoint::new(planned.ase_id, AudioRole::Sink);
        endpoint.state = AseState::CODEC_CONFIGURED;
        endpoint.codec_id = planned.codec_id;
        endpoint.codec_specific_configuration =
            planned.codec_specific_configuration.to_bytes();
        endpoint.value().unwrap()
    }

    fn codec_cp_success_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        AseControlResponse {
            opcode: AseOpcode::ConfigCodec,
            responses: vec![AseResponse {
                ase_id: machine.planned_sink(side).unwrap().ase_id,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    fn drive_ase_prepare(
        machine: &mut ControlMachine,
        mut actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::hci_host::gatt::{
            GattDescriptor, GattOperation, GattResult,
        };

        let request = only_begin_gatt(&actions);
        let ase_cccd = match request.operation {
            GattOperation::DiscoverDescriptors {
                ending_handle, ..
            } => ending_handle,
            ref other => panic!(
                "expected ASE descriptor discovery, got {other:?}"
            ),
        };
        actions = complete_gatt(
            machine,
            &request,
            GattResult::Descriptors(vec![GattDescriptor {
                handle: ase_cccd,
                uuid: Uuid::from_16_bits(0x2902),
            }]),
        );

        let request = only_begin_gatt(&actions);
        let cp_cccd = match request.operation {
            GattOperation::DiscoverDescriptors {
                ending_handle, ..
            } => ending_handle,
            ref other => panic!(
                "expected CP descriptor discovery, got {other:?}"
            ),
        };
        actions = complete_gatt(
            machine,
            &request,
            GattResult::Descriptors(vec![GattDescriptor {
                handle: cp_cccd,
                uuid: Uuid::from_16_bits(0x2902),
            }]),
        );

        let request = only_begin_gatt(&actions);
        let (value_handle, cccd_handle, indicate) =
            match request.operation {
                GattOperation::Subscribe {
                    value_handle,
                    cccd_handle,
                    indicate,
                } => (value_handle, cccd_handle, indicate),
                ref other => panic!(
                    "expected ASE subscribe, got {other:?}"
                ),
            };
        actions = complete_gatt(
            machine,
            &request,
            GattResult::Subscription {
                value_handle,
                cccd_handle,
                indicate,
                enabled: true,
            },
        );

        let request = only_begin_gatt(&actions);
        let (value_handle, cccd_handle, indicate) =
            match request.operation {
                GattOperation::Subscribe {
                    value_handle,
                    cccd_handle,
                    indicate,
                } => (value_handle, cccd_handle, indicate),
                ref other => panic!(
                    "expected CP subscribe, got {other:?}"
                ),
            };
        complete_gatt(
            machine,
            &request,
            GattResult::Subscription {
                value_handle,
                cccd_handle,
                indicate,
                enabled: true,
            },
        )
    }

    fn drive_config_codec(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::hci_host::gatt::{
            GattOperation, GattResult, GattValueEvent,
        };

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteRequest {
                attribute_handle: 0x0043,
                ..
            }
        ));

        let _ = complete_gatt(
            machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: true,
            },
        );

        assert!(machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0043,
                        value: codec_cp_success_bytes(machine, side),
                    },
                ),
            ))
            .unwrap()
            .is_empty());

        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0046,
                        value: codec_status_bytes(machine, side),
                    },
                ),
            ))
            .unwrap();

        let readback = only_begin_gatt(&actions);
        assert_eq!(
            readback.operation,
            GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        );
        complete_gatt(
            machine,
            &readback,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: codec_status_bytes(machine, side),
            },
        )
    }

    #[test]
    fn codec_configuration_runs_left_then_right_to_codec_configured() {
        let mut machine = machine_at_codec_planned();

        let actions = machine.begin_codec_configuration().unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::PreparingCodecAse {
                side: AudioSide::Left,
            }
        );

        let actions = drive_ase_prepare(&mut machine, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringCodec {
                side: AudioSide::Left,
            }
        );
        assert!(machine.prepared_ase(AudioSide::Left).is_some());

        let actions =
            drive_config_codec(&mut machine, AudioSide::Left, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::PreparingCodecAse {
                side: AudioSide::Right,
            }
        );
        assert!(machine.codec_status(AudioSide::Left).is_some());

        let actions = drive_ase_prepare(&mut machine, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringCodec {
                side: AudioSide::Right,
            }
        );

        let actions =
            drive_config_codec(&mut machine, AudioSide::Right, actions);
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(
                ControlTimer::AseOperation {
                    side: AudioSide::Right
                }
            )
        )));
        assert_eq!(machine.phase(), ControlPhase::CodecConfigured);
        assert!(machine.codec_status(AudioSide::Left).is_some());
        assert!(machine.codec_status(AudioSide::Right).is_some());
    }

    #[test]
    fn codec_semantic_timeout_after_write_response_rolls_back() {
        use crate::hci_host::gatt::{
            GattOperation, GattResult,
        };

        let mut machine = machine_at_codec_planned();
        let actions = machine.begin_codec_configuration().unwrap();
        let actions = drive_ase_prepare(&mut machine, actions);

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteRequest { .. }
        ));
        let actions = complete_gatt(
            &mut machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: true,
            },
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(ControlTimer::Gatt { .. })
        )));
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringCodec {
                side: AudioSide::Left,
            }
        );

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::AseOperation {
                    side: AudioSide::Left,
                },
            ))
            .unwrap();
        let disconnect = actions
            .iter()
            .position(|action| matches!(
                action,
                ControlAction::DisconnectAcl { .. }
            ))
            .expect("semantic timeout must begin ACL rollback");
        assert!(actions[..disconnect].iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(
                ControlTimer::AseOperation {
                    side: AudioSide::Left
                }
            )
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::CancelGatt { .. }
        )));
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::AseOperationTimeout {
                side: AudioSide::Left,
            })
        ));
    }

    fn machine_at_codec_configured() -> ControlMachine {
        let mut machine = machine_at_codec_planned();
        machine.phase = ControlPhase::CodecConfigured;
        machine
    }

    fn drive_to_qos_planned() -> ControlMachine {
        let mut machine = machine_at_codec_planned();

        let actions = machine.begin_codec_configuration().unwrap();
        let actions = drive_ase_prepare(&mut machine, actions);
        let actions =
            drive_config_codec(&mut machine, AudioSide::Left, actions);
        let actions = drive_ase_prepare(&mut machine, actions);
        let _actions =
            drive_config_codec(&mut machine, AudioSide::Right, actions);
        assert_eq!(machine.phase(), ControlPhase::CodecConfigured);

        machine.plan_iso().unwrap();
        let _ = machine.begin_cig_configuration().unwrap();
        machine
            .on_hci_report(&HciReport::CigConfigureState(
                CigConfigureState::Ready(CigAllocation {
                    cig_id: 1,
                    cis: vec![
                        crate::hci_host::iso::CisAllocation {
                            cis_id: 1,
                            connection_handle: 0x0900,
                        },
                        crate::hci_host::iso::CisAllocation {
                            cis_id: 2,
                            connection_handle: 0x0901,
                        },
                    ],
                }),
            ))
            .unwrap();
        assert_eq!(machine.phase(), ControlPhase::CigConfigured);

        machine.plan_qos().unwrap();
        assert_eq!(machine.phase(), ControlPhase::QosPlanned);
        machine
    }

    fn qos_status_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseEndpoint, AudioRole,
        };

        let planned = match side {
            AudioSide::Left => &machine.qos_plan().unwrap().left,
            AudioSide::Right => &machine.qos_plan().unwrap().right,
        };
        let mut endpoint =
            AseEndpoint::new(planned.parameters.ase_id, AudioRole::Sink);
        endpoint.state = AseState::QOS_CONFIGURED;
        endpoint.qos = planned.expected_readback();
        endpoint.value().unwrap()
    }

    fn qos_cp_success_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        let ase_id = match side {
            AudioSide::Left => {
                machine.qos_plan().unwrap().left.parameters.ase_id
            }
            AudioSide::Right => {
                machine.qos_plan().unwrap().right.parameters.ase_id
            }
        };
        AseControlResponse {
            opcode: AseOpcode::ConfigQos,
            responses: vec![AseResponse {
                ase_id,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    fn drive_config_qos(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::hci_host::gatt::{
            GattOperation, GattResult, GattValueEvent,
        };

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteRequest {
                attribute_handle: 0x0043,
                ..
            }
        ));

        let actions = complete_gatt(
            machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: true,
            },
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(ControlTimer::Gatt { .. })
        )));

        assert!(machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0043,
                        value: qos_cp_success_bytes(machine, side),
                    },
                ),
            ))
            .unwrap()
            .is_empty());

        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0046,
                        value: qos_status_bytes(machine, side),
                    },
                ),
            ))
            .unwrap();
        let readback = only_begin_gatt(&actions);
        assert_eq!(
            readback.operation,
            GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        );

        complete_gatt(
            machine,
            &readback,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: qos_status_bytes(machine, side),
            },
        )
    }

    #[test]
    fn qos_configuration_runs_left_then_right_to_qos_configured() {
        let mut machine = drive_to_qos_planned();

        let actions = machine.begin_qos_configuration().unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringQos {
                side: AudioSide::Left,
            }
        );

        let actions =
            drive_config_qos(&mut machine, AudioSide::Left, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringQos {
                side: AudioSide::Right,
            }
        );
        assert!(machine.qos_status(AudioSide::Left).is_some());

        let actions =
            drive_config_qos(&mut machine, AudioSide::Right, actions);
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(
                ControlTimer::AseOperation {
                    side: AudioSide::Right,
                }
            )
        )));
        assert_eq!(machine.phase(), ControlPhase::QosConfigured);
        assert!(machine.qos_status(AudioSide::Left).is_some());
        assert!(machine.qos_status(AudioSide::Right).is_some());
    }

    fn drive_to_enable_planned() -> ControlMachine {
        let mut machine = drive_to_qos_planned();

        let actions = machine.begin_qos_configuration().unwrap();
        let actions =
            drive_config_qos(&mut machine, AudioSide::Left, actions);
        let _actions =
            drive_config_qos(&mut machine, AudioSide::Right, actions);
        assert_eq!(machine.phase(), ControlPhase::QosConfigured);

        machine.plan_enable().unwrap();
        assert_eq!(machine.phase(), ControlPhase::EnablePlanned);
        machine
    }

    fn enable_status_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::{
            ascs::{AseEndpoint, AudioRole},
            le_audio::Metadata,
        };

        let planned = match side {
            AudioSide::Left => &machine.enable_plan().unwrap().left,
            AudioSide::Right => &machine.enable_plan().unwrap().right,
        };
        let qos = match side {
            AudioSide::Left => {
                &machine.qos_plan().unwrap().left
            }
            AudioSide::Right => {
                &machine.qos_plan().unwrap().right
            }
        };

        let mut endpoint =
            AseEndpoint::new(planned.ase_id, AudioRole::Sink);
        endpoint.state = AseState::ENABLING;
        endpoint.qos = qos.expected_readback();
        endpoint.metadata =
            Metadata::from_bytes(&planned.metadata).unwrap();
        endpoint.value().unwrap()
    }

    fn enable_cp_success_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        let ase_id = match side {
            AudioSide::Left => {
                machine.enable_plan().unwrap().left.ase_id
            }
            AudioSide::Right => {
                machine.enable_plan().unwrap().right.ase_id
            }
        };

        AseControlResponse {
            opcode: AseOpcode::Enable,
            responses: vec![AseResponse {
                ase_id,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    fn drive_enable(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::hci_host::gatt::{
            GattOperation, GattResult, GattValueEvent,
        };

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteRequest {
                attribute_handle: 0x0043,
                ..
            }
        ));

        let actions = complete_gatt(
            machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: true,
            },
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(ControlTimer::Gatt { .. })
        )));

        assert!(machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0043,
                        value: enable_cp_success_bytes(machine, side),
                    },
                ),
            ))
            .unwrap()
            .is_empty());

        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0046,
                        value: enable_status_bytes(machine, side),
                    },
                ),
            ))
            .unwrap();

        let readback = only_begin_gatt(&actions);
        assert_eq!(
            readback.operation,
            GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        );

        complete_gatt(
            machine,
            &readback,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: enable_status_bytes(machine, side),
            },
        )
    }

    #[test]
    fn enable_runs_left_then_right_to_enabled() {
        use bumble_profiles::{
            bap::ContextType,
            le_audio::MetadataTag,
        };

        let mut machine = drive_to_enable_planned();

        let actions = machine.begin_enable().unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::Enabling {
                side: AudioSide::Left,
            }
        );

        let actions =
            drive_enable(&mut machine, AudioSide::Left, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::Enabling {
                side: AudioSide::Right,
            }
        );
        assert!(machine.enable_status(AudioSide::Left).is_some());

        let actions =
            drive_enable(&mut machine, AudioSide::Right, actions);
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(
                ControlTimer::AseOperation {
                    side: AudioSide::Right,
                }
            )
        )));
        assert_eq!(machine.phase(), ControlPhase::Enabled);
        assert!(machine.enable_status(AudioSide::Left).is_some());
        assert!(machine.enable_status(AudioSide::Right).is_some());

        for side in [AudioSide::Left, AudioSide::Right] {
            let status = machine.enable_status(side).unwrap();
            let AseStatusDetails::Metadata {
                metadata, ..
            } = &status.details
            else {
                panic!("expected Enable metadata readback");
            };
            assert!(metadata.entries.iter().any(|entry| {
                entry.tag == MetadataTag::STREAMING_AUDIO_CONTEXTS
                    && entry.data
                        == ContextType::MEDIA.0.to_le_bytes()
            }));
        }
    }

    #[test]
    fn set_cig_success_reaches_cig_configured_without_retaining_raw_handles() {
        let mut machine = machine_at_codec_configured();

        machine.plan_iso().unwrap();
        assert_eq!(machine.phase(), ControlPhase::IsoPlanned);
        let plan = machine.iso_plan().unwrap().clone();
        assert_eq!(plan.cig_id, 1);
        assert_eq!(plan.left.cis_id, 1);
        assert_eq!(plan.right.cis_id, 2);

        let actions = machine.begin_cig_configuration().unwrap();
        assert_eq!(machine.phase(), ControlPhase::ConfiguringCig);
        assert_eq!(
            actions,
            vec![
                ControlAction::ConfigureCig(plan.cig_config()),
                ControlAction::ArmTimer(ControlTimer::CigConfig),
            ]
        );

        let allocation = CigAllocation {
            cig_id: 1,
            cis: vec![
                crate::hci_host::iso::CisAllocation {
                    cis_id: 1,
                    connection_handle: 0x0900,
                },
                crate::hci_host::iso::CisAllocation {
                    cis_id: 2,
                    connection_handle: 0x0901,
                },
            ],
        };
        let actions = machine
            .on_hci_report(&HciReport::CigConfigureState(
                CigConfigureState::Ready(allocation),
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::CigConfig,
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::CigConfigured);
        assert!(machine.cig_may_exist);
        // Control retains only the semantic plan. The controller-assigned
        // 0x0900/0x0901 handles from CigAllocation are intentionally not
        // represented in ControlMachine state.
        assert_eq!(machine.iso_plan().unwrap(), &plan);
    }

    #[test]
    fn set_cig_uncertain_failure_removes_cig_before_acl_rollback() {
        let mut machine = machine_at_codec_configured();
        machine.plan_iso().unwrap();
        machine.begin_cig_configuration().unwrap();

        let failure = CigConfigureFailure {
            cig_id: 1,
            may_have_allocated: true,
            kind: crate::hci_host::iso::CigConfigureFailureKind::
                MalformedResponse,
        };
        let actions = machine
            .on_hci_report(&HciReport::CigConfigureState(
                CigConfigureState::Failed(failure.clone()),
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::CigConfig),
                ControlAction::RemoveCig { cig_id: 1 },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupCig { cig_id: 1 },
                ),
            ]
        );
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::CigConfigure(actual))
                if actual == &failure
        ));

        let actions = machine
            .on_hci_report(&HciReport::CigRemoveState(
                CigRemoveState::Removed { cig_id: 1 },
            ))
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupCig { cig_id: 1 },
            )
        );
        assert_eq!(
            actions[1],
            ControlAction::DisconnectAcl {
                connection: ConnectionToken {
                    connection_handle: 0x0801,
                    generation: 2,
                },
            }
        );
        assert_eq!(
            actions[2],
            ControlAction::ArmTimer(
                ControlTimer::CleanupDisconnect(ConnectionToken {
                    connection_handle: 0x0801,
                    generation: 2,
                }),
            )
        );
        assert!(!machine.cig_may_exist);
    }

    #[test]
    fn set_cig_timeout_assumes_possible_allocation_and_removes_cig_first() {
        let mut machine = machine_at_codec_configured();
        machine.plan_iso().unwrap();
        machine.begin_cig_configuration().unwrap();

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::CigConfig,
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::CigConfig),
                ControlAction::RemoveCig { cig_id: 1 },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupCig { cig_id: 1 },
                ),
            ]
        );
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(machine.cig_may_exist);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::CigConfigureTimeout {
                cig_id: 1,
            })
        ));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::DisconnectAcl { .. }
        )));
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
                    control_point:
                        crate::control::audio_snapshot::GattCharacteristicRange {
                            declaration_handle: 8,
                            value_handle: 9,
                            end_group_handle: 10,
                            properties: 0x12,
                        },
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
