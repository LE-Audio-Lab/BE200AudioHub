//! Core timer and local Control event reducer.

use super::*;

impl ControlMachine {
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
                self.rollback.cleanup_failures.push(
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
                && self.discovery_operation.machine
                    .as_ref()
                    .and_then(|value| value.pending_request_id())
                    == Some(request_id) =>
            {
                let connection = self.confirmed.connections[member_index]
                    .expect("audio discovery requires live connection");
                self.discovery_operation.machine = None;
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
                && self.ase_control_operation.prepare
                    .as_ref()
                    .and_then(|value| value.pending_request_id())
                    == Some(request_id) =>
            {
                let side = match self.phase {
                    ControlPhase::PreparingCodecAse { side } => side,
                    _ => unreachable!(),
                };
                let connection = self.planned_sink(side)?.connection;
                self.ase_control_operation.prepare = None;
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
                && self.streaming_operation.gate
                    .as_ref()
                    .and_then(|gate| gate.pending_request_id())
                    == Some(request_id) =>
            {
                let side = match self.phase {
                    ControlPhase::AwaitingStreaming { side } => side,
                    _ => unreachable!(),
                };
                let connection = self.planned_sink(side)?.connection;
                self.streaming_operation.gate = None;
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
                && self.ase_control_operation.control
                    .as_ref()
                    .and_then(|value| value.pending_request_id())
                    == Some(request_id) =>
            {
                let side = self
                    .active_ase_operation_side()
                    .expect("GATT timer requires active ASE operation");
                let connection = self.planned_sink(side)?.connection;
                self.ase_control_operation.control = None;
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
                let pending = self.ase_control_operation.control
                    .as_ref()
                    .and_then(|value| value.pending_request_id());
                self.ase_control_operation.control = None;

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
                && self.streaming_operation.gate
                    .as_ref()
                    .map(|gate| gate.phase())
                    == Some(StreamingGatePhase::WaitingRetry) =>
            {
                let mut gate = self.streaming_operation.gate
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
                self.streaming_operation.gate = Some(gate);
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
                let pending = self.streaming_operation.gate
                    .as_ref()
                    .and_then(|gate| gate.pending_request_id());
                self.streaming_operation.gate = None;
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
                let cig_id = self.planned.iso_plan
                    .as_ref()
                    .expect("ConfiguringCig requires ISO plan")
                    .cig_id;
                // The command may have reached the controller even if the
                // completion is lost. Treat allocation as possible.
                self.cleanup_knowledge.cig_may_exist = true;
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
                let cig_id = self.planned.iso_plan
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
                && self.rollback.stream_route
                    == Some((session_id, stream_id))
                && self.rollback.stream_route_unbind_pending =>
            {
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupStreamRoute {
                            session_id,
                            stream_id,
                        },
                    ),
                );
                self.rollback.stream_route = None;
                self.rollback.stream_route_unbind_pending = false;
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
                && self.rollback.iso_path_current == Some(cis) =>
            {
                self.rollback.cleanup_failures.push(
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
                && self.rollback.cis_current == Some(cis) =>
            {
                self.rollback.cleanup_failures.push(
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
                && self.rollback.ase_current == Some(side) =>
            {
                let gate = self.rollback.ase_operation
                    .as_ref()
                    .expect("CleanupAse timeout requires active ASE operation")
                    .gate_snapshot();
                let pending = gate.pending_request_id;
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::AseReleaseGateTimeout {
                        side,
                        gate,
                    },
                );
                self.rollback.ase_operation = None;
                self.rollback.ase_current = None;

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
                && self.rollback.cig_pending =>
            {
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupCig { cig_id },
                    ),
                );
                self.rollback.cig_pending = false;
                self.cleanup_knowledge.cig_may_exist = false;
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
                && self.rollback.ase_operation
                    .as_ref()
                    .and_then(|machine| machine.pending_request_id())
                    == Some(request_id)
                && self.rollback.ase_current
                    .and_then(|side| {
                        self.planned_sink(side)
                            .ok()
                            .map(|sink| sink.member_index)
                    })
                    == Some(member_index) =>
            {
                let side = self.rollback.ase_current
                    .expect("rollback GATT timer requires ASE side");
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::Gatt {
                            member_index,
                            request_id,
                        },
                    ),
                );
                self.rollback.ase_operation = None;
                self.rollback.ase_current = None;
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
                && self.rollback.acl_current == Some(connection) =>
            {
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::Timeout(
                        ControlTimer::CleanupDisconnect(connection),
                    ),
                );
                self.finish_rollback()
            }
            ControlEvent::TimerExpired(ControlTimer::CleanupPrivacy)
                if self.phase == ControlPhase::RollingBack =>
            {
                self.rollback.cleanup_failures.push(
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
}
