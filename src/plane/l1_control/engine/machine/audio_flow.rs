//! Audio discovery, ASCS and streaming semantic flow for ControlMachine.
//!
//! The detailed local protocol FSMs stay under discovery/ and ascs/. This
//! module contains only Control-level orchestration and verification glue.

use super::*;

impl ControlMachine {
    pub(super) fn start_streaming_gate(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let cis = self
            .cis_for_side(side)
            .ok_or(ControlFailure::InvalidStartState(self.phase))?;
        if !self.confirmed.iso_paths_ready.contains(&cis) {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let prepared = self.confirmed.prepared_ases[side_index(side)]
            .as_ref()
            .expect("streaming gate requires prepared ASE")
            .clone();
        let context = self.planned.enable_plan
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
        self.streaming_operation.gate = Some(gate);
        self.phase = ControlPhase::AwaitingStreaming { side };

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::StreamingDeadline { side },
        )];
        actions.extend(
            self.translate_streaming_action(side, None, action)?,
        );
        Ok(actions)
    }

    pub(super) fn on_streaming_gatt_report(
        &mut self,
        side: AudioSide,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut gate = self.streaming_operation.gate
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
                self.streaming_operation.gate = Some(gate);
                Ok(Vec::new())
            }
            Ok(action @ StreamingGateAction::BeginGatt(_))
            | Ok(action @ StreamingGateAction::RetryLater) => {
                self.streaming_operation.gate = Some(gate);
                self.translate_streaming_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Ok(action @ StreamingGateAction::Complete(_)) => {
                self.streaming_operation.gate = None;
                self.translate_streaming_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Err(failure) => {
                self.streaming_operation.gate = None;
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

    pub(super) fn translate_streaming_action(
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
                self.confirmed.streaming_statuses[side_index(side)] =
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

    pub(super) fn start_ase_prepare(
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
        self.ase_control_operation.prepare = Some(machine);
        self.phase = ControlPhase::PreparingCodecAse { side };
        self.translate_ase_prepare_action(side, None, action)
    }

    pub(super) fn start_config_codec(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.confirmed.prepared_ases[side_index(side)]
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
        self.ase_control_operation.control = Some(machine);
        self.phase = ControlPhase::ConfiguringCodec { side };

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::AseOperation { side },
        )];
        actions.extend(
            self.translate_ase_operation_action(side, None, action)?,
        );
        Ok(actions)
    }

    pub(super) fn start_config_qos(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.confirmed.prepared_ases[side_index(side)]
            .as_ref()
            .expect("QoS requires prepared ASE control")
            .clone();
        let planned = match side {
            AudioSide::Left => &self.planned.qos_plan
                .as_ref()
                .expect("QosPlanned requires QoS plan")
                .left,
            AudioSide::Right => &self.planned.qos_plan
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
        self.ase_control_operation.control = Some(machine);
        self.phase = ControlPhase::ConfiguringQos { side };

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::AseOperation { side },
        )];
        actions.extend(
            self.translate_ase_operation_action(side, None, action)?,
        );
        Ok(actions)
    }

    pub(super) fn start_enable(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let prepared = self.confirmed.prepared_ases[side_index(side)]
            .as_ref()
            .expect("Enable requires prepared ASE control")
            .clone();
        let planned = match side {
            AudioSide::Left => &self.planned.enable_plan
                .as_ref()
                .expect("EnablePlanned requires Enable plan")
                .left,
            AudioSide::Right => &self.planned.enable_plan
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
        self.ase_control_operation.control = Some(machine);
        self.phase = ControlPhase::Enabling { side };

        let mut actions = vec![ControlAction::ArmTimer(
            ControlTimer::AseOperation { side },
        )];
        actions.extend(
            self.translate_ase_operation_action(side, None, action)?,
        );
        Ok(actions)
    }

    pub(super) fn on_ase_prepare_gatt_report(
        &mut self,
        side: AudioSide,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut machine = self.ase_control_operation.prepare
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
                self.ase_control_operation.prepare = Some(machine);
                Ok(Vec::new())
            }
            Ok(action) => {
                self.ase_control_operation.prepare = if matches!(
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
                self.ase_control_operation.prepare = None;
                let mut actions =
                    self.disarm_gatt_request(side, completed_request)?;
                actions.extend(self.begin_rollback(
                    ControlFailure::AsePrepare { side, failure },
                )?);
                Ok(actions)
            }
        }
    }

    pub(super) fn translate_ase_prepare_action(
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
                self.confirmed.prepared_ases[side_index(side)] = Some(prepared);
                actions.extend(self.start_config_codec(side)?);
                Ok(actions)
            }
            AsePrepareAction::Ignored => Ok(actions),
        }
    }

    pub(super) fn on_ase_operation_gatt_report(
        &mut self,
        side: AudioSide,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut machine = self.ase_control_operation.control
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
                self.ase_control_operation.control = Some(machine);
                Ok(Vec::new())
            }
            Ok(AseOperationAction::Await) => {
                self.ase_control_operation.control = Some(machine);
                self.disarm_gatt_request(side, completed_request)
            }
            Ok(action @ AseOperationAction::BeginGatt(_)) => {
                self.ase_control_operation.control = Some(machine);
                self.translate_ase_operation_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Ok(action @ AseOperationAction::Complete(_)) => {
                self.ase_control_operation.control = None;
                self.translate_ase_operation_action(
                    side,
                    completed_request,
                    action,
                )
            }
            Err(failure) => {
                self.ase_control_operation.control = None;
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

    pub(super) fn translate_ase_operation_action(
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

                        if let AseStatusDetails::CodecConfigured {
                            preferred_framing,
                            preferred_phy,
                            preferred_retransmission_number,
                            preferred_max_transport_latency,
                            supported_presentation_delay_min,
                            supported_presentation_delay_max,
                            preferred_presentation_delay_min,
                            preferred_presentation_delay_max,
                            ..
                        } = &status.details
                        {
                            println!(
                                "[le-audio][peer-qos] side={:?} ase_id={} preferred_framing={} preferred_phy={} preferred_rtn={} preferred_max_transport_latency_ms={} supported_pd_us={}..{} preferred_pd_us={}..{}",
                                side,
                                status.ase_id,
                                preferred_framing,
                                preferred_phy,
                                preferred_retransmission_number,
                                preferred_max_transport_latency,
                                supported_presentation_delay_min,
                                supported_presentation_delay_max,
                                preferred_presentation_delay_min,
                                preferred_presentation_delay_max,
                            );
                        }

                        self.confirmed.codec_statuses[side_index(side)] =
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

                        self.confirmed.qos_statuses[side_index(side)] =
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

                        self.confirmed.enable_statuses[side_index(side)] =
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

    pub(super) fn verify_codec_readback(
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

    pub(super) fn verify_qos_readback(
        &self,
        side: AudioSide,
        status: &AseStatus,
    ) -> Result<(), String> {
        let plan = self.planned.qos_plan
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

    pub(super) fn verify_enable_readback(
        &self,
        side: AudioSide,
        status: &AseStatus,
    ) -> Result<(), String> {
        use bumble_profiles::le_audio::MetadataTag;

        let plan = self.planned.enable_plan
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

    pub(super) fn active_ase_operation_side(&self) -> Option<AudioSide> {
        match self.phase {
            ControlPhase::ConfiguringCodec { side }
            | ControlPhase::ConfiguringQos { side }
            | ControlPhase::Enabling { side } => Some(side),
            _ => None,
        }
    }

    pub(super) fn disarm_gatt_request(
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

    pub(super) fn planned_sink(
        &self,
        side: AudioSide,
    ) -> Result<&crate::plane::l1_control::planning::codec::PlannedSink, ControlFailure> {
        let plan = self.planned.codec_plan.as_ref().ok_or(
            ControlFailure::InvalidStartState(self.phase),
        )?;
        Ok(match side {
            AudioSide::Left => &plan.left,
            AudioSide::Right => &plan.right,
        })
    }

    pub(super) fn start_audio_member_discovery(
        &mut self,
        member_index: usize,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let connection = self.confirmed.connections[member_index]
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
        self.discovery_operation.machine = Some(discovery);
        self.phase = ControlPhase::DiscoveringAudioMember {
            member_index,
        };
        self.translate_audio_action(member_index, None, action)
    }

    pub(super) fn on_audio_gatt_report(
        &mut self,
        member_index: usize,
        report: &GattReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let mut discovery = self.discovery_operation.machine
            .take()
            .expect("audio discovery phase missing member FSM");
        let pending = discovery.pending_request_id();
        let outcome = discovery.on_gatt_report(report);
        self.next_gatt_request_id = discovery.next_request_id();

        match outcome {
            Ok(AudioDiscoveryAction::Ignored) => {
                self.discovery_operation.machine = Some(discovery);
                Ok(Vec::new())
            }
            Ok(action) => {
                self.discovery_operation.machine = if matches!(
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
                self.discovery_operation.machine = None;
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

    pub(super) fn translate_audio_action(
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
                self.discovery_operation.members.push(snapshot);
                if member_index + 1 < self.plan.members.len() {
                    actions.extend(
                        self.start_audio_member_discovery(
                            member_index + 1,
                        )?,
                    );
                    return Ok(actions);
                }

                let set = AudioSetSnapshot {
                    members: std::mem::take(&mut self.discovery_operation.members),
                };
                if let Err(error) = set.validate_dual_sink() {
                    actions.extend(self.begin_rollback(
                        ControlFailure::AudioSet(error),
                    )?);
                    return Ok(actions);
                }
                self.confirmed.audio_set = Some(set);
                self.phase = ControlPhase::AudioSetDiscovered;
                Ok(actions)
            }
            AudioDiscoveryAction::Ignored => Ok(actions),
        }
    }
}
