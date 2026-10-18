//! Rollback/teardown orchestration for ControlMachine.
//!
//! State is owned by operation::RollbackOperation. This module keeps the
//! established route -> path -> CIS -> ASE -> CIG -> ACL/privacy ordering and
//! fail-closed cleanup decisions out of the main engine file.

use super::*;

impl ControlMachine {
    pub(super) fn start_rollback_ase_release(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.confirmed.prepared_ases[side_index(side)]
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
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::AseRelease {
                        side,
                        failure,
                    },
                );
                return self.continue_rollback();
            }
        };
        self.next_gatt_request_id = machine.next_request_id();
        self.rollback.ase_current = Some(side);
        self.rollback.ase_operation = Some(machine);

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::CleanupAse { side },
        )];
        actions.extend(
            self.translate_rollback_ase_action(side, None, action)?,
        );
        Ok(actions)
    }

    pub(super) fn on_rollback_ase_gatt_report(
        &mut self,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let side = self.rollback.ase_current
            .expect("rollback ASE report requires current side");
        let mut machine = self.rollback.ase_operation
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
                self.rollback.ase_operation = Some(machine);
                Ok(Vec::new())
            }
            Ok(AseOperationAction::Await) => {
                self.rollback.ase_operation = Some(machine);
                self.disarm_gatt_request(side, completed_request)
            }
            Ok(action @ AseOperationAction::BeginGatt(_)) => {
                self.rollback.ase_operation = Some(machine);
                self.translate_rollback_ase_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Ok(action @ AseOperationAction::Complete(_)) => {
                self.rollback.ase_operation = None;
                self.translate_rollback_ase_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Err(failure) => {
                self.rollback.ase_operation = None;
                self.rollback.ase_current = None;
                self.rollback.cleanup_failures.push(
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

    pub(super) fn translate_rollback_ase_action(
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
                    self.rollback.cleanup_failures.push(
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
                self.rollback.ase_current = None;
                actions.push(ControlAction::DisarmTimer(
                    ControlTimer::CleanupAse { side },
                ));
                actions.extend(self.continue_rollback()?);
                Ok(actions)
            }
        }
    }

    pub(super) fn prepare_stream_route_rollback(&mut self) {
        let binding = self.confirmed.active_stream_binding
            .or(self.stream_route_operation.pending_binding);
        self.rollback.stream_route =
            binding.map(|value| (value.session_id, value.stream_id));
        self.rollback.stream_route_unbind_pending = false;
        self.stream_route_operation.pending_binding = None;
        self.confirmed.active_stream_binding = None;
    }

    /// Begin a normal product teardown from a route-bound stream.
    ///
    /// This reuses the proven rollback ordering but carries no primary failure:
    /// successful cleanup returns Control to Idle, while cleanup failures become
    /// TeardownFailed.
    pub fn begin_teardown(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::StreamBoundReady {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }

        self.failure = None;
        self.prepare_stream_route_rollback();
        self.rollback.acl_queue = self.confirmed.connections
            .iter()
            .flatten()
            .copied()
            .collect();
        self.rollback.acl_current = None;

        self.rollback.iso_path_queue =
            self.cleanup_knowledge.iso_paths_may_exist.iter().copied().collect();
        self.rollback.iso_path_current = None;

        self.rollback.cis_queue =
            self.confirmed.live_cis.iter().copied().collect();
        self.rollback.cis_current = None;

        self.rollback.ase_queue.clear();
        for side in [AudioSide::Left, AudioSide::Right] {
            if self.confirmed.prepared_ases[side_index(side)].is_some() {
                self.rollback.ase_queue.push(side);
            }
        }
        self.rollback.ase_current = None;
        self.rollback.ase_operation = None;
        self.rollback.cleanup_failures.clear();

        self.phase = ControlPhase::RollingBack;
        self.continue_rollback()
    }

    pub(super) fn begin_rollback(
        &mut self,
        failure: ControlFailure,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.failure = Some(failure);
        self.prepare_stream_route_rollback();
        self.rollback.acl_queue = self.confirmed.connections
            .iter()
            .flatten()
            .copied()
            .collect();
        self.rollback.acl_current = None;
        self.rollback.iso_path_queue.clear();
        self.rollback.iso_path_current = None;
        self.rollback.cis_queue.clear();
        self.rollback.cis_current = None;
        self.rollback.ase_queue.clear();
        self.rollback.ase_current = None;
        self.rollback.ase_operation = None;
        self.rollback.cleanup_failures.clear();
        self.phase = ControlPhase::RollingBack;
        self.continue_rollback()
    }

    pub(super) fn begin_stream_rollback(
        &mut self,
        failure: ControlFailure,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.failure = Some(failure);
        self.prepare_stream_route_rollback();
        self.rollback.acl_queue = self.confirmed.connections
            .iter()
            .flatten()
            .copied()
            .collect();
        self.rollback.acl_current = None;

        self.rollback.iso_path_queue =
            self.cleanup_knowledge.iso_paths_may_exist.iter().copied().collect();
        self.rollback.iso_path_current = None;

        self.rollback.cis_queue =
            self.confirmed.live_cis.iter().copied().collect();
        self.rollback.cis_current = None;

        self.rollback.ase_queue.clear();
        for side in [AudioSide::Left, AudioSide::Right] {
            if self.confirmed.prepared_ases[side_index(side)].is_some() {
                self.rollback.ase_queue.push(side);
            }
        }
        self.rollback.ase_current = None;
        self.rollback.ase_operation = None;

        self.rollback.cleanup_failures.clear();
        self.phase = ControlPhase::RollingBack;
        self.continue_rollback()
    }

    pub(super) fn continue_rollback(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.phase = ControlPhase::RollingBack;

        if let Some((session_id, stream_id)) =
            self.rollback.stream_route
        {
            if !self.rollback.stream_route_unbind_pending {
                self.rollback.stream_route_unbind_pending = true;
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

        if self.rollback.iso_path_current.is_none() {
            if let Some(cis) = self.rollback.iso_path_queue.pop() {
                self.rollback.iso_path_current = Some(cis);
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
        if self.rollback.iso_path_current.is_some() {
            return Ok(Vec::new());
        }

        if self.rollback.cis_current.is_none() {
            if let Some(cis) = self.rollback.cis_queue.pop() {
                self.rollback.cis_current = Some(cis);
                return Ok(vec![
                    ControlAction::DisconnectCisByRef { cis },
                    ControlAction::ArmTimer(
                        ControlTimer::CleanupCis { cis },
                    ),
                ]);
            }
        }
        if self.rollback.cis_current.is_some() {
            return Ok(Vec::new());
        }

        if self.rollback.ase_current.is_none() {
            if let Some(side) = self.rollback.ase_queue.pop() {
                return self.start_rollback_ase_release(side);
            }
        }
        if self.rollback.ase_current.is_some() {
            return Ok(Vec::new());
        }

        if self.cleanup_knowledge.cig_may_exist {
            if self.rollback.cig_pending {
                return Ok(Vec::new());
            }
            let cig_id = self.planned.iso_plan
                .as_ref()
                .expect("CIG cleanup requires ISO plan")
                .cig_id;
            self.rollback.cig_pending = true;
            return Ok(vec![
                ControlAction::RemoveCig { cig_id },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupCig { cig_id },
                ),
            ]);
        }

        if self.rollback.acl_current.is_none() {
            if let Some(connection) = self.rollback.acl_queue.pop() {
                self.rollback.acl_current = Some(connection);
                return Ok(vec![
                    ControlAction::DisconnectAcl { connection },
                    ControlAction::ArmTimer(
                        ControlTimer::CleanupDisconnect(connection),
                    ),
                ]);
            }
        }

        if self.rollback.acl_current.is_some() {
            return Ok(Vec::new());
        }

        if self.confirmed.privacy_active {
            return Ok(vec![
                ControlAction::ClearControllerPrivacy,
                ControlAction::ArmTimer(
                    ControlTimer::CleanupPrivacy,
                ),
            ]);
        }

        self.finish_rollback()
    }

    fn reset_after_successful_teardown(&mut self) {
        self.confirmed = ConfirmedStatus::new();
        self.planned = PlannedConfiguration::new();
        self.cleanup_knowledge = CleanupKnowledge::new();
        self.rollback = RollbackOperation::new();
        self.discovery_operation = AudioDiscoveryOperation::new();
        self.ase_control_operation = AseControlOperation::new();
        self.streaming_operation = StreamingOperation::new();
        self.stream_route_operation = StreamRouteOperation::new();
        self.failure = None;
        self.phase = ControlPhase::Idle;
    }

    pub(super) fn finish_rollback(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.failure.is_none() {
            if self.rollback.cleanup_failures.is_empty() {
                self.reset_after_successful_teardown();
                return Ok(Vec::new());
            }

            self.phase = ControlPhase::Faulted;
            let final_failure = ControlFailure::TeardownFailed {
                cleanup: std::mem::take(
                    &mut self.rollback.cleanup_failures,
                ),
            };
            self.failure = Some(final_failure.clone());
            return Err(final_failure);
        }

        self.phase = ControlPhase::Faulted;
        let primary = self.failure.take()
            .expect("failure rollback requires primary failure");
        let final_failure = if self.rollback.cleanup_failures.is_empty() {
            primary
        } else {
            ControlFailure::RollbackFailed {
                primary: Box::new(primary),
                cleanup: std::mem::take(
                    &mut self.rollback.cleanup_failures,
                ),
            }
        };
        self.failure = Some(final_failure.clone());
        Err(final_failure)
    }
}
