//! Reliable lower semantic report reducer for ControlMachine.
//!
//! This is a relocation of the proven HCI-report transition table. Domain
//! helpers remain split across connection/audio/stream/rollback flows.

use super::*;

impl ControlMachine {
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

                self.confirmed.resolved_members = resolved;
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
                self.confirmed.privacy_active = true;
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
                self.confirmed.connections[member_index] = Some(token);
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
                let expected = self.confirmed.connections[member_index]
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
                self.confirmed.encryption_key_sizes[member_index] =
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
                self.confirmed.connections[member_index] =
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
                let plan = self.planned.iso_plan
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
                    self.cleanup_knowledge.cig_may_exist = true;
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

                self.cleanup_knowledge.cig_may_exist = true;
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
                self.cleanup_knowledge.cig_may_exist = failure.may_have_allocated;
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
                    let plan = self.planned.iso_plan
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

                self.confirmed.iso_paths_ready.insert(*cis);
                self.cleanup_knowledge.iso_paths_may_exist.insert(*cis);
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
                    self.cleanup_knowledge.iso_paths_may_exist.insert(failure.cis);
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
                let binding = self.stream_route_operation.pending_binding
                    .take()
                    .expect("binding phase requires pending StreamBinding");
                self.confirmed.active_stream_binding = Some(binding);
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
            ) if self.rollback.stream_route
                == Some((*session_id, *stream_id))
                && self.rollback.stream_route_unbind_pending =>
            {
                self.rollback.stream_route = None;
                self.rollback.stream_route_unbind_pending = false;
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
            ) if self.rollback.iso_path_current == Some(*cis) => {
                self.confirmed.iso_paths_ready.remove(cis);
                self.cleanup_knowledge.iso_paths_may_exist.remove(cis);
                self.rollback.iso_path_current = None;
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
            ) if self.rollback.iso_path_current == Some(failure.cis) =>
            {
                self.rollback.cleanup_failures.push(
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
            ) if self.rollback.cis_current == Some(*cis) => {
                self.confirmed.live_cis.remove(cis);
                self.rollback.cis_current = None;
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
            ) if self.rollback.cis_current == Some(*cis) => {
                self.rollback.cleanup_failures.push(
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
            ) if self.rollback.cig_pending
                && self.planned.iso_plan
                    .as_ref()
                    .map(|plan| plan.cig_id)
                    == Some(*cig_id) =>
            {
                self.rollback.cig_pending = false;
                self.cleanup_knowledge.cig_may_exist = false;
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
            ) if self.rollback.cig_pending =>
            {
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::CigRemove(
                        failure.clone(),
                    ),
                );
                self.rollback.cig_pending = false;
                self.cleanup_knowledge.cig_may_exist = false;
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
            ) if self.rollback.acl_current == Some(*connection) => {
                self.remove_known_connection(*connection);
                self.rollback.acl_current = None;
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
            ) if self.rollback.acl_current == Some(failure.connection) => {
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::Disconnect(
                        failure.clone(),
                    ),
                );
                self.rollback.acl_current = None;
                self.rollback.acl_queue.clear();
                let mut actions = vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::CleanupDisconnect(
                            failure.connection,
                        ),
                    ),
                ];
                if self.confirmed.privacy_active {
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
            ) if self.rollback.acl_current.is_none()
                && self.rollback.acl_queue.is_empty()
                && self.confirmed.privacy_active =>
            {
                self.confirmed.privacy_active = false;
                self.finish_rollback()
            }
            (
                ControlPhase::RollingBack,
                HciReport::ControllerPrivacyState(
                    ControllerPrivacyState::Failed(failure),
                ),
            ) if self.rollback.acl_current.is_none()
                && self.rollback.acl_queue.is_empty() =>
            {
                self.rollback.cleanup_failures.push(
                    ControlCleanupFailure::Privacy(failure.clone()),
                );
                self.confirmed.privacy_active = false;
                self.finish_rollback()
            }

            (
                ControlPhase::AwaitingStreaming { side },
                HciReport::Gatt(report),
            ) => self.on_streaming_gatt_report(side, report),
            (
                ControlPhase::RollingBack,
                HciReport::Gatt(report),
            ) if self.rollback.ase_operation.is_some() => {
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
}
