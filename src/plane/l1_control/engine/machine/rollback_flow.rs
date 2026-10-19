//! Rollback/teardown orchestration for ControlMachine.
//!
//! State is owned by operation::RollbackOperation.
//!
//! Failure rollback keeps the established fail-closed
//! route -> path -> CIS -> ASE Release -> CIG -> ACL/privacy ordering.
//!
//! Product Stop is intentionally protocol-ungraceful but controller-clean:
//! route -> path -> CIS -> CIG -> ACL/privacy. It does not depend on the
//! ASCS/GATT bearer remaining responsive. Serial/Group ASE Release modes remain
//! available only to Review/HIL so the historical graceful path can still be
//! diagnosed without blocking the product lifecycle.

use super::*;

impl ControlMachine {
    fn gatt_report_connection(report: &GattReport) -> ConnectionToken {
        match report {
            GattReport::Complete { connection, .. }
            | GattReport::BearerPoisoned { connection, .. }
            | GattReport::LateResponseDropped { connection, .. }
            | GattReport::UnexpectedPdu { connection, .. } => *connection,
            GattReport::Failed(failure) => failure.connection,
            GattReport::Value(
                GattValueEvent::Notification { connection, .. }
                | GattValueEvent::Indication { connection, .. },
            ) => *connection,
        }
    }

    pub(super) fn has_group_rollback_ase_operation(&self) -> bool {
        self.rollback
            .ase_group_operations
            .iter()
            .any(Option::is_some)
    }

    pub(super) fn group_rollback_side_for_request(
        &self,
        member_index: usize,
        request_id: GattRequestId,
    ) -> Option<AudioSide> {
        [AudioSide::Left, AudioSide::Right]
            .into_iter()
            .find(|side| {
                self.rollback.ase_group_operations[side_index(*side)]
                    .as_ref()
                    .and_then(AseOperationMachine::pending_request_id)
                    == Some(request_id)
                    && self
                        .planned_sink(*side)
                        .ok()
                        .map(|sink| sink.member_index)
                        == Some(member_index)
            })
    }

    fn start_graceful_group_ase_releases(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let sides = std::mem::take(&mut self.rollback.ase_queue);
        let mut actions = Vec::new();

        for side in sides {
            let prepared = self.confirmed.prepared_ases[side_index(side)]
                .as_ref()
                .expect("group Release requires prepared control")
                .clone();
            let ase_id = prepared.sink.ase_id;
            let mut machine = AseOperationMachine::new_notification_gate(
                prepared,
                AseOperation::Release(vec![ase_id]),
                AseState::RELEASING,
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
                    continue;
                }
            };

            self.next_gatt_request_id = machine.next_request_id();
            self.rollback.ase_group_operations[side_index(side)] =
                Some(machine);
            actions.push(ControlAction::ArmTimer(
                ControlTimer::CleanupAse { side },
            ));
            actions.extend(
                self.translate_rollback_ase_action(
                    side,
                    None,
                    action,
                )?,
            );
        }

        if self.has_group_rollback_ase_operation() {
            Ok(actions)
        } else {
            actions.extend(self.continue_rollback()?);
            Ok(actions)
        }
    }

    pub(super) fn on_group_rollback_ase_gatt_report(
        &mut self,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let connection = Self::gatt_report_connection(report);
        let mut side = None;
        for candidate in [AudioSide::Left, AudioSide::Right] {
            if self.rollback.ase_group_operations[side_index(candidate)]
                .is_some()
                && self.planned_sink(candidate)?.connection == connection
            {
                side = Some(candidate);
                break;
            }
        }
        let Some(side) = side else {
            return Ok(Vec::new());
        };

        let index = side_index(side);
        let mut machine = self.rollback.ase_group_operations[index]
            .take()
            .expect("group rollback report requires active operation");
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
                self.rollback.ase_group_operations[index] =
                    Some(machine);
                Ok(Vec::new())
            }
            Ok(AseOperationAction::Await) => {
                self.rollback.ase_group_operations[index] =
                    Some(machine);
                self.disarm_gatt_request(side, completed_request)
            }
            Ok(action @ AseOperationAction::BeginGatt(_)) => {
                self.rollback.ase_group_operations[index] =
                    Some(machine);
                self.translate_rollback_ase_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Ok(action @ AseOperationAction::Complete(_)) => {
                self.translate_rollback_ase_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Err(failure) => {
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

    pub(super) fn start_rollback_ase_release(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.confirmed.prepared_ases[side_index(side)]
            .as_ref()
            .expect("rollback ASE release requires prepared control")
            .clone();
        let ase_id = prepared.sink.ase_id;
        let operation = AseOperation::Release(vec![ase_id]);
        let mut machine = if self.failure.is_none() {
            // Graceful Stop must pivot on RELEASING while the ISO path/CIS
            // are still live. Waiting for terminal IDLE here deadlocks peers
            // that only finish Release after transport teardown.
            AseOperationMachine::new_notification_gate(
                prepared,
                operation,
                AseState::RELEASING,
                self.next_gatt_request_id,
            )
        } else {
            AseOperationMachine::new_with_states(
                prepared,
                operation,
                AseState::RELEASING,
                AseState::IDLE,
                self.next_gatt_request_id,
            )
        };
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
                let valid = if self.failure.is_none() {
                    status.state == AseState::RELEASING
                } else {
                    matches!(
                        status.state,
                        AseState::IDLE | AseState::CODEC_CONFIGURED
                    )
                };
                if !valid {
                    self.rollback.cleanup_failures.push(
                        ControlCleanupFailure::AseRelease {
                            side,
                            failure:
                                AseOperationFailure::ReadbackStateMismatch {
                                    ase_id: status.ase_id,
                                    expected_state: if self.failure.is_none() {
                                        AseState::RELEASING.0
                                    } else {
                                        AseState::IDLE.0
                                    },
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

    /// Begin the product teardown from a route-bound stream.
    ///
    /// Product Stop deliberately skips ASCS Release. The peer-facing GATT
    /// control bearer has proven capable of becoming permanently unresponsive
    /// while HCI/LL state remains live; shutdown therefore depends only on
    /// controller-owned resources that the Host can retire deterministically.
    ///
    /// Ordering remains semantic and controller-clean:
    /// route -> ISO path -> CIS -> CIG -> ACL -> privacy.
    pub fn begin_teardown(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.begin_teardown_with_release_mode(
            GracefulAseReleaseMode::Skip,
        )
    }

    pub(crate) fn begin_emergency_teardown_for_review(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if !matches!(
            self.phase,
            ControlPhase::CodecConfigured
                | ControlPhase::StreamingReady
                | ControlPhase::StreamBoundReady
        ) {
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
        self.rollback.ase_current = None;
        self.rollback.ase_operation = None;
        self.rollback.ase_group_operations = [None, None];
        self.rollback.graceful_ase_release_mode =
            GracefulAseReleaseMode::Skip;
        self.rollback.cleanup_failures.clear();
        self.phase = ControlPhase::RollingBack;
        self.continue_rollback()
    }

    pub(crate) fn begin_teardown_with_release_mode(
        &mut self,
        release_mode: GracefulAseReleaseMode,
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
        self.rollback.ase_group_operations = [None, None];
        self.rollback.graceful_ase_release_mode = release_mode;
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
        self.rollback.ase_group_operations = [None, None];
        self.rollback.graceful_ase_release_mode =
            GracefulAseReleaseMode::Serial;
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
        self.rollback.ase_group_operations = [None, None];
        self.rollback.graceful_ase_release_mode =
            GracefulAseReleaseMode::Serial;

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

        let normal_teardown = self.failure.is_none();

        // Review-only graceful modes may release Streaming ASEs before
        // disturbing the ISO path/CIS. Product Stop selects Skip, while
        // failure rollback keeps its separate fail-closed ordering.
        if normal_teardown {
            match self.rollback.graceful_ase_release_mode {
                GracefulAseReleaseMode::Serial => {
                    if self.rollback.ase_current.is_none() {
                        if let Some(side) = self.rollback.ase_queue.pop() {
                            return self.start_rollback_ase_release(side);
                        }
                    }
                    if self.rollback.ase_current.is_some() {
                        return Ok(Vec::new());
                    }
                }
                GracefulAseReleaseMode::Group => {
                    if !self.rollback.ase_queue.is_empty() {
                        return self.start_graceful_group_ase_releases();
                    }
                    if self.has_group_rollback_ase_operation() {
                        return Ok(Vec::new());
                    }
                }
                GracefulAseReleaseMode::Skip => {}
            }
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

        if !normal_teardown {
            if self.rollback.ase_current.is_none() {
                if let Some(side) = self.rollback.ase_queue.pop() {
                    return self.start_rollback_ase_release(side);
                }
            }
            if self.rollback.ase_current.is_some() {
                return Ok(Vec::new());
            }
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
