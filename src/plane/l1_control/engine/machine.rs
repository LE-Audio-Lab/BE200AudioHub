use std::collections::BTreeSet;

use bumble_profiles::ascs::{
    AseOperation, AseState, AseStatus, AseStatusDetails,
};

use crate::{
    architecture::{
        control::{ControlSnapshot, ControlState, StreamBinding, StreamTopology},
        types::{AudioSide, SessionId, StreamId},
    },
    plane::l1_hci::interface::report::HciReport,
    plane::l1_hci::{
        iso::{
            cig_cis::{
                CigAllocation, CigConfig, CigConfigureFailure,
                CigConfigureState, CisCreateRequest, CisRef,
                SemanticCisCreateFailure, SemanticCisCreateState,
            },
            feature::{CisHostFeatureFailure, CisHostFeatureState},
            lifecycle::{
                CigRemoveFailure, CigRemoveState,
                CisDisconnectFailureKind, SemanticCisDisconnectState,
            },
            path::{
                IsoDataPathDirection, SemanticIsoPathFailure,
                SemanticIsoPathState,
            },
            route::{
                StereoStreamRouteBinding, StreamRouteFailure, StreamRouteState,
            },
        },
        host::engine::{
            ControllerIdentityFailure, OperationalAttachFailure,
            OperationalAttachState, PassiveScanState,
        },
        acl::{
            connection::{
                ConnectionToken, DisconnectFailure, DisconnectState,
                FalConnectFailure, FalConnectRequest, FalConnectionState,
                LeConnectionSetSnapshot,
            },
            privacy::{
                ControllerPrivacyState, PrivacyFailure, ResolvingListConfig,
            },
            security::{
                StoredEncryptionFailure, StoredEncryptionRequest,
                StoredEncryptionState,
            },
        },
        gatt::transaction::{GattReport, GattRequest, GattRequestId},
    },
};

use crate::plane::l1_control::{
    ascs::{
        operation::{
            AseOperationAction, AseOperationFailure, AseOperationMachine,
            AsePrepareAction, AsePrepareFailure, AsePrepareMachine,
            PreparedAseControl,
        },
        streaming::{
            StreamingGateAction, StreamingGateFailure,
            StreamingGateMachine, StreamingGatePhase,
        },
    },
    discovery::{
        audio::{
            AudioDiscoveryAction, AudioDiscoveryFailure,
            AudioDiscoveryMachine,
        },
        peer::{
            resolve_connectable_members, ControlDiscoveryError,
            ResolvedMemberPresence,
        },
        snapshot::{AudioMemberSnapshot, AudioSetSnapshot},
    },
    model::set_plan::{ControlPlanError, ControlSetPlan},
    planning::{
        codec::{plan_first_stream, CodecPlanError, StereoCodecPlan},
        enable::{plan_first_enable, EnablePlanError, StereoEnablePlan},
        iso::{plan_first_iso, IsoPlanError, StereoIsoPlan},
        qos::{plan_first_qos, QosPlanError, StereoQosPlan},
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
    CreatingCis,
    CisEstablished,
    SettingUpIsoPath { side: AudioSide },
    IsoPathsReady,
    AwaitingStreaming { side: AudioSide },
    StreamingReady,
    BindingStreamRoute {
        session_id: SessionId,
        stream_id: StreamId,
    },
    StreamBoundReady,
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
    CisCreate,
    IsoPathSetup { cis: CisRef },
    StreamingProbe { side: AudioSide },
    StreamingDeadline { side: AudioSide },
    StreamRouteBind {
        session_id: SessionId,
        stream_id: StreamId,
    },
    CleanupStreamRoute {
        session_id: SessionId,
        stream_id: StreamId,
    },
    CleanupIsoPath { cis: CisRef },
    CleanupCis { cis: CisRef },
    CleanupAse { side: AudioSide },
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
    CreateCis(CisCreateRequest),
    SetupIsoInputPathByRef { cis: CisRef },
    RemoveIsoPathByRef {
        cis: CisRef,
        directions: u8,
    },
    DisconnectCisByRef { cis: CisRef },
    RemoveCig { cig_id: u8 },
    BindStreamRoute(StereoStreamRouteBinding),
    UnbindStreamRoute {
        session_id: SessionId,
        stream_id: StreamId,
    },
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
    IsoPathRemove(SemanticIsoPathFailure),
    CisDisconnect {
        cis: CisRef,
        kind: CisDisconnectFailureKind,
    },
    AseRelease {
        side: AudioSide,
        failure: AseOperationFailure,
    },
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
    CisCreate(SemanticCisCreateFailure),
    CisCreateMismatch {
        expected_cig_id: u8,
        actual_cig_id: u8,
        expected_cis_ids: Vec<u8>,
        actual_cis_ids: Vec<u8>,
    },
    CisCreateTimeout {
        cig_id: u8,
    },
    IsoPathSetup {
        cis: CisRef,
        failure: SemanticIsoPathFailure,
    },
    IsoPathSetupMismatch {
        expected: CisRef,
        actual: CisRef,
    },
    IsoPathSetupTimeout {
        cis: CisRef,
    },
    StreamingGate {
        side: AudioSide,
        failure: StreamingGateFailure,
    },
    StreamingGattTimeout {
        side: AudioSide,
        request_id: GattRequestId,
    },
    StreamingDeadline {
        side: AudioSide,
    },
    StreamRoute(StreamRouteFailure),
    StreamRouteTimeout {
        session_id: SessionId,
        stream_id: StreamId,
    },
    StreamRouteUnexpectedUnbound {
        session_id: SessionId,
        stream_id: StreamId,
        was_bound: bool,
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
    live_cis: BTreeSet<CisRef>,
    iso_paths_ready: BTreeSet<CisRef>,
    iso_paths_may_exist: BTreeSet<CisRef>,
    streaming_gate: Option<StreamingGateMachine>,
    streaming_statuses: [Option<AseStatus>; 2],
    pending_stream_binding: Option<StreamBinding>,
    active_stream_binding: Option<StreamBinding>,
    rollback_stream_route: Option<(SessionId, StreamId)>,
    rollback_stream_route_unbind_pending: bool,
    rollback_iso_path_queue: Vec<CisRef>,
    rollback_iso_path_current: Option<CisRef>,
    rollback_cis_queue: Vec<CisRef>,
    rollback_cis_current: Option<CisRef>,
    rollback_ase_queue: Vec<AudioSide>,
    rollback_ase_current: Option<AudioSide>,
    rollback_ase_operation: Option<AseOperationMachine>,
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
            live_cis: BTreeSet::new(),
            iso_paths_ready: BTreeSet::new(),
            iso_paths_may_exist: BTreeSet::new(),
            streaming_gate: None,
            streaming_statuses: [None, None],
            pending_stream_binding: None,
            active_stream_binding: None,
            rollback_stream_route: None,
            rollback_stream_route_unbind_pending: false,
            rollback_iso_path_queue: Vec::new(),
            rollback_iso_path_current: None,
            rollback_cis_queue: Vec::new(),
            rollback_cis_current: None,
            rollback_ase_queue: Vec::new(),
            rollback_ase_current: None,
            rollback_ase_operation: None,
            failure: None,
        })
    }

    pub fn phase(&self) -> ControlPhase {
        self.phase
    }

    /// Stable observation projection for Core/RuntimeState readers.
    ///
    /// Fine-grained ControlPhase remains private implementation detail. The
    /// exported ControlState intentionally changes much less often.
    pub fn snapshot(&self) -> ControlSnapshot {
        let state = match self.phase {
            ControlPhase::Idle => ControlState::Idle,
            ControlPhase::Attaching
            | ControlPhase::ReadingControllerIdentity
            | ControlPhase::EnsuringCisHostFeature
            | ControlPhase::StartingDiscovery
            | ControlPhase::DiscoveringMembers
            | ControlPhase::StoppingDiscovery => ControlState::Resolving,
            ControlPhase::ConfiguringPrivacy
            | ControlPhase::ConnectingMember { .. }
            | ControlPhase::CancellingConnection { .. }
            | ControlPhase::AwaitingSetConnected
            | ControlPhase::SetConnected => ControlState::Connecting,
            ControlPhase::SecuringMember { .. } => ControlState::Securing,
            ControlPhase::DiscoveringAudioMember { .. }
            | ControlPhase::AudioSetDiscovered => ControlState::Discovering,
            ControlPhase::CodecPlanned
            | ControlPhase::PreparingCodecAse { .. }
            | ControlPhase::ConfiguringCodec { .. }
            | ControlPhase::CodecConfigured
            | ControlPhase::IsoPlanned
            | ControlPhase::ConfiguringCig
            | ControlPhase::CigConfigured
            | ControlPhase::QosPlanned
            | ControlPhase::ConfiguringQos { .. }
            | ControlPhase::QosConfigured
            | ControlPhase::EnablePlanned
            | ControlPhase::Enabling { .. }
            | ControlPhase::Enabled
            | ControlPhase::CreatingCis
            | ControlPhase::CisEstablished
            | ControlPhase::SettingUpIsoPath { .. }
            | ControlPhase::IsoPathsReady
            | ControlPhase::AwaitingStreaming { .. } => ControlState::Configuring,
            ControlPhase::StreamingReady
            | ControlPhase::BindingStreamRoute { .. }
            | ControlPhase::StreamBoundReady => ControlState::StreamReady,
            ControlPhase::RollingBack => ControlState::TearingDown,
            ControlPhase::Faulted => ControlState::Faulted,
        };

        let binding = self.active_stream_binding.or(self.pending_stream_binding);
        ControlSnapshot {
            state,
            stream_id: binding.map(|binding| binding.stream_id),
            format: binding.map(|binding| binding.format),
        }
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

    pub fn live_cis(&self) -> &BTreeSet<CisRef> {
        &self.live_cis
    }

    pub fn iso_paths_ready(&self) -> &BTreeSet<CisRef> {
        &self.iso_paths_ready
    }

    pub fn streaming_status(
        &self,
        side: AudioSide,
    ) -> Option<&AseStatus> {
        self.streaming_statuses[side_index(side)].as_ref()
    }

    pub fn stream_binding(
        &self,
    ) -> Result<StreamBinding, ControlFailure> {
        if self.phase != ControlPhase::StreamBoundReady {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        Ok(self
            .active_stream_binding
            .expect("StreamBoundReady requires active binding"))
    }

    pub fn begin_stream_handoff(
        &mut self,
        session_id: SessionId,
        stream_id: StreamId,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::StreamingReady {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }

        let codec = self
            .codec_plan
            .as_ref()
            .expect("StreamingReady requires codec plan");
        let qos = self
            .qos_plan
            .as_ref()
            .expect("StreamingReady requires QoS plan");
        let left = self
            .cis_for_side(AudioSide::Left)
            .expect("StreamingReady requires left CIS");
        let right = self
            .cis_for_side(AudioSide::Right)
            .expect("StreamingReady requires right CIS");

        let binding = StreamBinding {
            session_id,
            stream_id,
            format: codec.format,
            presentation_delay_us:
                qos.left.parameters.presentation_delay,
            topology: StreamTopology::StereoSplit,
        };
        self.pending_stream_binding = Some(binding);
        self.active_stream_binding = None;
        self.phase = ControlPhase::BindingStreamRoute {
            session_id,
            stream_id,
        };

        Ok(vec![
            ControlAction::BindStreamRoute(
                StereoStreamRouteBinding {
                    session_id,
                    stream_id,
                    left,
                    right,
                },
            ),
            ControlAction::ArmTimer(
                ControlTimer::StreamRouteBind {
                    session_id,
                    stream_id,
                },
            ),
        ])
    }

    pub fn begin_streaming_gate(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::IsoPathsReady {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.streaming_gate = None;
        self.streaming_statuses = [None, None];
        self.pending_stream_binding = None;
        self.active_stream_binding = None;
        self.rollback_stream_route = None;
        self.rollback_stream_route_unbind_pending = false;
        self.start_streaming_gate(AudioSide::Left)
    }

    pub fn begin_iso_path_setup(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::CisEstablished {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.iso_paths_ready.clear();
        self.iso_paths_may_exist.clear();
        self.streaming_gate = None;
        self.streaming_statuses = [None, None];
        self.start_iso_path_setup(AudioSide::Left)
    }

    pub fn begin_cis_creation(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::Enabled {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let request = self
            .iso_plan
            .as_ref()
            .expect("Enabled requires ISO plan")
            .cis_create_request();
        self.live_cis.clear();
        self.iso_paths_ready.clear();
        self.iso_paths_may_exist.clear();
        self.streaming_gate = None;
        self.streaming_statuses = [None, None];
        self.phase = ControlPhase::CreatingCis;
        Ok(vec![
            ControlAction::CreateCis(request),
            ControlAction::ArmTimer(ControlTimer::CisCreate),
        ])
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
        self.live_cis.clear();
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
        self.live_cis.clear();
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
            ) if matches!(
                self.phase,
                ControlPhase::AwaitingStreaming { side }
                    if self.planned_sink(side)
                        .map(|sink| sink.member_index)
                        .ok() == Some(member_index)
            )
                && self
                    .streaming_gate
                    .as_ref()
                    .and_then(|gate| gate.pending_request_id())
                    == Some(request_id) =>
            {
                let side = match self.phase {
                    ControlPhase::AwaitingStreaming { side } => side,
                    _ => unreachable!(),
                };
                let connection = self.planned_sink(side)?.connection;
                self.streaming_gate = None;
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
                        ControlTimer::StreamingProbe { side },
                    ),
                    ControlAction::DisarmTimer(
                        ControlTimer::StreamingDeadline { side },
                    ),
                ];
                actions.extend(self.begin_stream_rollback(
                    ControlFailure::StreamingGattTimeout {
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
                ControlTimer::StreamingProbe { side },
            ) if self.phase == ControlPhase::AwaitingStreaming { side }
                && self
                    .streaming_gate
                    .as_ref()
                    .map(|gate| gate.phase())
                    == Some(StreamingGatePhase::WaitingRetry) =>
            {
                let mut gate = self
                    .streaming_gate
                    .take()
                    .expect("streaming retry requires gate");
                let action = match gate.retry() {
                    Ok(action) => action,
                    Err(failure) => {
                        let mut actions = vec![
                            ControlAction::DisarmTimer(
                                ControlTimer::StreamingProbe { side },
                            ),
                            ControlAction::DisarmTimer(
                                ControlTimer::StreamingDeadline { side },
                            ),
                        ];
                        actions.extend(self.begin_stream_rollback(
                            ControlFailure::StreamingGate {
                                side,
                                failure,
                            },
                        )?);
                        return Ok(actions);
                    }
                };
                self.next_gatt_request_id = gate.next_request_id();
                self.streaming_gate = Some(gate);
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::StreamingProbe { side },
                )];
                actions.extend(
                    self.translate_streaming_action(
                        side,
                        None,
                        action,
                    )?,
                );
                Ok(actions)
            }
            ControlEvent::TimerExpired(
                ControlTimer::StreamingDeadline { side },
            ) if self.phase == ControlPhase::AwaitingStreaming { side } =>
            {
                let pending = self
                    .streaming_gate
                    .as_ref()
                    .and_then(|gate| gate.pending_request_id());
                self.streaming_gate = None;
                let mut actions = vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::StreamingProbe { side },
                    ),
                    ControlAction::DisarmTimer(
                        ControlTimer::StreamingDeadline { side },
                    ),
                ];
                if let Some(request_id) = pending {
                    let member_index =
                        self.planned_sink(side)?.member_index;
                    actions.push(ControlAction::CancelGatt {
                        connection: self.planned_sink(side)?.connection,
                        request_id,
                    });
                    actions.push(ControlAction::DisarmTimer(
                        ControlTimer::Gatt {
                            member_index,
                            request_id,
                        },
                    ));
                }
                actions.extend(self.begin_stream_rollback(
                    ControlFailure::StreamingDeadline { side },
                )?);
                Ok(actions)
            }
            ControlEvent::TimerExpired(
                ControlTimer::StreamRouteBind {
                    session_id,
                    stream_id,
                },
            ) if self.phase
                == ControlPhase::BindingStreamRoute {
                    session_id,
                    stream_id,
                } =>
            {
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::StreamRouteBind {
                        session_id,
                        stream_id,
                    },
                )];
                actions.extend(self.begin_stream_rollback(
                    ControlFailure::StreamRouteTimeout {
                        session_id,
                        stream_id,
                    },
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
                ControlTimer::CisCreate,
            ) if self.phase == ControlPhase::CreatingCis =>
            {
                let cig_id = self
                    .iso_plan
                    .as_ref()
                    .expect("CreatingCis requires ISO plan")
                    .cig_id;
                let failure =
                    ControlFailure::CisCreateTimeout { cig_id };
                self.phase = ControlPhase::Faulted;
                self.failure = Some(failure);
                Ok(vec![ControlAction::DisarmTimer(
                    ControlTimer::CisCreate,
                )])
            }
            ControlEvent::TimerExpired(
                ControlTimer::IsoPathSetup { cis },
            ) if matches!(
                self.phase,
                ControlPhase::SettingUpIsoPath { side }
                    if self.cis_for_side(side) == Some(cis)
            ) =>
            {
                let failure =
                    ControlFailure::IsoPathSetupTimeout { cis };
                self.phase = ControlPhase::Faulted;
                self.failure = Some(failure);
                Ok(vec![ControlAction::DisarmTimer(
                    ControlTimer::IsoPathSetup { cis },
                )])
            }
            ControlEvent::TimerExpired(
                ControlTimer::CleanupStreamRoute {
                    session_id,
                    stream_id,
                },
            ) if self.phase == ControlPhase::RollingBack
                && self.rollback_stream_route
                    == Some((session_id, stream_id))
                && self.rollback_stream_route_unbind_pending =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupStreamRoute {
                            session_id,
                            stream_id,
                        },
                    ),
                );
                self.rollback_stream_route = None;
                self.rollback_stream_route_unbind_pending = false;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CleanupStreamRoute {
                        session_id,
                        stream_id,
                    },
                )];
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
            ControlEvent::TimerExpired(
                ControlTimer::CleanupIsoPath { cis },
            ) if self.phase == ControlPhase::RollingBack
                && self.rollback_iso_path_current == Some(cis) =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupIsoPath { cis },
                    ),
                );
                // Path state is uncertain; disconnecting the CIS is unsafe.
                self.finish_rollback()
            }
            ControlEvent::TimerExpired(
                ControlTimer::CleanupCis { cis },
            ) if self.phase == ControlPhase::RollingBack
                && self.rollback_cis_current == Some(cis) =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupCis { cis },
                    ),
                );
                // Do not continue to RemoveCig while a CIS may still be live.
                self.finish_rollback()
            }
            ControlEvent::TimerExpired(
                ControlTimer::CleanupAse { side },
            ) if self.phase == ControlPhase::RollingBack
                && self.rollback_ase_current == Some(side) =>
            {
                let pending = self
                    .rollback_ase_operation
                    .as_ref()
                    .and_then(|machine| machine.pending_request_id());
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupAse { side },
                    ),
                );
                self.rollback_ase_operation = None;
                self.rollback_ase_current = None;

                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CleanupAse { side },
                )];
                if let Some(request_id) = pending {
                    let member_index =
                        self.planned_sink(side)?.member_index;
                    actions.push(ControlAction::CancelGatt {
                        connection: self.planned_sink(side)?.connection,
                        request_id,
                    });
                    actions.push(ControlAction::DisarmTimer(
                        ControlTimer::Gatt {
                            member_index,
                            request_id,
                        },
                    ));
                }
                actions.extend(self.continue_rollback()?);
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
                ControlTimer::Gatt {
                    member_index,
                    request_id,
                },
            ) if self.phase == ControlPhase::RollingBack
                && self
                    .rollback_ase_operation
                    .as_ref()
                    .and_then(|machine| machine.pending_request_id())
                    == Some(request_id)
                && self
                    .rollback_ase_current
                    .and_then(|side| {
                        self.planned_sink(side)
                            .ok()
                            .map(|sink| sink.member_index)
                    })
                    == Some(member_index) =>
            {
                let side = self
                    .rollback_ase_current
                    .expect("rollback GATT timer requires ASE side");
                self.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::Gatt {
                            member_index,
                            request_id,
                        },
                    ),
                );
                self.rollback_ase_operation = None;
                self.rollback_ase_current = None;
                let mut actions = vec![
                    ControlAction::CancelGatt {
                        connection: self.planned_sink(side)?.connection,
                        request_id,
                    },
                    ControlAction::DisarmTimer(
                        ControlTimer::Gatt {
                            member_index,
                            request_id,
                        },
                    ),
                    ControlAction::DisarmTimer(
                        ControlTimer::CleanupAse { side },
                    ),
                ];
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
                ControlPhase::CreatingCis,
                HciReport::SemanticCisCreateState(
                    SemanticCisCreateState::AwaitingEstablished {
                        cig_id,
                        established_cis_ids,
                    },
                ),
            ) => {
                self.observe_semantic_cis_set(
                    *cig_id,
                    established_cis_ids,
                )?;
                Ok(Vec::new())
            }
            (
                ControlPhase::CreatingCis,
                HciReport::SemanticCisCreateState(
                    SemanticCisCreateState::Ready {
                        cig_id,
                        established_cis_ids,
                    },
                ),
            ) => {
                self.observe_semantic_cis_set(
                    *cig_id,
                    established_cis_ids,
                )?;
                if !self.semantic_cis_set_complete(
                    *cig_id,
                    established_cis_ids,
                ) {
                    let plan = self
                        .iso_plan
                        .as_ref()
                        .expect("CreatingCis requires ISO plan");
                    let mut expected =
                        vec![plan.left.cis_id, plan.right.cis_id];
                    expected.sort_unstable();
                    let mut actual = established_cis_ids.clone();
                    actual.sort_unstable();
                    return self.fail(
                        ControlFailure::CisCreateMismatch {
                            expected_cig_id: plan.cig_id,
                            actual_cig_id: *cig_id,
                            expected_cis_ids: expected,
                            actual_cis_ids: actual,
                        },
                    );
                }

                self.phase = ControlPhase::CisEstablished;
                Ok(vec![ControlAction::DisarmTimer(
                    ControlTimer::CisCreate,
                )])
            }
            (
                ControlPhase::CreatingCis,
                HciReport::SemanticCisCreateState(
                    SemanticCisCreateState::Failed(failure),
                ),
            ) => {
                self.observe_semantic_cis_set(
                    failure.cig_id,
                    &failure.established_cis_ids,
                )?;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CisCreate,
                )];
                actions.extend(self.begin_stream_rollback(
                    ControlFailure::CisCreate(failure.clone()),
                )?);
                Ok(actions)
            }
            (
                ControlPhase::SettingUpIsoPath { side },
                HciReport::SemanticIsoPathState(
                    SemanticIsoPathState::SetupComplete {
                        cis,
                        direction:
                            IsoDataPathDirection::Input,
                    },
                ),
            ) => {
                let expected = self
                    .cis_for_side(side)
                    .expect("ISO path setup requires ISO plan");
                if *cis != expected {
                    let failure =
                        ControlFailure::IsoPathSetupMismatch {
                            expected,
                            actual: *cis,
                        };
                    self.phase = ControlPhase::Faulted;
                    self.failure = Some(failure);
                    return Ok(vec![ControlAction::DisarmTimer(
                        ControlTimer::IsoPathSetup {
                            cis: expected,
                        },
                    )]);
                }

                self.iso_paths_ready.insert(*cis);
                self.iso_paths_may_exist.insert(*cis);
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::IsoPathSetup { cis: *cis },
                )];
                match side {
                    AudioSide::Left => {
                        actions.extend(
                            self.start_iso_path_setup(
                                AudioSide::Right,
                            )?,
                        );
                    }
                    AudioSide::Right => {
                        self.phase = ControlPhase::IsoPathsReady;
                    }
                }
                Ok(actions)
            }
            (
                ControlPhase::SettingUpIsoPath { side },
                HciReport::SemanticIsoPathState(
                    SemanticIsoPathState::Failed(failure),
                ),
            ) => {
                let expected = self
                    .cis_for_side(side)
                    .expect("ISO path setup requires ISO plan");
                if failure.cis != expected {
                    let mismatch =
                        ControlFailure::IsoPathSetupMismatch {
                            expected,
                            actual: failure.cis,
                        };
                    self.phase = ControlPhase::Faulted;
                    self.failure = Some(mismatch);
                    return Ok(vec![ControlAction::DisarmTimer(
                        ControlTimer::IsoPathSetup {
                            cis: expected,
                        },
                    )]);
                }
                if failure.may_have_configured {
                    self.iso_paths_may_exist.insert(failure.cis);
                }
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: failure.cis,
                    },
                )];
                actions.extend(self.begin_stream_rollback(
                    ControlFailure::IsoPathSetup {
                        cis: failure.cis,
                        failure: failure.clone(),
                    },
                )?);
                Ok(actions)
            }
            (
                ControlPhase::BindingStreamRoute {
                    session_id,
                    stream_id,
                },
                HciReport::StreamRouteState(
                    StreamRouteState::Bound {
                        session_id: actual_session,
                        stream_id: actual_stream,
                    },
                ),
            ) if session_id == *actual_session
                && stream_id == *actual_stream =>
            {
                let binding = self
                    .pending_stream_binding
                    .take()
                    .expect("binding phase requires pending StreamBinding");
                self.active_stream_binding = Some(binding);
                self.phase = ControlPhase::StreamBoundReady;
                Ok(vec![ControlAction::DisarmTimer(
                    ControlTimer::StreamRouteBind {
                        session_id,
                        stream_id,
                    },
                )])
            }
            (
                ControlPhase::BindingStreamRoute {
                    session_id,
                    stream_id,
                },
                HciReport::StreamRouteState(
                    StreamRouteState::Failed(failure),
                ),
            ) if session_id == failure.session_id
                && stream_id == failure.stream_id =>
            {
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::StreamRouteBind {
                        session_id,
                        stream_id,
                    },
                )];
                actions.extend(self.begin_stream_rollback(
                    ControlFailure::StreamRoute(failure.clone()),
                )?);
                Ok(actions)
            }
            (
                ControlPhase::BindingStreamRoute {
                    session_id,
                    stream_id,
                },
                HciReport::StreamRouteState(
                    StreamRouteState::Unbound {
                        session_id: actual_session,
                        stream_id: actual_stream,
                        was_bound,
                    },
                ),
            ) if session_id == *actual_session
                && stream_id == *actual_stream =>
            {
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::StreamRouteBind {
                        session_id,
                        stream_id,
                    },
                )];
                actions.extend(self.begin_stream_rollback(
                    ControlFailure::StreamRouteUnexpectedUnbound {
                        session_id,
                        stream_id,
                        was_bound: *was_bound,
                    },
                )?);
                Ok(actions)
            }
            (
                ControlPhase::RollingBack,
                HciReport::StreamRouteState(
                    StreamRouteState::Unbound {
                        session_id,
                        stream_id,
                        ..
                    },
                ),
            ) if self.rollback_stream_route
                == Some((*session_id, *stream_id))
                && self.rollback_stream_route_unbind_pending =>
            {
                self.rollback_stream_route = None;
                self.rollback_stream_route_unbind_pending = false;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CleanupStreamRoute {
                        session_id: *session_id,
                        stream_id: *stream_id,
                    },
                )];
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
            (
                ControlPhase::RollingBack,
                HciReport::SemanticIsoPathState(
                    SemanticIsoPathState::RemoveComplete {
                        cis,
                        directions: 0x01,
                    },
                ),
            ) if self.rollback_iso_path_current == Some(*cis) => {
                self.iso_paths_ready.remove(cis);
                self.iso_paths_may_exist.remove(cis);
                self.rollback_iso_path_current = None;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CleanupIsoPath { cis: *cis },
                )];
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
            (
                ControlPhase::RollingBack,
                HciReport::SemanticIsoPathState(
                    SemanticIsoPathState::Failed(failure),
                ),
            ) if self.rollback_iso_path_current == Some(failure.cis) =>
            {
                self.cleanup_failures.push(
                    ControlCleanupFailure::IsoPathRemove(
                        failure.clone(),
                    ),
                );
                // A possibly-live data path forbids safe CIS teardown.
                self.finish_rollback()
            }
            (
                ControlPhase::RollingBack,
                HciReport::SemanticCisDisconnectState(
                    SemanticCisDisconnectState::Disconnected {
                        cis,
                        ..
                    },
                ),
            ) if self.rollback_cis_current == Some(*cis) => {
                self.live_cis.remove(cis);
                self.rollback_cis_current = None;
                let mut actions = vec![ControlAction::DisarmTimer(
                    ControlTimer::CleanupCis { cis: *cis },
                )];
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
            (
                ControlPhase::RollingBack,
                HciReport::SemanticCisDisconnectState(
                    SemanticCisDisconnectState::Failed {
                        cis,
                        kind,
                    },
                ),
            ) if self.rollback_cis_current == Some(*cis) => {
                self.cleanup_failures.push(
                    ControlCleanupFailure::CisDisconnect {
                        cis: *cis,
                        kind: kind.clone(),
                    },
                );
                // A failed CIS disconnect means RemoveCig is unsafe.
                self.finish_rollback()
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
                ControlPhase::AwaitingStreaming { side },
                HciReport::Gatt(report),
            ) => self.on_streaming_gatt_report(side, report),
            (
                ControlPhase::RollingBack,
                HciReport::Gatt(report),
            ) if self.rollback_ase_operation.is_some() => {
                self.on_rollback_ase_gatt_report(report)
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

    fn start_streaming_gate(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let cis = self
            .cis_for_side(side)
            .ok_or(ControlFailure::InvalidStartState(self.phase))?;
        if !self.iso_paths_ready.contains(&cis) {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let prepared = self.prepared_ases[side_index(side)]
            .as_ref()
            .expect("streaming gate requires prepared ASE")
            .clone();
        let context = self
            .enable_plan
            .as_ref()
            .expect("streaming gate requires Enable plan")
            .context;
        let mut gate = StreamingGateMachine::new(
            prepared,
            cis,
            context,
            self.next_gatt_request_id,
        );
        let action = gate.start().map_err(|failure| {
            ControlFailure::StreamingGate { side, failure }
        })?;
        self.next_gatt_request_id = gate.next_request_id();
        self.streaming_gate = Some(gate);
        self.phase = ControlPhase::AwaitingStreaming { side };

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::StreamingDeadline { side },
        )];
        actions.extend(
            self.translate_streaming_action(side, None, action)?,
        );
        Ok(actions)
    }

    fn on_streaming_gatt_report(
        &mut self,
        side: AudioSide,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut gate = self
            .streaming_gate
            .take()
            .expect("streaming phase missing gate");
        let pending_before = gate.pending_request_id();
        let connection = self.planned_sink(side)?.connection;
        let completed_request = match report {
            GattReport::Complete {
                request_id,
                connection: actual,
                ..
            }
            | GattReport::BearerPoisoned {
                connection: actual,
                request_id,
            } if Some(*request_id) == pending_before
                && *actual == connection =>
            {
                Some(*request_id)
            }
            GattReport::Failed(failure)
                if Some(failure.request_id) == pending_before
                    && failure.connection == connection =>
            {
                Some(failure.request_id)
            }
            _ => None,
        };

        let outcome = gate.on_gatt_report(report);
        let pending_after = gate.pending_request_id();
        self.next_gatt_request_id = gate.next_request_id();

        match outcome {
            Ok(StreamingGateAction::Ignored) => {
                self.streaming_gate = Some(gate);
                Ok(Vec::new())
            }
            Ok(action @ StreamingGateAction::BeginGatt(_))
            | Ok(action @ StreamingGateAction::RetryLater) => {
                self.streaming_gate = Some(gate);
                self.translate_streaming_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Ok(action @ StreamingGateAction::Complete(_)) => {
                self.streaming_gate = None;
                self.translate_streaming_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Err(failure) => {
                self.streaming_gate = None;
                let mut actions =
                    self.disarm_gatt_request(side, completed_request)?;
                if let Some(request_id) = pending_before
                    .filter(|_| completed_request.is_none())
                {
                    actions.push(ControlAction::CancelGatt {
                        connection,
                        request_id,
                    });
                    actions.extend(self.disarm_gatt_request(
                        side,
                        Some(request_id),
                    )?);
                } else if let Some(request_id) = pending_after {
                    actions.push(ControlAction::CancelGatt {
                        connection,
                        request_id,
                    });
                    actions.extend(self.disarm_gatt_request(
                        side,
                        Some(request_id),
                    )?);
                }
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::StreamingProbe { side },
                ));
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::StreamingDeadline { side },
                ));
                actions.extend(self.begin_stream_rollback(
                    ControlFailure::StreamingGate {
                        side,
                        failure,
                    },
                )?);
                Ok(actions)
            }
        }
    }

    fn translate_streaming_action(
        &mut self,
        side: AudioSide,
        completed_request: Option<GattRequestId>,
        action: StreamingGateAction,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut actions =
            self.disarm_gatt_request(side, completed_request)?;

        match action {
            StreamingGateAction::BeginGatt(request) => {
                let request_id = request.request_id;
                let member_index = self.planned_sink(side)?.member_index;
                // A STREAMING notification may shortcut a previously armed
                // retry probe. Disarming a stale probe is intentional.
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::StreamingProbe { side },
                ));
                actions.push(ControlAction::BeginGatt(request));
                actions.push(ControlAction::ArmTimer(
                    ControlTimer::Gatt {
                        member_index,
                        request_id,
                    },
                ));
                Ok(actions)
            }
            StreamingGateAction::RetryLater => {
                actions.push(ControlAction::ArmTimer(
                    ControlTimer::StreamingProbe { side },
                ));
                Ok(actions)
            }
            StreamingGateAction::Complete(status) => {
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::StreamingProbe { side },
                ));
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::StreamingDeadline { side },
                ));
                self.streaming_statuses[side_index(side)] =
                    Some(status);
                match side {
                    AudioSide::Left => {
                        actions.extend(
                            self.start_streaming_gate(
                                AudioSide::Right,
                            )?,
                        );
                    }
                    AudioSide::Right => {
                        self.phase = ControlPhase::StreamingReady;
                    }
                }
                Ok(actions)
            }
            StreamingGateAction::Ignored => Ok(actions),
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
    ) -> Result<&crate::plane::l1_control::planning::codec::PlannedSink, ControlFailure> {
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

    fn cis_for_side(
        &self,
        side: AudioSide,
    ) -> Option<CisRef> {
        let plan = self.iso_plan.as_ref()?;
        Some(CisRef {
            cig_id: plan.cig_id,
            cis_id: plan.cis_id(side),
        })
    }

    fn start_iso_path_setup(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let cis = self
            .cis_for_side(side)
            .ok_or(ControlFailure::InvalidStartState(self.phase))?;
        if !self.live_cis.contains(&cis) {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.phase = ControlPhase::SettingUpIsoPath { side };
        Ok(vec![
            ControlAction::SetupIsoInputPathByRef { cis },
            ControlAction::ArmTimer(
                ControlTimer::IsoPathSetup { cis },
            ),
        ])
    }

    fn semantic_cis_ref(
        &self,
        cig_id: u8,
        cis_id: u8,
    ) -> Result<CisRef, ControlFailure> {
        let plan = self
            .iso_plan
            .as_ref()
            .ok_or(ControlFailure::InvalidStartState(self.phase))?;
        if cig_id != plan.cig_id
            || ![plan.left.cis_id, plan.right.cis_id]
                .contains(&cis_id)
        {
            let mut expected =
                vec![plan.left.cis_id, plan.right.cis_id];
            expected.sort_unstable();
            return Err(ControlFailure::CisCreateMismatch {
                expected_cig_id: plan.cig_id,
                actual_cig_id: cig_id,
                expected_cis_ids: expected,
                actual_cis_ids: vec![cis_id],
            });
        }
        Ok(CisRef { cig_id, cis_id })
    }

    fn observe_semantic_cis_set(
        &mut self,
        cig_id: u8,
        cis_ids: &[u8],
    ) -> Result<(), ControlFailure> {
        let mut next = BTreeSet::new();
        for cis_id in cis_ids {
            next.insert(self.semantic_cis_ref(cig_id, *cis_id)?);
        }
        self.live_cis = next;
        Ok(())
    }

    fn semantic_cis_set_complete(
        &self,
        cig_id: u8,
        cis_ids: &[u8],
    ) -> bool {
        let Some(plan) = self.iso_plan.as_ref() else {
            return false;
        };
        if cig_id != plan.cig_id || cis_ids.len() != 2 {
            return false;
        }
        let mut actual = cis_ids.to_vec();
        actual.sort_unstable();
        let mut expected =
            vec![plan.left.cis_id, plan.right.cis_id];
        expected.sort_unstable();
        actual == expected
    }

    fn start_rollback_ase_release(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.prepared_ases[side_index(side)]
            .as_ref()
            .expect("rollback ASE release requires prepared control")
            .clone();
        let ase_id = prepared.sink.ase_id;
        let mut machine = AseOperationMachine::new_with_states(
            prepared,
            AseOperation::Release(vec![ase_id]),
            AseState::RELEASING,
            AseState::IDLE,
            self.next_gatt_request_id,
        );
        let action = match machine.start() {
            Ok(action) => action,
            Err(failure) => {
                self.cleanup_failures.push(
                    ControlCleanupFailure::AseRelease {
                        side,
                        failure,
                    },
                );
                return self.continue_rollback();
            }
        };
        self.next_gatt_request_id = machine.next_request_id();
        self.rollback_ase_current = Some(side);
        self.rollback_ase_operation = Some(machine);

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::CleanupAse { side },
        )];
        actions.extend(
            self.translate_rollback_ase_action(side, None, action)?,
        );
        Ok(actions)
    }

    fn on_rollback_ase_gatt_report(
        &mut self,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let side = self
            .rollback_ase_current
            .expect("rollback ASE report requires current side");
        let mut machine = self
            .rollback_ase_operation
            .take()
            .expect("rollback ASE report requires operation");
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
                self.rollback_ase_operation = Some(machine);
                Ok(Vec::new())
            }
            Ok(AseOperationAction::Await) => {
                self.rollback_ase_operation = Some(machine);
                self.disarm_gatt_request(side, completed_request)
            }
            Ok(action @ AseOperationAction::BeginGatt(_)) => {
                self.rollback_ase_operation = Some(machine);
                self.translate_rollback_ase_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Ok(action @ AseOperationAction::Complete(_)) => {
                self.rollback_ase_operation = None;
                self.translate_rollback_ase_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Err(failure) => {
                self.rollback_ase_operation = None;
                self.rollback_ase_current = None;
                self.cleanup_failures.push(
                    ControlCleanupFailure::AseRelease {
                        side,
                        failure,
                    },
                );
                let mut actions =
                    self.disarm_gatt_request(side, completed_request)?;
                if let Some(request_id) = pending_after {
                    actions.push(ControlAction::CancelGatt {
                        connection: self.planned_sink(side)?.connection,
                        request_id,
                    });
                    actions.extend(self.disarm_gatt_request(
                        side,
                        Some(request_id),
                    )?);
                }
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::CleanupAse { side },
                ));
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
        }
    }

    fn translate_rollback_ase_action(
        &mut self,
        side: AudioSide,
        completed_request: Option<GattRequestId>,
        action: AseOperationAction,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut actions =
            self.disarm_gatt_request(side, completed_request)?;

        match action {
            AseOperationAction::BeginGatt(request) => {
                let request_id = request.request_id;
                let member_index = self.planned_sink(side)?.member_index;
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
                if status.state != AseState::IDLE {
                    self.cleanup_failures.push(
                        ControlCleanupFailure::AseRelease {
                            side,
                            failure:
                                AseOperationFailure::ReadbackStateMismatch {
                                    ase_id: status.ase_id,
                                    expected_state: AseState::IDLE.0,
                                    actual_state: status.state.0,
                                },
                        },
                    );
                }
                self.rollback_ase_current = None;
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::CleanupAse { side },
                ));
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
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

    fn prepare_stream_route_rollback(&mut self) {
        let binding = self
            .active_stream_binding
            .or(self.pending_stream_binding);
        self.rollback_stream_route =
            binding.map(|value| (value.session_id, value.stream_id));
        self.rollback_stream_route_unbind_pending = false;
        self.pending_stream_binding = None;
        self.active_stream_binding = None;
    }

    fn begin_rollback(
        &mut self,
        failure: ControlFailure,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.failure = Some(failure);
        self.prepare_stream_route_rollback();
        self.rollback_queue = self
            .connections
            .iter()
            .flatten()
            .copied()
            .collect();
        self.rollback_current = None;
        self.rollback_iso_path_queue.clear();
        self.rollback_iso_path_current = None;
        self.rollback_cis_queue.clear();
        self.rollback_cis_current = None;
        self.rollback_ase_queue.clear();
        self.rollback_ase_current = None;
        self.rollback_ase_operation = None;
        self.cleanup_failures.clear();
        self.phase = ControlPhase::RollingBack;
        self.continue_rollback()
    }

    fn begin_stream_rollback(
        &mut self,
        failure: ControlFailure,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.failure = Some(failure);
        self.prepare_stream_route_rollback();
        self.rollback_queue = self
            .connections
            .iter()
            .flatten()
            .copied()
            .collect();
        self.rollback_current = None;

        self.rollback_iso_path_queue =
            self.iso_paths_may_exist.iter().copied().collect();
        self.rollback_iso_path_current = None;

        self.rollback_cis_queue =
            self.live_cis.iter().copied().collect();
        self.rollback_cis_current = None;

        self.rollback_ase_queue.clear();
        for side in [AudioSide::Left, AudioSide::Right] {
            if self.prepared_ases[side_index(side)].is_some() {
                self.rollback_ase_queue.push(side);
            }
        }
        self.rollback_ase_current = None;
        self.rollback_ase_operation = None;

        self.cleanup_failures.clear();
        self.phase = ControlPhase::RollingBack;
        self.continue_rollback()
    }

    fn continue_rollback(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.phase = ControlPhase::RollingBack;

        if let Some((session_id, stream_id)) =
            self.rollback_stream_route
        {
            if !self.rollback_stream_route_unbind_pending {
                self.rollback_stream_route_unbind_pending = true;
                return Ok(vec![
                    ControlAction::UnbindStreamRoute {
                        session_id,
                        stream_id,
                    },
                    ControlAction::ArmTimer(
                        ControlTimer::CleanupStreamRoute {
                            session_id,
                            stream_id,
                        },
                    ),
                ]);
            }
            return Ok(Vec::new());
        }

        if self.rollback_iso_path_current.is_none() {
            if let Some(cis) = self.rollback_iso_path_queue.pop() {
                self.rollback_iso_path_current = Some(cis);
                return Ok(vec![
                    ControlAction::RemoveIsoPathByRef {
                        cis,
                        directions: 0x01,
                    },
                    ControlAction::ArmTimer(
                        ControlTimer::CleanupIsoPath { cis },
                    ),
                ]);
            }
        }
        if self.rollback_iso_path_current.is_some() {
            return Ok(Vec::new());
        }

        if self.rollback_cis_current.is_none() {
            if let Some(cis) = self.rollback_cis_queue.pop() {
                self.rollback_cis_current = Some(cis);
                return Ok(vec![
                    ControlAction::DisconnectCisByRef { cis },
                    ControlAction::ArmTimer(
                        ControlTimer::CleanupCis { cis },
                    ),
                ]);
            }
        }
        if self.rollback_cis_current.is_some() {
            return Ok(Vec::new());
        }

        if self.rollback_ase_current.is_none() {
            if let Some(side) = self.rollback_ase_queue.pop() {
                return self.start_rollback_ase_release(side);
            }
        }
        if self.rollback_ase_current.is_some() {
            return Ok(Vec::new());
        }

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
        plane::l1_control::model::set_plan::ControlMemberPlan,
        plane::l1_hci::{
            host::engine::{
                ControllerCapabilities, ControllerIdentity, PacketPoolSnapshot,
            },
            acl::{
            scan::{ScanObservation, ScanSummary},
            security::StoredLeKeyMaterial,
            },
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
    ) -> crate::plane::l1_hci::acl::connection::LeConnectionSnapshot {
        crate::plane::l1_hci::acl::connection::LeConnectionSnapshot {
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
            kind: crate::plane::l1_hci::acl::security::
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
    ) -> crate::plane::l1_control::planning::codec::PlannedSink {
        use bumble_hci::CodingFormat;
        use bumble_profiles::bap::{
            CodecSpecificConfiguration, FrameDuration,
            SamplingFrequency,
        };
        use crate::plane::l1_control::discovery::snapshot::GattCharacteristicRange;

        crate::plane::l1_control::planning::codec::PlannedSink {
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
        result: crate::plane::l1_hci::gatt::transaction::GattResult,
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
        use crate::plane::l1_hci::gatt::transaction::{
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
        use crate::plane::l1_hci::gatt::transaction::{
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
        use crate::plane::l1_hci::gatt::transaction::{
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
                        crate::plane::l1_hci::iso::cig_cis::CisAllocation {
                            cis_id: 1,
                            connection_handle: 0x0900,
                        },
                        crate::plane::l1_hci::iso::cig_cis::CisAllocation {
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
        use crate::plane::l1_hci::gatt::transaction::{
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
        use crate::plane::l1_hci::gatt::transaction::{
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

    fn drive_to_enabled() -> ControlMachine {
        let mut machine = drive_to_enable_planned();
        let actions = machine.begin_enable().unwrap();
        let actions =
            drive_enable(&mut machine, AudioSide::Left, actions);
        let _actions =
            drive_enable(&mut machine, AudioSide::Right, actions);
        assert_eq!(machine.phase(), ControlPhase::Enabled);
        machine
    }

    fn release_cp_success_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };
        AseControlResponse {
            opcode: AseOpcode::Release,
            responses: vec![AseResponse {
                ase_id: machine.planned_sink(side).unwrap().ase_id,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    fn drive_rollback_release(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::plane::l1_hci::gatt::transaction::{
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
                        value: release_cp_success_bytes(machine, side),
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
                        value: vec![
                            machine.planned_sink(side).unwrap().ase_id,
                            AseState::RELEASING.0,
                        ],
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
                value: vec![
                    machine.planned_sink(side).unwrap().ase_id,
                    AseState::IDLE.0,
                ],
            },
        )
    }

    #[test]
    fn create_cis_success_uses_only_semantic_cis_refs() {
        let mut machine = drive_to_enabled();
        let plan = machine.iso_plan().unwrap().clone();

        let actions = machine.begin_cis_creation().unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::CreateCis(
                    plan.cis_create_request(),
                ),
                ControlAction::ArmTimer(ControlTimer::CisCreate),
            ]
        );
        assert_eq!(machine.phase(), ControlPhase::CreatingCis);

        assert!(machine
            .on_hci_report(&HciReport::SemanticCisCreateState(
                SemanticCisCreateState::AwaitingEstablished {
                    cig_id: 1,
                    established_cis_ids: vec![2],
                },
            ))
            .unwrap()
            .is_empty());
        assert_eq!(
            machine.live_cis(),
            &BTreeSet::from([CisRef {
                cig_id: 1,
                cis_id: 2,
            }])
        );

        let actions = machine
            .on_hci_report(&HciReport::SemanticCisCreateState(
                SemanticCisCreateState::Ready {
                    cig_id: 1,
                    established_cis_ids: vec![2, 1],
                },
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::CisCreate,
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::CisEstablished);
        assert_eq!(
            machine.live_cis(),
            &BTreeSet::from([
                CisRef {
                    cig_id: 1,
                    cis_id: 1,
                },
                CisRef {
                    cig_id: 1,
                    cis_id: 2,
                },
            ])
        );
    }

    fn drive_to_cis_established() -> ControlMachine {
        let mut machine = drive_to_enabled();
        machine.begin_cis_creation().unwrap();
        machine
            .on_hci_report(&HciReport::SemanticCisCreateState(
                SemanticCisCreateState::Ready {
                    cig_id: 1,
                    established_cis_ids: vec![1, 2],
                },
            ))
            .unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::CisEstablished
        );
        machine
    }

    #[test]
    fn iso_path_setup_runs_left_then_right_to_ready() {
        let mut machine = drive_to_cis_established();
        let left = CisRef {
            cig_id: 1,
            cis_id: 1,
        };
        let right = CisRef {
            cig_id: 1,
            cis_id: 2,
        };

        let actions = machine.begin_iso_path_setup().unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::SetupIsoInputPathByRef {
                    cis: left,
                },
                ControlAction::ArmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: left,
                    },
                ),
            ]
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::SettingUpIsoPath {
                side: AudioSide::Left,
            }
        );

        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: left,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: left,
                    },
                ),
                ControlAction::SetupIsoInputPathByRef {
                    cis: right,
                },
                ControlAction::ArmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: right,
                    },
                ),
            ]
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::SettingUpIsoPath {
                side: AudioSide::Right,
            }
        );

        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: right,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::IsoPathSetup {
                    cis: right,
                },
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::IsoPathsReady);
        assert_eq!(
            machine.iso_paths_ready(),
            &BTreeSet::from([left, right])
        );
    }

    #[test]
    fn uncertain_right_path_failure_removes_paths_before_any_cis() {
        use crate::plane::l1_hci::iso::path::{
            SemanticIsoPathFailureKind,
        };

        let mut machine = drive_to_cis_established();
        let left = CisRef {
            cig_id: 1,
            cis_id: 1,
        };
        let right = CisRef {
            cig_id: 1,
            cis_id: 2,
        };

        machine.begin_iso_path_setup().unwrap();
        machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: left,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();

        let failure = SemanticIsoPathFailure {
            cis: right,
            may_have_configured: true,
            kind: SemanticIsoPathFailureKind::
                ConnectionHandleMismatch,
        };
        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::Failed(
                    failure.clone(),
                ),
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: right,
                    },
                ),
                ControlAction::RemoveIsoPathByRef {
                    cis: right,
                    directions: 0x01,
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupIsoPath {
                        cis: right,
                    },
                ),
            ]
        );
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::IsoPathSetup {
                cis,
                failure: actual,
            }) if *cis == right && actual == &failure
        ));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::DisconnectCisByRef { .. }
        )));

        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::RemoveComplete {
                    cis: right,
                    directions: 0x01,
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::CleanupIsoPath {
                        cis: right,
                    },
                ),
                ControlAction::RemoveIsoPathByRef {
                    cis: left,
                    directions: 0x01,
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupIsoPath {
                        cis: left,
                    },
                ),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::RemoveComplete {
                    cis: left,
                    directions: 0x01,
                },
            ))
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupIsoPath {
                    cis: left,
                },
            )
        );
        assert_eq!(
            actions[1],
            ControlAction::DisconnectCisByRef {
                cis: right,
            }
        );
        assert_eq!(
            actions[2],
            ControlAction::ArmTimer(
                ControlTimer::CleanupCis {
                    cis: right,
                },
            )
        );
        assert!(machine.iso_paths_ready().is_empty());
    }

    fn drive_to_iso_paths_ready() -> ControlMachine {
        let mut machine = drive_to_cis_established();
        let left = CisRef {
            cig_id: 1,
            cis_id: 1,
        };
        let right = CisRef {
            cig_id: 1,
            cis_id: 2,
        };

        machine.begin_iso_path_setup().unwrap();
        machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: left,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();
        machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: right,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();
        assert_eq!(machine.phase(), ControlPhase::IsoPathsReady);
        machine
    }

    fn streaming_status_bytes(
        machine: &ControlMachine,
        side: AudioSide,
        state: AseState,
    ) -> Vec<u8> {
        use bumble_profiles::{
            ascs::{AseEndpoint, AudioRole},
            le_audio::Metadata,
        };

        let enable = match side {
            AudioSide::Left => &machine.enable_plan().unwrap().left,
            AudioSide::Right => &machine.enable_plan().unwrap().right,
        };
        let qos = match side {
            AudioSide::Left => &machine.qos_plan().unwrap().left,
            AudioSide::Right => &machine.qos_plan().unwrap().right,
        };

        let mut endpoint =
            AseEndpoint::new(enable.ase_id, AudioRole::Sink);
        endpoint.state = state;
        endpoint.qos = qos.expected_readback();
        endpoint.metadata =
            Metadata::from_bytes(&enable.metadata).unwrap();
        endpoint.value().unwrap()
    }

    fn only_read_gatt(actions: &[ControlAction]) -> GattRequest {
        let request = only_begin_gatt(actions);
        assert!(matches!(
            request.operation,
            crate::plane::l1_hci::gatt::transaction::GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        ));
        request
    }

    fn drive_to_streaming_ready() -> ControlMachine {
        use crate::plane::l1_hci::gatt::transaction::GattResult;

        let mut machine = drive_to_iso_paths_ready();
        let actions = machine.begin_streaming_gate().unwrap();
        let left_read = only_read_gatt(&actions);
        let left_streaming = streaming_status_bytes(
            &machine,
            AudioSide::Left,
            AseState::STREAMING,
        );
        let actions = complete_gatt(
            &mut machine,
            &left_read,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: left_streaming,
            },
        );

        let right_read = only_read_gatt(&actions);
        let right_streaming = streaming_status_bytes(
            &machine,
            AudioSide::Right,
            AseState::STREAMING,
        );
        complete_gatt(
            &mut machine,
            &right_read,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: right_streaming,
            },
        );

        assert_eq!(machine.phase(), ControlPhase::StreamingReady);
        machine
    }

    #[test]
    fn stream_binding_requires_successful_hci_route_ack() {
        let machine = drive_to_iso_paths_ready();
        assert_eq!(
            machine.stream_binding(),
            Err(ControlFailure::InvalidStartState(
                ControlPhase::IsoPathsReady,
            ))
        );

        let machine = drive_to_streaming_ready();
        assert_eq!(
            machine.stream_binding(),
            Err(ControlFailure::InvalidStartState(
                ControlPhase::StreamingReady,
            ))
        );
    }

    #[test]
    fn route_ack_unlocks_transport_neutral_stream_binding() {
        let mut machine = drive_to_streaming_ready();

        let actions = machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::BindingStreamRoute {
                session_id: SessionId(7),
                stream_id: StreamId(11),
            }
        );
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::BindStreamRoute(
                    StereoStreamRouteBinding {
                        session_id: SessionId(7),
                        stream_id: StreamId(11),
                        left: CisRef {
                            cig_id: 1,
                            cis_id: 1,
                        },
                        right: CisRef {
                            cig_id: 1,
                            cis_id: 2,
                        },
                    }
                ),
                ControlAction::ArmTimer(
                    ControlTimer::StreamRouteBind {
                        session_id: SessionId(7),
                        stream_id: StreamId(11),
                    }
                )
            ]
        ));
        assert!(matches!(
            machine.stream_binding(),
            Err(ControlFailure::InvalidStartState(
                ControlPhase::BindingStreamRoute {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                }
            ))
        ));

        let actions = machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Bound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::StreamRouteBind {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::StreamBoundReady);

        let binding = machine.stream_binding().unwrap();
        assert_eq!(binding.session_id, SessionId(7));
        assert_eq!(binding.stream_id, StreamId(11));
        assert_eq!(
            binding.format,
            crate::architecture::control::LeAudioFormat {
                sample_rate_hz: 48_000,
                frame_duration_us: 10_000,
                octets_per_sdu: 100,
            }
        );
        assert_eq!(binding.presentation_delay_us, 40_000);
        assert_eq!(
            binding.topology,
            StreamTopology::StereoSplit
        );
    }

    #[test]
    fn route_bind_timeout_rolls_back_before_media_handoff() {
        let mut machine = drive_to_streaming_ready();
        machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::StreamRouteBind {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();

        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::StreamRouteTimeout {
                session_id: SessionId(7),
                stream_id: StreamId(11),
            })
        ));
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::StreamRouteBind {
                        session_id: SessionId(7),
                        stream_id: StreamId(11),
                    },
                ),
                ControlAction::UnbindStreamRoute {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupStreamRoute {
                        session_id: SessionId(7),
                        stream_id: StreamId(11),
                    },
                ),
            ]
        );
        assert!(machine.stream_binding().is_err());

        let actions = machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Unbound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                    was_bound: true,
                },
            ))
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupStreamRoute {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            )
        );
        assert_eq!(
            actions[1],
            ControlAction::RemoveIsoPathByRef {
                cis: CisRef {
                    cig_id: 1,
                    cis_id: 2,
                },
                directions: 0x01,
            }
        );
        assert_eq!(
            actions[2],
            ControlAction::ArmTimer(
                ControlTimer::CleanupIsoPath {
                    cis: CisRef {
                        cig_id: 1,
                        cis_id: 2,
                    },
                },
            )
        );
        assert!(!actions[..1].iter().any(|action| matches!(
            action,
            ControlAction::DisconnectCisByRef { .. }
                | ControlAction::RemoveCig { .. }
                | ControlAction::DisconnectAcl { .. }
        )));
    }

    #[test]
    fn route_cleanup_timeout_records_failure_then_starts_path_cleanup() {
        let mut machine = drive_to_streaming_ready();
        machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();
        machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::StreamRouteBind {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();

        let cleanup_timer = ControlTimer::CleanupStreamRoute {
            session_id: SessionId(7),
            stream_id: StreamId(11),
        };
        let actions = machine
            .on_event(ControlEvent::TimerExpired(cleanup_timer))
            .unwrap();

        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(machine.cleanup_failures.contains(
            &ControlCleanupFailure::Timeout(cleanup_timer)
        ));
        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(cleanup_timer)
        );
        assert_eq!(
            actions[1],
            ControlAction::RemoveIsoPathByRef {
                cis: CisRef {
                    cig_id: 1,
                    cis_id: 2,
                },
                directions: 0x01,
            }
        );
        assert_eq!(
            actions[2],
            ControlAction::ArmTimer(
                ControlTimer::CleanupIsoPath {
                    cis: CisRef {
                        cig_id: 1,
                        cis_id: 2,
                    },
                },
            )
        );
    }

    #[test]
    fn streaming_gate_retries_left_then_confirms_both_ears() {
        use crate::plane::l1_hci::gatt::transaction::GattResult;

        let mut machine = drive_to_iso_paths_ready();

        let actions = machine.begin_streaming_gate().unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::AwaitingStreaming {
                side: AudioSide::Left,
            }
        );
        assert!(actions.contains(&ControlAction::ArmTimer(
            ControlTimer::StreamingDeadline {
                side: AudioSide::Left,
            },
        )));
        let left_read = only_read_gatt(&actions);

        let left_enabling = streaming_status_bytes(
            &machine,
            AudioSide::Left,
            AseState::ENABLING,
        );
        let actions = complete_gatt(
            &mut machine,
            &left_read,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: left_enabling,
            },
        );
        assert!(actions.contains(&ControlAction::ArmTimer(
            ControlTimer::StreamingProbe {
                side: AudioSide::Left,
            },
        )));

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::StreamingProbe {
                    side: AudioSide::Left,
                },
            ))
            .unwrap();
        let left_confirm = only_read_gatt(&actions);

        let left_streaming = streaming_status_bytes(
            &machine,
            AudioSide::Left,
            AseState::STREAMING,
        );
        let actions = complete_gatt(
            &mut machine,
            &left_confirm,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: left_streaming,
            },
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::AwaitingStreaming {
                side: AudioSide::Right,
            }
        );
        assert!(machine.streaming_status(AudioSide::Left).is_some());
        assert!(actions.contains(&ControlAction::DisarmTimer(
            ControlTimer::StreamingDeadline {
                side: AudioSide::Left,
            },
        )));
        assert!(actions.contains(&ControlAction::ArmTimer(
            ControlTimer::StreamingDeadline {
                side: AudioSide::Right,
            },
        )));

        let right_read = only_read_gatt(&actions);
        let right_streaming = streaming_status_bytes(
            &machine,
            AudioSide::Right,
            AseState::STREAMING,
        );
        let actions = complete_gatt(
            &mut machine,
            &right_read,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: right_streaming,
            },
        );

        assert_eq!(machine.phase(), ControlPhase::StreamingReady);
        assert!(machine.streaming_status(AudioSide::Left).is_some());
        assert!(machine.streaming_status(AudioSide::Right).is_some());
        assert!(actions.contains(&ControlAction::DisarmTimer(
            ControlTimer::StreamingDeadline {
                side: AudioSide::Right,
            },
        )));
    }

    #[test]
    fn streaming_deadline_cancels_read_then_rolls_back_paths_first() {
        let mut machine = drive_to_iso_paths_ready();
        let actions = machine.begin_streaming_gate().unwrap();
        let read = only_read_gatt(&actions);

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::StreamingDeadline {
                    side: AudioSide::Left,
                },
            ))
            .unwrap();

        let cancel_index = actions
            .iter()
            .position(|action| {
                *action == ControlAction::CancelGatt {
                    connection: read.connection,
                    request_id: read.request_id,
                }
            })
            .expect("deadline must cancel the active GATT read");
        let remove_index = actions
            .iter()
            .position(|action| matches!(
                action,
                ControlAction::RemoveIsoPathByRef {
                    cis: CisRef {
                        cig_id: 1,
                        cis_id: 2,
                    },
                    directions: 0x01,
                }
            ))
            .expect("stream rollback must begin with right ISO path");

        assert!(cancel_index < remove_index);
        assert!(!actions[..remove_index].iter().any(|action| matches!(
            action,
            ControlAction::DisconnectCisByRef { .. }
                | ControlAction::RemoveCig { .. }
                | ControlAction::DisconnectAcl { .. }
        )));
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::StreamingDeadline {
                side: AudioSide::Left,
            })
        ));
    }

    #[test]
    fn partial_cis_failure_disconnects_cis_releases_ases_then_removes_cig() {
        use crate::plane::l1_hci::iso::cig_cis::{
            SemanticCisCreateFailureKind,
            SemanticCisEstablishedFailure,
        };

        let mut machine = drive_to_enabled();
        machine.begin_cis_creation().unwrap();

        let failure = SemanticCisCreateFailure {
            cig_id: 1,
            established_cis_ids: vec![2],
            failed: vec![SemanticCisEstablishedFailure {
                cis_id: 1,
                status: 0x3E,
            }],
            kind: SemanticCisCreateFailureKind::
                CisEstablishmentFailed,
        };
        let actions = machine
            .on_hci_report(&HciReport::SemanticCisCreateState(
                SemanticCisCreateState::Failed(failure.clone()),
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::CisCreate),
                ControlAction::DisconnectCisByRef {
                    cis: CisRef {
                        cig_id: 1,
                        cis_id: 2,
                    },
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupCis {
                        cis: CisRef {
                            cig_id: 1,
                            cis_id: 2,
                        },
                    },
                ),
            ]
        );
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::CisCreate(actual))
                if actual == &failure
        ));

        let cis = CisRef {
            cig_id: 1,
            cis_id: 2,
        };
        let actions = machine
            .on_hci_report(
                &HciReport::SemanticCisDisconnectState(
                    SemanticCisDisconnectState::Disconnected {
                        cis,
                        event_reason: 0x16,
                    },
                ),
            )
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupCis { cis },
            )
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::ArmTimer(
                ControlTimer::CleanupAse {
                    side: AudioSide::Right,
                }
            )
        )));
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::BeginGatt(_)
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveCig { .. }
        )));

        let actions = drive_rollback_release(
            &mut machine,
            AudioSide::Right,
            actions,
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::ArmTimer(
                ControlTimer::CleanupAse {
                    side: AudioSide::Left,
                }
            )
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveCig { .. }
        )));

        let actions = drive_rollback_release(
            &mut machine,
            AudioSide::Left,
            actions,
        );
        assert!(actions.iter().any(|action| {
            *action == ControlAction::RemoveCig { cig_id: 1 }
        }));
        let remove_index = actions
            .iter()
            .position(|action| {
                *action == ControlAction::RemoveCig { cig_id: 1 }
            })
            .unwrap();
        assert!(!actions[..remove_index].iter().any(|action| matches!(
            action,
            ControlAction::DisconnectAcl { .. }
        )));
        assert!(machine.live_cis().is_empty());
    }

    #[test]
    fn create_cis_timeout_fails_closed_without_guessing_controller_cleanup() {
        let mut machine = drive_to_enabled();
        machine.begin_cis_creation().unwrap();

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::CisCreate,
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::CisCreate,
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::Faulted);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::CisCreateTimeout {
                cig_id: 1,
            })
        ));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveCig { .. }
                | ControlAction::DisconnectAcl { .. }
        )));
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
                crate::plane::l1_hci::iso::cig_cis::CisAllocation {
                    cis_id: 1,
                    connection_handle: 0x0900,
                },
                crate::plane::l1_hci::iso::cig_cis::CisAllocation {
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
            kind: crate::plane::l1_hci::iso::cig_cis::CigConfigureFailureKind::
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
        use crate::plane::l1_control::discovery::snapshot::{
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
                        crate::plane::l1_control::discovery::snapshot::GattCharacteristicRange {
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
                    operation: crate::plane::l1_hci::gatt::transaction::GattOperation::
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

        let failure = crate::plane::l1_hci::gatt::transaction::GattFailure {
            request_id: request.request_id,
            connection: request.connection,
            kind: crate::plane::l1_hci::gatt::transaction::GattFailureKind::Att {
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
