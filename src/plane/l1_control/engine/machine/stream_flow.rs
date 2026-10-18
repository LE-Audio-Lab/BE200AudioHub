//! Stream planning, CIG/CIS/path lifecycle and logical handoff orchestration.

use super::*;

impl ControlMachine {
    /// Continue one internally ready product stage without inventing a new
    /// Core command for protocol-local progression.
    ///
    /// This is composition glue: external waits still stop on their normal
    /// report/timer cursors, while confirmed checkpoints advance into the next
    /// Control-owned operation.
    pub fn continue_ready_stage(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        match self.phase {
            ControlPhase::SetConnected => self.begin_audio_discovery(),
            ControlPhase::AudioSetDiscovered => {
                self.plan_codec()?;
                self.begin_codec_configuration()
            }
            ControlPhase::CodecConfigured => {
                self.plan_iso()?;
                self.begin_cig_configuration()
            }
            ControlPhase::CigConfigured => {
                self.plan_qos()?;
                self.begin_qos_configuration()
            }
            ControlPhase::QosConfigured => {
                self.plan_enable()?;
                self.begin_enable()
            }
            ControlPhase::Enabled => self.begin_cis_creation(),
            ControlPhase::CisEstablished => self.begin_iso_path_setup(),
            ControlPhase::IsoPathsReady => self.begin_streaming_gate(),
            _ => Ok(Vec::new()),
        }
    }

    pub fn begin_stream_handoff(
        &mut self,
        session_id: SessionId,
        stream_id: StreamId,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::StreamingReady {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }

        let codec = self.planned.codec_plan
            .as_ref()
            .expect("StreamingReady requires codec plan");
        let qos = self.planned.qos_plan
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
        self.stream_route_operation.pending_binding = Some(binding);
        self.confirmed.active_stream_binding = None;
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
        self.streaming_operation.gate = None;
        self.confirmed.streaming_statuses = [None, None];
        self.stream_route_operation.pending_binding = None;
        self.confirmed.active_stream_binding = None;
        self.rollback.stream_route = None;
        self.rollback.stream_route_unbind_pending = false;
        self.start_streaming_gate(AudioSide::Left)
    }

    pub fn begin_iso_path_setup(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::CisEstablished {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.confirmed.iso_paths_ready.clear();
        self.cleanup_knowledge.iso_paths_may_exist.clear();
        self.streaming_operation.gate = None;
        self.confirmed.streaming_statuses = [None, None];
        self.start_iso_path_setup(AudioSide::Left)
    }

    pub fn begin_cis_creation(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::Enabled {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let request = self.planned.iso_plan
            .as_ref()
            .expect("Enabled requires ISO plan")
            .cis_create_request();
        self.confirmed.live_cis.clear();
        self.confirmed.iso_paths_ready.clear();
        self.cleanup_knowledge.iso_paths_may_exist.clear();
        self.streaming_operation.gate = None;
        self.confirmed.streaming_statuses = [None, None];
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
        let codec = self.planned.codec_plan
            .as_ref()
            .expect("QosConfigured requires codec plan");
        let iso = self.planned.iso_plan
            .as_ref()
            .expect("QosConfigured requires ISO plan");
        let qos = self.planned.qos_plan
            .as_ref()
            .expect("QosConfigured requires QoS plan");
        let plan = plan_first_enable(codec, iso, qos)
            .map_err(ControlFailure::EnablePlan)?;
        self.planned.enable_plan = Some(plan);
        self.confirmed.enable_statuses = [None, None];
        self.confirmed.live_cis.clear();
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
        let codec = self.planned.codec_plan
            .as_ref()
            .expect("CigConfigured requires codec plan");
        let iso = self.planned.iso_plan
            .as_ref()
            .expect("CigConfigured requires ISO plan");
        let plan = plan_first_qos(codec, iso)
            .map_err(ControlFailure::QosPlan)?;
        self.planned.qos_plan = Some(plan);
        self.confirmed.qos_statuses = [None, None];
        self.planned.enable_plan = None;
        self.confirmed.enable_statuses = [None, None];
        self.confirmed.live_cis.clear();
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
        let codec = self.planned.codec_plan
            .as_ref()
            .expect("CodecConfigured requires codec plan");
        let plan = plan_first_iso(codec)
            .map_err(ControlFailure::IsoPlan)?;
        self.planned.iso_plan = Some(plan);
        self.planned.qos_plan = None;
        self.confirmed.qos_statuses = [None, None];
        self.planned.enable_plan = None;
        self.confirmed.enable_statuses = [None, None];
        self.phase = ControlPhase::IsoPlanned;
        Ok(())
    }

    pub fn begin_cig_configuration(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::IsoPlanned {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let plan = self.planned.iso_plan
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
        self.ase_control_operation.prepare = None;
        self.ase_control_operation.control = None;
        self.confirmed.prepared_ases = [None, None];
        self.confirmed.codec_statuses = [None, None];
        self.start_ase_prepare(AudioSide::Left)
    }

    pub fn plan_codec(
        &mut self,
    ) -> Result<(), ControlFailure> {
        if self.phase != ControlPhase::AudioSetDiscovered {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        let set = self.confirmed.audio_set
            .as_ref()
            .expect("AudioSetDiscovered requires snapshot");
        let plan = plan_first_stream(set)
            .map_err(ControlFailure::CodecPlan)?;
        self.planned.codec_plan = Some(plan);
        self.planned.iso_plan = None;
        self.planned.qos_plan = None;
        self.confirmed.qos_statuses = [None, None];
        self.planned.enable_plan = None;
        self.confirmed.enable_statuses = [None, None];
        self.cleanup_knowledge.cig_may_exist = false;
        self.rollback.cig_pending = false;
        self.ase_control_operation.prepare = None;
        self.ase_control_operation.control = None;
        self.confirmed.prepared_ases = [None, None];
        self.confirmed.codec_statuses = [None, None];
        self.phase = ControlPhase::CodecPlanned;
        Ok(())
    }

    pub fn begin_audio_discovery(
        &mut self,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::SetConnected {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.discovery_operation.members.clear();
        self.confirmed.audio_set = None;
        self.planned.codec_plan = None;
        self.planned.iso_plan = None;
        self.cleanup_knowledge.cig_may_exist = false;
        self.rollback.cig_pending = false;
        self.ase_control_operation.prepare = None;
        self.ase_control_operation.control = None;
        self.confirmed.prepared_ases = [None, None];
        self.confirmed.codec_statuses = [None, None];
        self.start_audio_member_discovery(0)
    }

    pub(super) fn cis_for_side(
        &self,
        side: AudioSide,
    ) -> Option<CisRef> {
        let plan = self.planned.iso_plan.as_ref()?;
        Some(CisRef {
            cig_id: plan.cig_id,
            cis_id: plan.cis_id(side),
        })
    }

    pub(super) fn start_iso_path_setup(
        &mut self,
        side: AudioSide,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let cis = self
            .cis_for_side(side)
            .ok_or(ControlFailure::InvalidStartState(self.phase))?;
        if !self.confirmed.live_cis.contains(&cis) {
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

    pub(super) fn semantic_cis_ref(
        &self,
        cig_id: u8,
        cis_id: u8,
    ) -> Result<CisRef, ControlFailure> {
        let plan = self.planned.iso_plan
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

    pub(super) fn observe_semantic_cis_set(
        &mut self,
        cig_id: u8,
        cis_ids: &[u8],
    ) -> Result<(), ControlFailure> {
        let mut next = BTreeSet::new();
        for cis_id in cis_ids {
            next.insert(self.semantic_cis_ref(cig_id, *cis_id)?);
        }
        self.confirmed.live_cis = next;
        Ok(())
    }

    pub(super) fn semantic_cis_set_complete(
        &self,
        cig_id: u8,
        cis_ids: &[u8],
    ) -> bool {
        let Some(plan) = self.planned.iso_plan.as_ref() else {
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
}
