use std::{
    collections::BTreeSet,
    thread::{self, JoinHandle},
};

use crossbeam_channel::{select_biased, TryRecvError, TrySendError};

use crate::{
    hci_host::{
        iso::{CigConfigureState, CisCreateState},
        iso_host_feature::CisHostFeatureState,
        iso_lifecycle::{CigRemoveState, CisDisconnectState},
        iso_path::IsoPathOperationState,
        iso_tx::{
            iso_group_touches_handles, IsoDataGroupRequest,
            IsoDrainRequest, IsoDrainState, IsoSubmitOutcome,
        },
        mailbox::{HciCommand, HciReport, HciWorkerPorts},
        stream_route::{
            StreamRouteFailure, StreamRouteFailureKind, StreamRouteState,
        },
    },
    plane::{
        l1_hci::acl::{
            connection::{
                DisconnectState, FalConnectionState, LeConnectionSetSnapshot,
            },
            privacy::ControllerPrivacyState,
            security::StoredEncryptionState,
        },
        l2_transport::worker::TransportTx,
    },
};

#[cfg(test)]
use crate::plane::l2_transport::packet::ControllerPacket;

use super::engine::{
    EngineFault, HciEngine, OperationalAttachState, PassiveScanState,
};

const CONTROL_BUDGET_PER_TURN: usize = 4;
const CONTROLLER_BUDGET_PER_TURN: usize = 64;
const ISO_GROUP_BUDGET_PER_TURN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciWorkerFault {
    Engine(EngineFault),
    ReportFull,
    ReportDisconnected,
    ControlDisconnected,
    ControllerDisconnected,
    IsoDataDisconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciWorkerExit {
    Stopped,
    Faulted(HciWorkerFault),
}

pub struct HciWorkerJoin {
    join: JoinHandle<HciWorkerExit>,
}

impl HciWorkerJoin {
    pub fn join(self) -> HciWorkerExit {
        self.join.join().unwrap_or(HciWorkerExit::Faulted(
            HciWorkerFault::Engine(EngineFault::PollBudgetExceeded),
        ))
    }
}

pub(crate) fn spawn_hci_worker(
    ports: HciWorkerPorts,
    transport_tx: TransportTx,
) -> HciWorkerJoin {
    let join = thread::Builder::new()
        .name("HciHost".into())
        .spawn(move || run_loop(ports, transport_tx))
        .expect("failed to spawn HCI host worker");

    HciWorkerJoin { join }
}

fn run_loop(
    ports: HciWorkerPorts,
    transport_tx: TransportTx,
) -> HciWorkerExit {
    let HciWorkerPorts {
        control_rx,
        stop_rx,
        controller_rx,
        iso_rx,
        report_tx,
    } = ports;

    let mut engine = HciEngine::new(transport_tx);
    let mut last_attach_state = engine.attach_state();
    let mut last_scan_state = engine.passive_scan_state();
    let mut last_privacy_state = engine.controller_privacy_state().clone();
    let mut last_fal_connection_state =
        engine.fal_connection_state().clone();
    let mut last_disconnect_state = engine.disconnect_state().clone();
    let mut last_stored_encryption_state =
        engine.stored_encryption_state().clone();
    let mut last_cis_host_feature_state =
        engine.cis_host_feature_state();
    let mut last_cig_configure_state =
        engine.cig_configure_state().clone();
    let mut last_cis_create_state =
        engine.cis_create_state().clone();
    let mut last_cis_disconnect_state =
        engine.cis_disconnect_state().clone();
    let mut last_cig_remove_state =
        engine.cig_remove_state().clone();
    let mut last_iso_path_state =
        engine.iso_path_state().clone();
    let mut last_connection_set = engine.connection_set_snapshot();
    let mut pending_iso_group: Option<IsoDataGroupRequest> = None;
    let mut blocked_iso_handles = BTreeSet::new();
    let mut iso_drain_state = IsoDrainState::Idle;
    let mut last_iso_drain_state = IsoDrainState::Idle;

    loop {
        match stop_rx.try_recv() {
            Ok(()) => return HciWorkerExit::Stopped,
            Err(TryRecvError::Disconnected) => return HciWorkerExit::Stopped,
            Err(TryRecvError::Empty) => {}
        }

        let mut did_work = false;

        for _ in 0..CONTROL_BUDGET_PER_TURN {
            match control_rx.try_recv() {
                Ok(command) => {
                    did_work = true;
                    let audit_command =
                        matches!(&command, HciCommand::ReadIsoTxAudit);

                    let route_state =
                        match process_worker_command(
                            &mut engine,
                            command,
                            &mut blocked_iso_handles,
                            &mut pending_iso_group,
                            &mut iso_drain_state,
                        ) {
                            Ok(state) => state,
                            Err(fault) => {
                                return HciWorkerExit::Faulted(
                                    HciWorkerFault::Engine(fault),
                                )
                            }
                        };

                    if let Some(state) = route_state {
                        if let Err(fault) = report_tx
                            .try_send(HciReport::StreamRouteState(
                                state,
                            ))
                            .map_err(map_report_error)
                        {
                            return HciWorkerExit::Faulted(fault);
                        }
                    }

                    if audit_command {
                        if let Err(fault) = report_tx
                            .try_send(HciReport::IsoTxAudit(
                                engine.iso_credit_snapshot(),
                            ))
                            .map_err(map_report_error)
                        {
                            return HciWorkerExit::Faulted(fault);
                        }
                    }
                    if let Err(fault) = publish_state_transitions(
                        &mut engine,
                        &mut last_attach_state,
                        &mut last_scan_state,
                        &mut last_privacy_state,
                        &mut last_fal_connection_state,
                        &mut last_disconnect_state,
                        &mut last_stored_encryption_state,
                        &mut last_cis_host_feature_state,
                        &mut last_cig_configure_state,
                        &mut last_cis_create_state,
                        &mut last_cis_disconnect_state,
                        &mut last_cig_remove_state,
                        &mut last_iso_path_state,
                        &mut iso_drain_state,
                        &mut last_iso_drain_state,
                        &mut last_connection_set,
                        &report_tx,
                    ) {
                        return HciWorkerExit::Faulted(fault);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::ControlDisconnected,
                    )
                }
            }
        }

        let mut controller_packets = 0usize;
        for _ in 0..CONTROLLER_BUDGET_PER_TURN {
            match controller_rx.try_recv() {
                Ok(packet) => {
                    did_work = true;
                    controller_packets += 1;
                    if let Err(fault) = engine.ingest(packet) {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::Engine(fault),
                        );
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::ControllerDisconnected,
                    )
                }
            }
        }

        if controller_packets != 0 {
            if let Err(fault) = engine.poll_until_quiescent() {
                return HciWorkerExit::Faulted(
                    HciWorkerFault::Engine(fault),
                );
            }
            if let Err(fault) = publish_state_transitions(
                &mut engine,
                &mut last_attach_state,
                &mut last_scan_state,
                &mut last_privacy_state,
                &mut last_fal_connection_state,
                &mut last_disconnect_state,
                &mut last_stored_encryption_state,
                &mut last_cis_host_feature_state,
                &mut last_cig_configure_state,
                &mut last_cis_create_state,
                &mut last_cis_disconnect_state,
                &mut last_cig_remove_state,
                &mut last_iso_path_state,
                &mut iso_drain_state,
                &mut last_iso_drain_state,
                &mut last_connection_set,
                &report_tx,
            ) {
                return HciWorkerExit::Faulted(fault);
            }
        }

        let mut iso_accepted = false;
        for _ in 0..ISO_GROUP_BUDGET_PER_TURN {
            let request = match pending_iso_group.take() {
                Some(request) => request,
                None => match iso_rx.try_recv() {
                    Ok(request) => request,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::IsoDataDisconnected,
                        )
                    }
                },
            };

            if iso_data_group_should_drop(
                &engine,
                &request,
                &blocked_iso_handles,
            ) {
                // Realtime data superseded by route teardown or a reliable
                // drain gate is stale. Drop the complete stereo group,
                // never one ear.
                did_work = true;
                continue;
            }

            match submit_iso_data_group(&mut engine, &request) {
                Ok(Some(IsoSubmitOutcome::Accepted)) => {
                    did_work = true;
                    iso_accepted = true;
                }
                Ok(Some(IsoSubmitOutcome::Backpressured)) => {
                    pending_iso_group = Some(request);
                    break;
                }
                Ok(None) => {
                    // A logical route may disappear between queueing and
                    // submission. Treat it as stale realtime data.
                    did_work = true;
                }
                Err(fault) => {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::Engine(fault),
                    )
                }
            }
        }

        if iso_accepted {
            // Successful ISO packet enqueue is journaled by QueueHostTransport.
            // Consume those observations now so credit accounting remains exact
            // before this HCI turn yields.
            if let Err(fault) = engine.poll_until_quiescent() {
                return HciWorkerExit::Faulted(
                    HciWorkerFault::Engine(fault),
                );
            }
        }

        if did_work {
            continue;
        }

        // Idle wait only. A retained backpressured ISO group must not cause a
        // busy retry loop: only controller/control/stop progress can make it
        // admissible again. Disable the ISO receive arm until that happens.
        let never_iso = crossbeam_channel::never();
        let iso_wait = if pending_iso_group.is_none() {
            &iso_rx
        } else {
            &never_iso
        };

        // No HCI transaction waits here: every command/event handler remains
        // run-to-completion and returns to this selector.
        select_biased! {
            recv(stop_rx) -> _ => return HciWorkerExit::Stopped,

            recv(controller_rx) -> message => {
                let packet = match message {
                    Ok(packet) => packet,
                    Err(_) => {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::ControllerDisconnected,
                        )
                    }
                };
                if let Err(fault) = engine.ingest(packet) {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::Engine(fault),
                    );
                }
                if let Err(fault) = engine.poll_until_quiescent() {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::Engine(fault),
                    );
                }
                if let Err(fault) = publish_state_transitions(
                    &mut engine,
                    &mut last_attach_state,
                    &mut last_scan_state,
                    &mut last_privacy_state,
                    &mut last_fal_connection_state,
                    &mut last_disconnect_state,
                    &mut last_stored_encryption_state,
                    &mut last_cis_host_feature_state,
                    &mut last_cig_configure_state,
                    &mut last_cis_create_state,
                    &mut last_cis_disconnect_state,
                    &mut last_cig_remove_state,
                    &mut last_iso_path_state,
                    &mut iso_drain_state,
                    &mut last_iso_drain_state,
                    &mut last_connection_set,
                    &report_tx,
                ) {
                    return HciWorkerExit::Faulted(fault);
                }
            }

            recv(control_rx) -> message => {
                let command = match message {
                    Ok(command) => command,
                    Err(_) => {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::ControlDisconnected,
                        )
                    }
                };
                let audit_command =
                    matches!(&command, HciCommand::ReadIsoTxAudit);
                let route_state =
                    match process_worker_command(
                        &mut engine,
                        command,
                        &mut blocked_iso_handles,
                        &mut pending_iso_group,
                        &mut iso_drain_state,
                    ) {
                        Ok(state) => state,
                        Err(fault) => {
                            return HciWorkerExit::Faulted(
                                HciWorkerFault::Engine(fault),
                            )
                        }
                    };
                if let Some(state) = route_state {
                    if let Err(fault) = report_tx
                        .try_send(HciReport::StreamRouteState(state))
                        .map_err(map_report_error)
                    {
                        return HciWorkerExit::Faulted(fault);
                    }
                }
                if audit_command {
                    if let Err(fault) = report_tx
                        .try_send(HciReport::IsoTxAudit(
                            engine.iso_credit_snapshot(),
                        ))
                        .map_err(map_report_error)
                    {
                        return HciWorkerExit::Faulted(fault);
                    }
                }
                if let Err(fault) = publish_state_transitions(
                    &mut engine,
                    &mut last_attach_state,
                    &mut last_scan_state,
                    &mut last_privacy_state,
                    &mut last_fal_connection_state,
                    &mut last_disconnect_state,
                    &mut last_stored_encryption_state,
                    &mut last_cis_host_feature_state,
                    &mut last_cig_configure_state,
                    &mut last_cis_create_state,
                    &mut last_cis_disconnect_state,
                    &mut last_cig_remove_state,
                    &mut last_iso_path_state,
                    &mut iso_drain_state,
                    &mut last_iso_drain_state,
                    &mut last_connection_set,
                    &report_tx,
                ) {
                    return HciWorkerExit::Faulted(fault);
                }
            }

            recv(iso_wait) -> message => {
                let request = match message {
                    Ok(request) => request,
                    Err(_) => {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::IsoDataDisconnected,
                        )
                    }
                };
                pending_iso_group = Some(request);
            }
        }
    }
}

fn iso_data_group_should_drop(
    engine: &HciEngine,
    request: &IsoDataGroupRequest,
    blocked_iso_handles: &BTreeSet<u16>,
) -> bool {
    match request {
        IsoDataGroupRequest::Raw(group) => {
            iso_group_touches_handles(group, blocked_iso_handles)
        }
        IsoDataGroupRequest::Logical(group) => {
            let left = engine.resolve_stream_handle(
                group.session_id,
                group.stream_id,
                crate::architecture::types::AudioSide::Left,
            );
            let right = engine.resolve_stream_handle(
                group.session_id,
                group.stream_id,
                crate::architecture::types::AudioSide::Right,
            );
            match (left, right) {
                (Some(left), Some(right)) => {
                    blocked_iso_handles.contains(&left)
                        || blocked_iso_handles.contains(&right)
                }
                // Route/path invalidation makes queued realtime data stale.
                // Deliberately do not validate payload here: reliable teardown
                // must be able to supersede malformed/stale data backlog.
                _ => true,
            }
        }
    }
}

fn submit_iso_data_group(
    engine: &mut HciEngine,
    request: &IsoDataGroupRequest,
) -> Result<Option<IsoSubmitOutcome>, EngineFault> {
    match request {
        IsoDataGroupRequest::Raw(group) => {
            engine.submit_iso_group(group).map(Some)
        }
        IsoDataGroupRequest::Logical(group) => {
            engine.submit_logical_iso_group(group)
        }
    }
}

fn maybe_publish_iso_audit(
    engine: &HciEngine,
    command: &HciCommand,
    report_tx: &crossbeam_channel::Sender<HciReport>,
) -> Result<(), HciWorkerFault> {
    if matches!(command, HciCommand::ReadIsoTxAudit) {
        report_tx
            .try_send(HciReport::IsoTxAudit(
                engine.iso_credit_snapshot(),
            ))
            .map_err(map_report_error)?;
    }
    Ok(())
}

fn stream_route_bind_ack(
    binding: super::stream_route::StereoStreamRouteBinding,
    result: Result<(), EngineFault>,
) -> Result<StreamRouteState, EngineFault> {
    match result {
        Ok(()) => Ok(StreamRouteState::Bound {
            session_id: binding.session_id,
            stream_id: binding.stream_id,
        }),
        Err(EngineFault::StreamRouteConfig(error)) => {
            Ok(StreamRouteState::Failed(StreamRouteFailure {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
                kind: StreamRouteFailureKind::Config(error),
            }))
        }
        Err(EngineFault::StreamRouteCisNotReady(cis)) => {
            Ok(StreamRouteState::Failed(StreamRouteFailure {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
                kind: StreamRouteFailureKind::CisNotReady(cis),
            }))
        }
        Err(fault) => Err(fault),
    }
}

fn process_worker_command(
    engine: &mut HciEngine,
    command: HciCommand,
    blocked_iso_handles: &mut BTreeSet<u16>,
    pending_iso_group: &mut Option<IsoDataGroupRequest>,
    iso_drain_state: &mut IsoDrainState,
) -> Result<Option<StreamRouteState>, EngineFault> {
    let route_state =
        process_stream_route_command(engine, &command)?;
    if route_state.is_none() {
        process_command(
            engine,
            command,
            blocked_iso_handles,
            pending_iso_group,
            iso_drain_state,
        )?;
    }
    Ok(route_state)
}

fn process_stream_route_command(
    engine: &mut HciEngine,
    command: &HciCommand,
) -> Result<Option<StreamRouteState>, EngineFault> {
    match command {
        HciCommand::BindStreamRoute(binding) => Ok(Some(
            stream_route_bind_ack(
                *binding,
                engine.bind_stream_route(*binding),
            )?,
        )),
        HciCommand::UnbindStreamRoute {
            session_id,
            stream_id,
        } => Ok(Some(StreamRouteState::Unbound {
            session_id: *session_id,
            stream_id: *stream_id,
            was_bound: engine
                .unbind_stream_route(*session_id, *stream_id),
        })),
        _ => Ok(None),
    }
}

fn process_command(
    engine: &mut HciEngine,
    command: HciCommand,
    blocked_iso_handles: &mut BTreeSet<u16>,
    pending_iso_group: &mut Option<IsoDataGroupRequest>,
    iso_drain_state: &mut IsoDrainState,
) -> Result<(), EngineFault> {
    match command {
        HciCommand::BeginOperationalAttach => {
            engine.begin_operational_attach()
        }
        HciCommand::StartPassiveExtendedScan => {
            engine.begin_passive_extended_scan()
        }
        HciCommand::StopPassiveExtendedScan => {
            engine.stop_passive_extended_scan()
        }
        HciCommand::ReadControllerIdentity => {
            engine.begin_controller_identity_probe()
        }
        HciCommand::ConfigureControllerPrivacy(config) => {
            engine.begin_controller_privacy(config)
        }
        HciCommand::ClearControllerPrivacy => {
            engine.begin_controller_privacy_clear()
        }
        HciCommand::BeginFalConnection(request) => {
            engine.begin_fal_connection(request)
        }
        HciCommand::CancelFalConnection => {
            engine.cancel_fal_connection()
        }
        HciCommand::Disconnect { connection } => {
            engine.begin_disconnect(connection)
        }
        HciCommand::BeginStoredEncryption(request) => {
            engine.begin_stored_encryption(request)
        }
        HciCommand::EnsureCisHostFeature => {
            engine.begin_cis_host_feature()
        }
        HciCommand::ReadIsoTxAudit => Ok(()),
        HciCommand::ConfigureCig(config) => {
            engine.begin_cig_configuration(config)
        }
        HciCommand::CreateCis(request) => {
            engine.begin_cis_creation(request)
        }
        HciCommand::DisconnectCis { connection_handle } => {
            engine.begin_cis_disconnect(connection_handle)
        }
        HciCommand::DisconnectCisByRef { cis } => {
            engine.begin_cis_disconnect_by_ref(cis)
        }
        HciCommand::RemoveCig { cig_id } => {
            engine.begin_cig_removal(cig_id)
        }
        HciCommand::SetupIsoPath(config) => {
            engine.begin_iso_path_setup(config)
        }
        HciCommand::SetupIsoInputPathByRef { cis } => {
            engine.begin_iso_input_path_setup_by_ref(cis)
        }
        HciCommand::RemoveIsoPath {
            connection_handle,
            directions,
        } => engine.begin_iso_path_remove(
            connection_handle,
            directions,
        ),
        HciCommand::RemoveIsoPathByRef {
            cis,
            directions,
        } => engine.begin_iso_path_remove_by_ref(
            cis,
            directions,
        ),
        HciCommand::BindStreamRoute(_)
        | HciCommand::UnbindStreamRoute { .. } => {
            unreachable!(
                "stream route commands are handled before generic dispatch"
            )
        }
        HciCommand::BeginIsoDrain(request) => {
            begin_iso_drain(
                engine,
                request,
                blocked_iso_handles,
                pending_iso_group,
                iso_drain_state,
            )
        }
        HciCommand::ResumeIsoData(request) => {
            resume_iso_data(
                engine,
                request,
                blocked_iso_handles,
                iso_drain_state,
            )
        }
        HciCommand::BeginGatt(request) => {
            engine.begin_gatt_request(request)
        }
        HciCommand::CancelGatt {
            connection,
            request_id,
        } => engine.cancel_gatt_request(connection, request_id),
    }
}

fn begin_iso_drain(
    engine: &HciEngine,
    request: IsoDrainRequest,
    blocked_iso_handles: &mut BTreeSet<u16>,
    pending_iso_group: &mut Option<IsoDataGroupRequest>,
    iso_drain_state: &mut IsoDrainState,
) -> Result<(), EngineFault> {
    engine.validate_iso_drain(&request)?;

    match iso_drain_state {
        IsoDrainState::Idle => {}
        IsoDrainState::Draining(current)
        | IsoDrainState::Drained(current)
            if current == &request =>
        {
            return Ok(())
        }
        _ => return Err(EngineFault::InvalidState),
    }

    blocked_iso_handles.extend(
        request.connection_handles.iter().copied(),
    );
    let drop_pending = pending_iso_group
        .as_ref()
        .is_some_and(|group| {
            iso_data_group_should_drop(
                engine,
                group,
                blocked_iso_handles,
            )
        });
    if drop_pending {
        *pending_iso_group = None;
    }

    *iso_drain_state = if engine.iso_outputs_drained(&request) {
        IsoDrainState::Drained(request)
    } else {
        IsoDrainState::Draining(request)
    };
    Ok(())
}

fn resume_iso_data(
    engine: &HciEngine,
    request: IsoDrainRequest,
    blocked_iso_handles: &mut BTreeSet<u16>,
    iso_drain_state: &mut IsoDrainState,
) -> Result<(), EngineFault> {
    engine.validate_iso_drain(&request)?;
    if !engine.iso_input_paths_ready(&request) {
        return Err(EngineFault::InvalidState);
    }

    for handle in &request.connection_handles {
        blocked_iso_handles.remove(handle);
    }

    let reset_state = matches!(
        iso_drain_state,
        IsoDrainState::Draining(current)
            | IsoDrainState::Drained(current)
            if current == &request
    );
    if reset_state {
        *iso_drain_state = IsoDrainState::Idle;
    }
    Ok(())
}

fn publish_state_transitions(
    engine: &mut HciEngine,
    last_state: &mut OperationalAttachState,
    last_scan_state: &mut PassiveScanState,
    last_privacy_state: &mut ControllerPrivacyState,
    last_fal_connection_state: &mut FalConnectionState,
    last_disconnect_state: &mut DisconnectState,
    last_stored_encryption_state: &mut StoredEncryptionState,
    last_cis_host_feature_state: &mut CisHostFeatureState,
    last_cig_configure_state: &mut CigConfigureState,
    last_cis_create_state: &mut CisCreateState,
    last_cis_disconnect_state: &mut CisDisconnectState,
    last_cig_remove_state: &mut CigRemoveState,
    last_iso_path_state: &mut IsoPathOperationState,
    iso_drain_state: &mut IsoDrainState,
    last_iso_drain_state: &mut IsoDrainState,
    last_connection_set: &mut LeConnectionSetSnapshot,
    report_tx: &crossbeam_channel::Sender<HciReport>,
) -> Result<(), HciWorkerFault> {
    let state = engine.attach_state();
    if state != *last_state {
        report_tx
            .try_send(HciReport::AttachState(state))
            .map_err(map_report_error)?;
        *last_state = state;

        if state == OperationalAttachState::Ready {
            if let Some(capabilities) = engine.capabilities() {
                report_tx
                    .try_send(HciReport::CapabilitiesReady(capabilities))
                    .map_err(map_report_error)?;
            }
        }
    }

    if let Some(identity) = engine.take_controller_identity_result() {
        report_tx
            .try_send(HciReport::ControllerIdentity(identity))
            .map_err(map_report_error)?;
    }

    let scan_state = engine.passive_scan_state();
    if scan_state != *last_scan_state {
        report_tx
            .try_send(HciReport::PassiveScanState(scan_state))
            .map_err(map_report_error)?;
        *last_scan_state = scan_state;
    }

    if let Some(summary) = engine.take_completed_scan() {
        report_tx
            .try_send(HciReport::ScanComplete(summary))
            .map_err(map_report_error)?;
    }

    let privacy_state = engine.controller_privacy_state().clone();
    if privacy_state != *last_privacy_state {
        report_tx
            .try_send(HciReport::ControllerPrivacyState(
                privacy_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_privacy_state = privacy_state;
    }

    let fal_connection_state = engine.fal_connection_state().clone();
    if fal_connection_state != *last_fal_connection_state {
        report_tx
            .try_send(HciReport::FalConnectionState(
                fal_connection_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_fal_connection_state = fal_connection_state;
    }

    let disconnect_state = engine.disconnect_state().clone();
    if disconnect_state != *last_disconnect_state {
        report_tx
            .try_send(HciReport::DisconnectState(
                disconnect_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_disconnect_state = disconnect_state;
    }

    let stored_encryption_state =
        *engine.stored_encryption_state();
    if stored_encryption_state != *last_stored_encryption_state {
        report_tx
            .try_send(HciReport::StoredEncryptionState(
                stored_encryption_state,
            ))
            .map_err(map_report_error)?;
        *last_stored_encryption_state = stored_encryption_state;
    }

    let cis_host_feature_state =
        engine.cis_host_feature_state();
    if cis_host_feature_state != *last_cis_host_feature_state {
        report_tx
            .try_send(HciReport::CisHostFeatureState(
                cis_host_feature_state,
            ))
            .map_err(map_report_error)?;
        *last_cis_host_feature_state = cis_host_feature_state;
    }

    let cig_configure_state = engine.cig_configure_state().clone();
    if cig_configure_state != *last_cig_configure_state {
        report_tx
            .try_send(HciReport::CigConfigureState(
                cig_configure_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_cig_configure_state = cig_configure_state;
    }

    let cis_create_state = engine.cis_create_state().clone();
    if cis_create_state != *last_cis_create_state {
        report_tx
            .try_send(HciReport::CisCreateState(
                cis_create_state.clone(),
            ))
            .map_err(map_report_error)?;
        report_tx
            .try_send(HciReport::SemanticCisCreateState(
                cis_create_state.semantic(),
            ))
            .map_err(map_report_error)?;
        *last_cis_create_state = cis_create_state;
    }

    let cis_disconnect_state = engine.cis_disconnect_state().clone();
    if cis_disconnect_state != *last_cis_disconnect_state {
        report_tx
            .try_send(HciReport::CisDisconnectState(
                cis_disconnect_state.clone(),
            ))
            .map_err(map_report_error)?;
        if let Some(semantic_state) =
            engine.semantic_cis_disconnect_state()
        {
            report_tx
                .try_send(HciReport::SemanticCisDisconnectState(
                    semantic_state,
                ))
                .map_err(map_report_error)?;
        }
        *last_cis_disconnect_state = cis_disconnect_state;
    }

    let cig_remove_state = engine.cig_remove_state().clone();
    if cig_remove_state != *last_cig_remove_state {
        report_tx
            .try_send(HciReport::CigRemoveState(
                cig_remove_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_cig_remove_state = cig_remove_state;
    }

    let iso_path_state = engine.iso_path_state().clone();
    if iso_path_state != *last_iso_path_state {
        report_tx
            .try_send(HciReport::IsoPathState(
                iso_path_state.clone(),
            ))
            .map_err(map_report_error)?;
        if let Some(semantic_state) =
            engine.semantic_iso_path_state()
        {
            report_tx
                .try_send(HciReport::SemanticIsoPathState(
                    semantic_state,
                ))
                .map_err(map_report_error)?;
        }
        *last_iso_path_state = iso_path_state;
    }

    if let IsoDrainState::Draining(request) = iso_drain_state.clone() {
        if engine.iso_outputs_drained(&request) {
            *iso_drain_state = IsoDrainState::Drained(request);
        }
    }
    if *iso_drain_state != *last_iso_drain_state {
        report_tx
            .try_send(HciReport::IsoDrainState(
                iso_drain_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_iso_drain_state = iso_drain_state.clone();
    }

    let connection_set = engine.connection_set_snapshot();
    if connection_set != *last_connection_set {
        report_tx
            .try_send(HciReport::ConnectionSet(
                connection_set.clone(),
            ))
            .map_err(map_report_error)?;
        *last_connection_set = connection_set;
    }

    for report in engine.take_gatt_reports() {
        report_tx
            .try_send(HciReport::Gatt(report))
            .map_err(map_report_error)?;
    }

    Ok(())
}

fn map_report_error(
    error: TrySendError<HciReport>,
) -> HciWorkerFault {
    match error {
        TrySendError::Full(_) => HciWorkerFault::ReportFull,
        TrySendError::Disconnected(_) => HciWorkerFault::ReportDisconnected,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::{
        hci_host::{
            iso::{
                CigAllocation, CigConfig, CigConfigureState, CisAllocation,
                CisConfig,
            },
            iso_tx::IsoSduGroupRequest,
            mailbox::{hci_mailbox, hci_mailbox_with_iso},
        },
        plane::{
            l1_hci::{
                acl::{
                    connection::{
                        FalConnectRequest, LeInitiationParameters, LePeerIdentity,
                    },
                    privacy::{
                        ResolvingListConfig, ResolvingPeer,
                        DEFAULT_RPA_TIMEOUT_SECONDS,
                    },
                },
                gatt::transaction::{
                    GattFailure, GattFailureKind, GattOperation, GattReport,
                    GattRequest,
                },
            },
            l2_transport::{
            packet::HostPacket,
            sink::ControllerPacketSink,
            },
        },
        runtime::mailbox::RecvError,
    };

    use super::*;

    fn recv_host_packet(
        rx: &crate::runtime::mailbox::BoundedReceiver<HostPacket>,
    ) -> HostPacket {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match rx.try_recv() {
                Ok(packet) => return packet,
                Err(RecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for host packet"
                    );
                    thread::yield_now();
                }
                Err(RecvError::Disconnected) => {
                    panic!("transport test channel disconnected")
                }
            }
        }
    }

    fn recv_report(
        reports: &crate::hci_host::mailbox::HciReportInbox,
    ) -> HciReport {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match reports.try_recv() {
                Ok(report) => return report,
                Err(RecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for HCI report"
                    );
                    thread::yield_now();
                }
                Err(RecvError::Disconnected) => {
                    panic!("HCI report channel disconnected")
                }
            }
        }
    }

    fn route_binding() -> super::super::stream_route::StereoStreamRouteBinding {
        super::super::stream_route::StereoStreamRouteBinding {
            session_id: crate::architecture::types::SessionId(7),
            stream_id: crate::architecture::types::StreamId(11),
            left: crate::hci_host::iso::CisRef {
                cig_id: 1,
                cis_id: 1,
            },
            right: crate::hci_host::iso::CisRef {
                cig_id: 1,
                cis_id: 2,
            },
        }
    }

    #[test]
    fn queued_logical_group_without_live_route_is_stale_drop() {
        let (transport_tx, _transport_rx) =
            TransportTx::test_channel(4);
        let engine = HciEngine::new(transport_tx);
        let request = IsoDataGroupRequest::Logical(
            crate::hci_host::iso_tx::LogicalIsoSduGroupRequest {
                session_id:
                    crate::architecture::types::SessionId(7),
                stream_id:
                    crate::architecture::types::StreamId(11),
                left: vec![0x11; 100],
                right: vec![0x22; 100],
            },
        );

        assert!(
            iso_data_group_should_drop(
                &engine,
                &request,
                &BTreeSet::new(),
            )
        );
    }

    #[test]
    fn queued_raw_group_still_obeys_reliable_drain_block_set() {
        let (transport_tx, _transport_rx) =
            TransportTx::test_channel(4);
        let engine = HciEngine::new(transport_tx);
        let request = IsoDataGroupRequest::Raw(
            IsoSduGroupRequest {
                sdus: vec![
                    crate::hci_host::iso_tx::IsoSduRequest {
                        connection_handle: 0x0900,
                        payload: vec![0x11; 100],
                    },
                    crate::hci_host::iso_tx::IsoSduRequest {
                        connection_handle: 0x0901,
                        payload: vec![0x22; 100],
                    },
                ],
            },
        );

        assert!(
            iso_data_group_should_drop(
                &engine,
                &request,
                &BTreeSet::from([0x0901]),
            )
        );
        assert!(
            !iso_data_group_should_drop(
                &engine,
                &request,
                &BTreeSet::from([0x0902]),
            )
        );
    }

    #[test]
    fn stale_logical_data_drop_does_not_validate_payload() {
        let (transport_tx, _transport_rx) =
            TransportTx::test_channel(4);
        let engine = HciEngine::new(transport_tx);
        let request = IsoDataGroupRequest::Logical(
            crate::hci_host::iso_tx::LogicalIsoSduGroupRequest {
                session_id: crate::architecture::types::SessionId(7),
                stream_id: crate::architecture::types::StreamId(11),
                left: vec![
                    0;
                    crate::hci_host::iso_tx::MAX_ISO_SDU_LENGTH + 1
                ],
                right: vec![0; 100],
            },
        );

        assert!(
            iso_data_group_should_drop(
                &engine,
                &request,
                &BTreeSet::new(),
            ),
            "missing route must make malformed realtime data stale without blocking teardown",
        );
    }

    #[test]
    fn logical_route_ack_maps_expected_engine_results() {
        let binding = route_binding();

        assert_eq!(
            stream_route_bind_ack(binding, Ok(())),
            Ok(StreamRouteState::Bound {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
            })
        );

        let config =
            super::super::stream_route::StreamRouteConfigError::
                DuplicateCis(binding.left);
        assert_eq!(
            stream_route_bind_ack(
                binding,
                Err(EngineFault::StreamRouteConfig(config)),
            ),
            Ok(StreamRouteState::Failed(StreamRouteFailure {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
                kind: StreamRouteFailureKind::Config(config),
            }))
        );

        assert_eq!(
            stream_route_bind_ack(
                binding,
                Err(EngineFault::StreamRouteCisNotReady(
                    binding.right,
                )),
            ),
            Ok(StreamRouteState::Failed(StreamRouteFailure {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
                kind: StreamRouteFailureKind::CisNotReady(
                    binding.right,
                ),
            }))
        );

        assert_eq!(
            stream_route_bind_ack(
                binding,
                Err(EngineFault::InvalidState),
            ),
            Err(EngineFault::InvalidState),
        );
    }

    #[test]
    fn logical_route_commands_ack_not_ready_and_unbound_without_fault() {
        let (transport_tx, _transport_rx) =
            TransportTx::test_channel(4);
        let mut engine = HciEngine::new(transport_tx);
        let binding = route_binding();

        assert_eq!(
            process_stream_route_command(
                &mut engine,
                &HciCommand::BindStreamRoute(binding),
            )
            .unwrap(),
            Some(StreamRouteState::Failed(StreamRouteFailure {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
                kind: StreamRouteFailureKind::CisNotReady(
                    binding.left,
                ),
            }))
        );

        assert_eq!(
            process_stream_route_command(
                &mut engine,
                &HciCommand::UnbindStreamRoute {
                    session_id: binding.session_id,
                    stream_id: binding.stream_id,
                },
            )
            .unwrap(),
            Some(StreamRouteState::Unbound {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
                was_bound: false,
            })
        );
    }

    #[test]
    fn worker_owns_device_and_completes_minimal_resetless_attach() {
        let (control, ingress, reports, ports) = hci_mailbox(16, 4, 8);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(16);
        let join = spawn_hci_worker(ports, transport_tx);

        control.try_begin_operational_attach().unwrap();

        assert_eq!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(vec![0x02, 0x10, 0x00])
        );
        assert_eq!(
            recv_report(&reports),
            HciReport::AttachState(
                OperationalAttachState::Discovering
            )
        );

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();

        // Minimal all-zero command bitmap causes only mandatory event masks.
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();

        assert_eq!(
            recv_report(&reports),
            HciReport::AttachState(OperationalAttachState::Ready)
        );
        assert!(matches!(
            recv_report(&reports),
            HciReport::CapabilitiesReady(_)
        ));

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn worker_routes_controller_privacy_as_typed_transaction() {
        let (control, ingress, reports, ports) = hci_mailbox(32, 8, 16);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(32);
        let join = spawn_hci_worker(ports, transport_tx);

        // Minimal resetless attach to Ready.
        control.try_begin_operational_attach().unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_report(&reports);

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_host_packet(&transport_rx);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();
        let _ = recv_report(&reports);
        let _ = recv_report(&reports);

        control
            .try_configure_controller_privacy(ResolvingListConfig {
                peers: vec![ResolvingPeer {
                    identity_address_le: [
                        0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8,
                    ],
                    identity_address_type: 0,
                    peer_irk: [0x11; 16],
                }],
                local_irk: [0x22; 16],
                rpa_timeout_seconds: DEFAULT_RPA_TIMEOUT_SECONDS,
            })
            .unwrap();

        let opcode = |packet: HostPacket| {
            let HostPacket::Command(bytes) = packet else {
                panic!("expected HCI command");
            };
            u16::from_le_bytes([bytes[0], bytes[1]])
        };

        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x202d);
        assert_eq!(
            recv_report(&reports),
            HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Configuring,
            )
        );

        for (current, extra, next) in [
            (0x202d, &[][..], 0x2029),
            (0x2029, &[][..], 0x202a),
            (0x202a, &[1][..], 0x2027),
            (0x2027, &[][..], 0x202e),
            (0x202e, &[][..], 0x202d),
        ] {
            let mut event = vec![
                0x0e,
                (4 + extra.len()) as u8,
                0x01,
                current as u8,
                (current >> 8) as u8,
                0x00,
            ];
            event.extend_from_slice(extra);
            ingress
                .try_send_controller(ControllerPacket::Event(event))
                .unwrap();
            assert_eq!(opcode(recv_host_packet(&transport_rx)), next);
        }

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x2d, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(
            recv_report(&reports),
            HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            )
        );

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn worker_routes_fal_connection_as_typed_transaction() {
        let (control, ingress, reports, ports) = hci_mailbox(48, 8, 24);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(48);
        let join = spawn_hci_worker(ports, transport_tx);

        // Minimal resetless attach to Ready.
        control.try_begin_operational_attach().unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_report(&reports);

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_host_packet(&transport_rx);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();
        let _ = recv_report(&reports);
        let _ = recv_report(&reports);

        control
            .try_begin_fal_connection(FalConnectRequest {
                peer: LePeerIdentity {
                    address_le: [
                        0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8,
                    ],
                    address_type: 0,
                },
                parameters: LeInitiationParameters::default(),
            })
            .unwrap();

        let opcode = |packet: HostPacket| {
            let HostPacket::Command(bytes) = packet else {
                panic!("expected HCI command");
            };
            u16::from_le_bytes([bytes[0], bytes[1]])
        };

        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x200f);
        assert_eq!(
            recv_report(&reports),
            HciReport::FalConnectionState(
                FalConnectionState::Preparing,
            )
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x05, 0x01, 0x0f, 0x20, 0x00, 0x08,
            ]))
            .unwrap();
        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x2010);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x10, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x2011);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x11, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x2043);
        assert_eq!(
            recv_report(&reports),
            HciReport::FalConnectionState(
                FalConnectionState::Initiating,
            )
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0f, 0x04, 0x00, 0x01, 0x43, 0x20,
            ]))
            .unwrap();
        assert_eq!(
            recv_report(&reports),
            HciReport::FalConnectionState(
                FalConnectionState::AwaitingConnection,
            )
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();
        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x2010);
        assert_eq!(
            recv_report(&reports),
            HciReport::FalConnectionState(
                FalConnectionState::Cleaning,
            )
        );
        match recv_report(&reports) {
            HciReport::ConnectionSet(snapshot) => {
                assert_eq!(snapshot.connections.len(), 1);
                assert_eq!(
                    snapshot.connections[0].connection_handle,
                    0x0800
                );
            }
            other => {
                panic!("expected connection-set report, got {other:?}")
            }
        }

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x10, 0x20, 0x00,
            ]))
            .unwrap();

        match recv_report(&reports) {
            HciReport::FalConnectionState(
                FalConnectionState::Connected(connection),
            ) => {
                assert_eq!(connection.connection_handle, 0x0800);
                assert_eq!(connection.connection_interval, 24);
            }
            other => panic!("expected connected report, got {other:?}"),
        }

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn worker_routes_cig_configuration_through_bounded_mailbox() {
        let (control, ingress, reports, ports) =
            hci_mailbox(32, 8, 24);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(32);
        let join = spawn_hci_worker(ports, transport_tx);

        control.try_begin_operational_attach().unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_report(&reports);

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_host_packet(&transport_rx);

        for event in [
            vec![0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00],
            vec![0x0e, 0x04, 0x01, 0x01, 0x20, 0x00],
        ] {
            ingress
                .try_send_controller(ControllerPacket::Event(event))
                .unwrap();
        }
        let _ = recv_report(&reports);
        let _ = recv_report(&reports);

        let cis = |cis_id| CisConfig {
            cis_id,
            max_sdu_c_to_p: 100,
            max_sdu_p_to_c: 0,
            phy_c_to_p: 2,
            phy_p_to_c: 1,
            rtn_c_to_p: 13,
            rtn_p_to_c: 0,
        };
        control
            .try_configure_cig(CigConfig {
                cig_id: 1,
                sdu_interval_c_to_p: 10_000,
                sdu_interval_p_to_c: 10_000,
                worst_case_sca: 0,
                packing: 0,
                framing: 0,
                max_transport_latency_c_to_p: 95,
                max_transport_latency_p_to_c: 95,
                cis: vec![cis(1), cis(2)],
            })
            .unwrap();

        let HostPacket::Command(command) =
            recv_host_packet(&transport_rx)
        else {
            panic!("expected Set CIG command");
        };
        assert_eq!(u16::from_le_bytes([command[0], command[1]]), 0x2062);
        assert_eq!(
            recv_report(&reports),
            HciReport::CigConfigureState(CigConfigureState::Configuring)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x0a, 0x01, 0x62, 0x20, 0x00,
                0x01, 0x02, 0x40, 0x00, 0x41, 0x00,
            ]))
            .unwrap();

        assert_eq!(
            recv_report(&reports),
            HciReport::CigConfigureState(CigConfigureState::Ready(
                CigAllocation {
                    cig_id: 1,
                    cis: vec![
                        CisAllocation {
                            cis_id: 1,
                            connection_handle: 0x0040,
                        },
                        CisAllocation {
                            cis_id: 2,
                            connection_handle: 0x0041,
                        },
                    ],
                }
            ))
        );

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn worker_routes_gatt_through_bounded_mailbox() {
        let (control, ingress, reports, ports) =
            hci_mailbox(32, 8, 24);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(32);
        let join = spawn_hci_worker(ports, transport_tx);

        control.try_begin_operational_attach().unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_report(&reports);

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_host_packet(&transport_rx);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();
        let _ = recv_report(&reports);
        let _ = recv_report(&reports);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();

        let connection = match recv_report(&reports) {
            HciReport::ConnectionSet(snapshot) => {
                snapshot.connections[0].token()
            }
            other => panic!(
                "expected connection-set report, got {other:?}"
            ),
        };

        control
            .try_begin_gatt(GattRequest {
                request_id: 77,
                connection,
                operation: GattOperation::DiscoverServices,
            })
            .unwrap();

        assert_eq!(
            recv_report(&reports),
            HciReport::Gatt(GattReport::Failed(GattFailure {
                request_id: 77,
                connection,
                kind: GattFailureKind::NotEncrypted,
            }))
        );

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn worker_routes_passive_scan_as_typed_transaction() {
        let (control, ingress, reports, ports) = hci_mailbox(32, 8, 16);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(32);
        let join = spawn_hci_worker(ports, transport_tx);

        // Bring the synthetic controller to the minimal resetless Ready state.
        control.try_begin_operational_attach().unwrap();
        assert_eq!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(vec![0x02, 0x10, 0x00])
        );
        assert!(matches!(
            recv_report(&reports),
            HciReport::AttachState(OperationalAttachState::Discovering)
        ));

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_host_packet(&transport_rx);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();

        assert!(matches!(
            recv_report(&reports),
            HciReport::AttachState(OperationalAttachState::Ready)
        ));
        assert!(matches!(
            recv_report(&reports),
            HciReport::CapabilitiesReady(_)
        ));

        control.try_start_passive_extended_scan().unwrap();
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));
        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Configuring)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x41, 0x20, 0x00,
            ]))
            .unwrap();
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));
        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Enabling)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x42, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Scanning)
        );

        control.try_stop_passive_extended_scan().unwrap();
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));
        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Disabling)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x42, 0x20, 0x00,
            ]))
            .unwrap();

        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Idle)
        );
        match recv_report(&reports) {
            HciReport::ScanComplete(summary) => {
                assert_eq!(summary.reports_seen, 0);
                assert!(summary.observations.is_empty());
                assert!(!summary.truncated);
            }
            other => panic!("unexpected report: {other:?}"),
        }

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    fn iso_test_group() -> IsoSduGroupRequest {
        IsoSduGroupRequest {
            sdus: vec![crate::hci_host::iso_tx::IsoSduRequest {
                connection_handle: 0x0900,
                payload: vec![0x55; 100],
            }],
        }
    }

    #[test]
    fn urgent_stop_is_not_blocked_by_full_iso_data_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 1, 4, 1);
        let iso = control.iso_tx();
        let (transport_tx, _transport_rx) =
            TransportTx::test_channel(4);

        iso.try_submit_group(iso_test_group()).unwrap();
        assert_eq!(
            iso.try_submit_group(iso_test_group()),
            Err(crate::runtime::mailbox::SendError::Full)
        );
        control.request_stop().unwrap();

        let join = spawn_hci_worker(ports, transport_tx);
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn control_lane_is_serviced_before_iso_data_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 1, 4, 1);
        let iso = control.iso_tx();
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(4);

        control.try_begin_operational_attach().unwrap();
        iso.try_submit_group(iso_test_group()).unwrap();

        let join = spawn_hci_worker(ports, transport_tx);
        assert_eq!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(vec![0x02, 0x10, 0x00])
        );
        assert_eq!(
            join.join(),
            HciWorkerExit::Faulted(HciWorkerFault::Engine(
                EngineFault::InvalidState
            ))
        );
    }

    #[test]
    fn urgent_stop_is_not_blocked_by_full_controller_lane() {
        let (control, ingress, _reports, ports) =
            hci_mailbox(1, 1, 4);
        let (transport_tx, _transport_rx) =
            TransportTx::test_channel(4);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![1]))
            .unwrap();
        control.request_stop().unwrap();

        let join = spawn_hci_worker(ports, transport_tx);
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }
}
