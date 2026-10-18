use std::collections::BTreeSet;

mod audio_flow;
mod connection_flow;
mod event_reducer;
mod report_reducer;
mod rollback_flow;
mod stream_flow;

use bumble_profiles::ascs::{
    AseOperation, AseState, AseStatus, AseStatusDetails,
};

use crate::{
    core::lifecycle::{SessionId, StreamId},
    plane::l1_control::interface::{
        condition::{ControlConditionUpdate, ControlState},
        stream::{AudioSide, StreamBinding, StreamTopology},
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
        snapshot::AudioSetSnapshot,
    },
    model::{
        cleanup_knowledge::CleanupKnowledge,
        confirmed_status::ConfirmedStatus,
        planned_configuration::PlannedConfiguration,
        set_plan::{ControlPlanError, ControlSetPlan},
    },
    operation::{
        ase_control::AseControlOperation,
        audio_discovery::AudioDiscoveryOperation,
        rollback::RollbackOperation,
        stream_route::StreamRouteOperation,
        streaming::StreamingOperation,
    },
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
    confirmed: ConfirmedStatus,
    planned: PlannedConfiguration,
    cleanup_knowledge: CleanupKnowledge,
    rollback: RollbackOperation,
    phase: ControlPhase,
    next_gatt_request_id: GattRequestId,
    discovery_operation: AudioDiscoveryOperation,
    ase_control_operation: AseControlOperation,
    streaming_operation: StreamingOperation,
    stream_route_operation: StreamRouteOperation,
    failure: Option<ControlFailure>,
}

impl ControlMachine {
    pub fn new(plan: ControlSetPlan) -> Result<Self, ControlPlanError> {
        plan.validate()?;
        Ok(Self {
            plan,
            confirmed: ConfirmedStatus::new(),
            planned: PlannedConfiguration::new(),
            cleanup_knowledge: CleanupKnowledge::new(),
            rollback: RollbackOperation::new(),
            phase: ControlPhase::Idle,
            next_gatt_request_id: 1,
            discovery_operation: AudioDiscoveryOperation::new(),
            ase_control_operation: AseControlOperation::new(),
            streaming_operation: StreamingOperation::new(),
            stream_route_operation: StreamRouteOperation::new(),
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
    pub fn condition_update(&self) -> ControlConditionUpdate {
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

        let binding = self.confirmed.active_stream_binding.or(self.stream_route_operation.pending_binding);
        ControlConditionUpdate {
            state,
            stream_id: binding.map(|binding| binding.stream_id),
            format: binding.map(|binding| binding.format),
        }
    }

    pub fn resolved_members(&self) -> &[ResolvedMemberPresence] {
        &self.confirmed.resolved_members
    }

    pub fn failure(&self) -> Option<&ControlFailure> {
        self.failure.as_ref()
    }

    pub fn connections(&self) -> [Option<ConnectionToken>; 2] {
        self.confirmed.connections
    }

    pub fn audio_set(&self) -> Option<&AudioSetSnapshot> {
        self.confirmed.audio_set.as_ref()
    }

    pub fn codec_plan(&self) -> Option<&StereoCodecPlan> {
        self.planned.codec_plan.as_ref()
    }

    pub fn prepared_ase(
        &self,
        side: AudioSide,
    ) -> Option<&PreparedAseControl> {
        self.confirmed.prepared_ases[side_index(side)].as_ref()
    }

    pub fn codec_status(
        &self,
        side: AudioSide,
    ) -> Option<&AseStatus> {
        self.confirmed.codec_statuses[side_index(side)].as_ref()
    }

    pub fn iso_plan(&self) -> Option<&StereoIsoPlan> {
        self.planned.iso_plan.as_ref()
    }

    pub fn qos_plan(&self) -> Option<&StereoQosPlan> {
        self.planned.qos_plan.as_ref()
    }

    pub fn qos_status(
        &self,
        side: AudioSide,
    ) -> Option<&AseStatus> {
        self.confirmed.qos_statuses[side_index(side)].as_ref()
    }

    pub fn enable_plan(&self) -> Option<&StereoEnablePlan> {
        self.planned.enable_plan.as_ref()
    }

    pub fn enable_status(
        &self,
        side: AudioSide,
    ) -> Option<&AseStatus> {
        self.confirmed.enable_statuses[side_index(side)].as_ref()
    }

    pub fn live_cis(&self) -> &BTreeSet<CisRef> {
        &self.confirmed.live_cis
    }

    pub fn iso_paths_ready(&self) -> &BTreeSet<CisRef> {
        &self.confirmed.iso_paths_ready
    }

    pub fn streaming_status(
        &self,
        side: AudioSide,
    ) -> Option<&AseStatus> {
        self.confirmed.streaming_statuses[side_index(side)].as_ref()
    }

    pub fn stream_binding(
        &self,
    ) -> Result<StreamBinding, ControlFailure> {
        if self.phase != ControlPhase::StreamBoundReady {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        Ok(self.confirmed.active_stream_binding
            .expect("StreamBoundReady requires active binding"))
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
mod tests;
