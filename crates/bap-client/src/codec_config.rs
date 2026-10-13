//! R4.2: ASCS Config Codec transactions on already encrypted, bonded ACLs.
//!
//! Reuses Bumble-RS's ASCS client, ATT/GATT stack, response and ASE parsers.
//! Every write has a matched Control Point response AND an ASE state gate.
//! The hardware smoke test RELEASES every attempted ASE back to Idle before
//! terminating ACLs. No SMP, pairing store, CIG, CIS, QoS or ISO side effects.

use std::time::{Duration, Instant};

use bumble_adapter::{AttachedController, AttValueEvent};
use bumble_profiles::ascs::{
    AseControlResponse, AseEvent, AseOpcode, AseOperation, AseResponseCode,
    AseState, AseStatus, AseStatusDetails, AudioStreamControlServiceProxy,
};
use bumble_profiles::bap::CodecSpecificConfiguration;
use bumble_transport::Error;

use crate::codec_plan::{CodecConfigurationPlan, PlannedSink, StereoSide};

const GATT_TIMEOUT: Duration = Duration::from_secs(12);
const CP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(8);
const REQUESTED_MTU: u16 = 247;
const TARGET_LATENCY_BALANCED: u8 = 0x02;
const TARGET_PHY_1M: u8 = 0x01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecConfigScope {
    Left,
    Right,
    Both,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecConfiguredMember {
    pub side: StereoSide,
    pub identity_address: String,
    pub ase_id: u8,
    pub preferred_framing: u8,
    pub preferred_phy: u8,
    pub preferred_retransmission_number: u8,
    pub preferred_max_transport_latency: u16,
    pub supported_presentation_delay_min: u32,
    pub supported_presentation_delay_max: u32,
    pub preferred_presentation_delay_min: u32,
    pub preferred_presentation_delay_max: u32,
    pub codec_specific_configuration: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecConfiguredSetSnapshot {
    /// Captured before release; ACL handles are not retained after this call.
    pub members: Vec<CodecConfiguredMember>,
    pub all_released_to_idle: bool,
}

struct PreparedSink<'a> {
    sink: &'a PlannedSink,
    proxy: AudioStreamControlServiceProxy,
}

#[derive(Debug)]
struct ResponseGate {
    expected_opcode: AseOpcode,
    ase_id: u8,
    expected_state: AseState,
    control_response_seen: bool,
    final_status: Option<AseStatus>,
}

impl ResponseGate {
    fn new(expected_opcode: AseOpcode, ase_id: u8, expected_state: AseState) -> Self {
        Self {
            expected_opcode,
            ase_id,
            expected_state,
            control_response_seen: false,
            final_status: None,
        }
    }

    fn accept(&mut self, event: AseEvent) -> Result<(), String> {
        match event {
            AseEvent::ControlPoint(response) => self.accept_control_point(response),
            AseEvent::Status(status) => {
                if status.ase_id == self.ase_id {
                    if status.state == self.expected_state {
                        self.final_status = Some(status);
                    } else if status.state != AseState::RELEASING {
                        return Err(format!(
                            "ASE {} unexpectedly entered state 0x{:02X} while awaiting 0x{:02X}",
                            self.ase_id, status.state.0, self.expected_state.0
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    fn accept_control_point(&mut self, response: AseControlResponse) -> Result<(), String> {
        if response.opcode != self.expected_opcode {
            return Err(format!(
                "unexpected ASCS Control Point opcode {:?}; expected {:?}",
                response.opcode, self.expected_opcode
            ));
        }
        if self.control_response_seen {
            return Err("duplicate ASCS Control Point response".into());
        }
        if response.responses.len() != 1 || response.responses[0].ase_id != self.ase_id {
            return Err(format!(
                "ASCS CP response did not address exactly ASE {}: {:?}",
                self.ase_id, response.responses
            ));
        }
        let result = &response.responses[0];
        if result.code != AseResponseCode::SUCCESS {
            return Err(format!(
                "ASCS {:?} rejected ASE {}: response=0x{:02X} reason=0x{:02X}",
                response.opcode, result.ase_id, result.code.0, result.reason.0
            ));
        }
        self.control_response_seen = true;
        Ok(())
    }

    fn complete(&self) -> bool {
        self.control_response_seen && self.final_status.is_some()
    }
}

fn observe_events(
    prepared: &PreparedSink<'_>,
    gate: &mut ResponseGate,
    events: Vec<AttValueEvent>,
) -> Result<(), String> {
    for event in events {
        if event.connection_handle != prepared.sink.acl_handle {
            return Err(format!(
                "ATT event escaped ACL isolation: got 0x{:04X}, expected 0x{:04X}",
                event.connection_handle, prepared.sink.acl_handle
            ));
        }
        let is_our_handle = event.attribute_handle == prepared.proxy.ase_control_point.handle
            || event.attribute_handle == prepared.sink.ase_value_handle;
        if is_our_handle {
            let parsed = prepared.proxy
                .event_from_notification(event.attribute_handle, &event.value)
                .map_err(|error| format!("invalid ASCS notification: {error}"))?;
            gate.accept(parsed)?;
        } else if !prepared.proxy.sink_ase.iter()
            .chain(prepared.proxy.source_ase.iter())
            .any(|ase| ase.handle == event.attribute_handle)
        {
            return Err(format!(
                "unrecognized ATT notification handle=0x{:04X} on ACL=0x{:04X}",
                event.attribute_handle, event.connection_handle
            ));
        }
        // ASCS notifications from non-selected ASEs are not state gates.
    }
    Ok(())
}

fn read_sink_status(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
) -> Result<AseStatus, String> {
    let handle = prepared.sink.acl_handle;
    controller.with_encrypted_gatt(
        handle, GATT_TIMEOUT, |client, transport| {
            let characteristic = prepared.proxy
                .sink_ase.iter()
                .find(|ase| ase.handle == prepared.sink.ase_value_handle)
                .ok_or_else(|| Error::Remote("selected Sink ASE missing from ASCS proxy".into()))?;
            AudioStreamControlServiceProxy::read_ase(characteristic, client, transport)
                .map_err(|error| Error::Remote(format!("ASE status read failed: {error}")))
        }
    )
    .map_err(|error| format!(
        "{} ASE {} status read failed: {error}",
        prepared.sink.side.as_str(), prepared.sink.ase_id
    ))
}

fn prepare_sink<'a>(
    controller: &mut AttachedController,
    sink: &'a PlannedSink,
) -> Result<PreparedSink<'a>, String> {
    let opcode = AseOperation::ConfigCodec(vec![
        sink.config_codec_parameters(TARGET_LATENCY_BALANCED, TARGET_PHY_1M)
    ]);
    let required_mtu = u16::try_from(opcode.to_bytes().map_err(|e| e.to_string())?.len() + 3)
        .map_err(|_| "ASCS Config Codec payload exceeds ATT MTU range".to_owned())?;

    let (proxy, buffered) = controller.with_encrypted_gatt_events(
        sink.acl_handle, GATT_TIMEOUT, |client, transport| {
            let mtu = client.exchange_mtu(transport, REQUESTED_MTU)
                .map_err(|error| Error::Remote(format!("ATT MTU exchange failed: {error}")))?;
            if mtu < required_mtu {
                return Err(Error::Remote(format!(
                    "ATT MTU {} cannot carry Config Codec write (needs {})",
                    mtu, required_mtu
                )));
            }
            let proxy = AudioStreamControlServiceProxy::discover(client, transport)
                .map_err(|error| Error::Remote(format!("ASCS re-discovery failed: {error}")))?
                .ok_or_else(|| Error::Remote("peer ASCS disappeared after R3 discovery".into()))?;
            if proxy.ase_control_point.handle != sink.ascs_control_point_handle
                || !proxy.sink_ase.iter().any(|ase| ase.handle == sink.ase_value_handle)
            {
                return Err(Error::Remote(
                    "ASCS attribute handles changed since R3 discovery".into()
                ));
            }
            // Install both the CP and ASE CCCDs before issuing *any* writes.
            proxy.subscribe_all(client, transport)
                .map_err(|error| Error::Remote(format!("ASCS CCCD subscription failed: {error}")))?;
            let selected_ase = proxy.sink_ase.iter()
                .find(|ase| ase.handle == sink.ase_value_handle)
                .expect("selected ASE handle verified");
            let current = AudioStreamControlServiceProxy::read_ase(
                selected_ase, client, transport
            ).map_err(|error| Error::Remote(format!("pre-configuration ASE read failed: {error}")))?;
            if current.ase_id != sink.ase_id || current.state != AseState::IDLE {
                return Err(Error::Remote(format!(
                    "expected ASE {} Idle before configuration, got ASE {} state=0x{:02X}",
                    sink.ase_id, current.ase_id, current.state.0
                )));
            }
            Ok(proxy)
        }
    )
    .map_err(|error| format!("{} ASCS prepare failed: {error}", sink.side.as_str()))?;

    let prepared = PreparedSink { sink, proxy };
    // Reject stale CP completions: otherwise a previous run might satisfy the
    // new command's response gate.
    for event in buffered.into_iter()
        .chain(controller.drain_att_value_events(&[sink.acl_handle])
            .map_err(|error| format!("ATT pre-write drain failed: {error}"))?)
    {
        if event.attribute_handle == prepared.proxy.ase_control_point.handle {
            return Err(format!(
                "{} has an uncorrelated ASCS CP response before configuration",
                sink.side.as_str()
            ));
        }
    }
    println!(
        "[R4.2/PREP] {} ASE={} ACL=0x{:04X}: MTU/CCCD/Idle PASS",
        sink.side.as_str(), sink.ase_id, sink.acl_handle
    );
    Ok(prepared)
}

fn perform_operation(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
    operation: AseOperation,
    expected_state: AseState,
    deadline_window: Duration,
) -> Result<AseStatus, String> {
    let opcode = operation.opcode();
    let handle = prepared.sink.acl_handle;
    // Subscriptions survive the temporary GattClient: CCCDs are peer-side.
    // These writes are the only ASCS mutation boundary in this module.
    let ((), initial_events) = controller.with_encrypted_gatt_events(
        handle, GATT_TIMEOUT, |client, transport| {
            prepared.proxy.write_operation(client, transport, &operation)
                .map_err(|error| Error::Remote(format!("ASCS write failed: {error}")))
        }
    )
    .map_err(|error| format!(
        "{} ASCS {:?} transport/write failure: {error}",
        prepared.sink.side.as_str(), opcode
    ))?;

    let mut gate = ResponseGate::new(opcode, prepared.sink.ase_id, expected_state);
    observe_events(prepared, &mut gate, initial_events)?;
    let deadline = Instant::now() + deadline_window;
    while !gate.complete() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "{} ASCS {:?} timeout: CP response={} final-state notification={}",
                prepared.sink.side.as_str(), opcode,
                gate.control_response_seen, gate.final_status.is_some()
            ));
        }
        let events = controller.wait_for_att_value_events(&[handle], remaining)
            .map_err(|error| format!(
                "{} ASCS {:?} event wait failed (CP={}, status={}): {error}",
                prepared.sink.side.as_str(), opcode,
                gate.control_response_seen, gate.final_status.is_some()
            ))?;
        observe_events(prepared, &mut gate, events)?;
    }
    let final_status = read_sink_status(controller, prepared)?;
    if final_status.ase_id != prepared.sink.ase_id || final_status.state != expected_state {
        return Err(format!(
            "{} ASCS {:?} readback mismatch: ASE={} state=0x{:02X}, expected ASE={} state=0x{:02X}",
            prepared.sink.side.as_str(), opcode,
            final_status.ase_id, final_status.state.0,
            prepared.sink.ase_id, expected_state.0
        ));
    }
    Ok(final_status)
}

fn configure_sink(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
) -> Result<CodecConfiguredMember, String> {
    let status = perform_operation(
        controller,
        prepared,
        AseOperation::ConfigCodec(vec![
            prepared.sink.config_codec_parameters(TARGET_LATENCY_BALANCED, TARGET_PHY_1M)
        ]),
        AseState::CODEC_CONFIGURED,
        CP_RESPONSE_TIMEOUT,
    )?;
    let AseStatusDetails::CodecConfigured {
        preferred_framing,
        preferred_phy,
        preferred_retransmission_number,
        preferred_max_transport_latency,
        supported_presentation_delay_min,
        supported_presentation_delay_max,
        preferred_presentation_delay_min,
        preferred_presentation_delay_max,
        codec_id,
        codec_specific_configuration,
    } = status.details else {
        return Err("ASE Codec Configured state lacks QoS preferences".into());
    };
    if codec_id != prepared.sink.codec_id {
        return Err(format!(
            "{} Codec Configured returned a different codec ID: {:?}",
            prepared.sink.side.as_str(), codec_id
        ));
    }
    let reported = CodecSpecificConfiguration::from_bytes(&codec_specific_configuration)
        .map_err(|error| format!(
            "{} Codec Configured returned malformed LC3 LTV: {error}",
            prepared.sink.side.as_str()
        ))?;
    let requested = &prepared.sink.codec_specific_configuration;
    if reported.sampling_frequency != requested.sampling_frequency
        || reported.frame_duration != requested.frame_duration
        || reported.octets_per_codec_frame != requested.octets_per_codec_frame
        || reported.audio_channel_allocation != requested.audio_channel_allocation
    {
        return Err(format!(
            "{} Codec Configured LC3 configuration does not match requested frequency/duration/octets/location",
            prepared.sink.side.as_str()
        ));
    }
    println!(
        "[R4.2/CONFIG] {} ASE={} CodecConfigured PASS (CP + notify + readback)",
        prepared.sink.side.as_str(), prepared.sink.ase_id
    );
    Ok(CodecConfiguredMember {
        side: prepared.sink.side,
        identity_address: prepared.sink.identity_address.clone(),
        ase_id: prepared.sink.ase_id,
        preferred_framing,
        preferred_phy,
        preferred_retransmission_number,
        preferred_max_transport_latency,
        supported_presentation_delay_min,
        supported_presentation_delay_max,
        preferred_presentation_delay_min,
        preferred_presentation_delay_max,
        codec_specific_configuration,
    })
}

fn release_if_needed(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
) -> Result<(), String> {
    let state = read_sink_status(controller, prepared)?;
    match state.state {
        AseState::IDLE => {
            println!(
                "[R4.2/RELEASE] {} ASE={}: already Idle",
                prepared.sink.side.as_str(), prepared.sink.ase_id
            );
            Ok(())
        }
        AseState::CODEC_CONFIGURED | AseState::QOS_CONFIGURED => {
            perform_operation(
                controller,
                prepared,
                AseOperation::Release(vec![prepared.sink.ase_id]),
                AseState::IDLE,
                RELEASE_TIMEOUT,
            )?;
            println!(
                "[R4.2/RELEASE] {} ASE={}: Idle PASS (CP + notify + readback)",
                prepared.sink.side.as_str(), prepared.sink.ase_id
            );
            Ok(())
        }
        _ => Err(format!(
            "{} ASE={} cannot safely release from unexpected state=0x{:02X}; leaving ACL cleanup to caller",
            prepared.sink.side.as_str(), prepared.sink.ase_id, state.state.0
        )),
    }
}

/// Configure one or two Sink ASEs, then Release every attempted ASE to Idle.
///
/// All selected members must prepare (MTU, CCCDs and Idle validation) before
/// the first Config Codec command. On failure we inspect/release *each* ASE
/// whose write may have started, including the failing ear.
pub fn configure_and_release(
    controller: &mut AttachedController,
    plan: &CodecConfigurationPlan,
    scope: CodecConfigScope,
) -> Result<CodecConfiguredSetSnapshot, String> {
    let selection = match scope {
        CodecConfigScope::Left => vec![&plan.left],
        CodecConfigScope::Right => vec![&plan.right],
        CodecConfigScope::Both => vec![&plan.left, &plan.right],
    };
    if selection.len() == 2 && selection[0].acl_handle == selection[1].acl_handle {
        return Err("stereo Config Codec requires two distinct ACL handles".into());
    }
    let mut prepared = Vec::with_capacity(selection.len());
    for sink in selection {
        prepared.push(prepare_sink(controller, sink)?);
    }
    println!("[R4.2/TXN] all selected ASE subscriptions ready; starting Config Codec");

    let mut attempted = 0usize;
    let mut configured = Vec::with_capacity(prepared.len());
    let mut failure = None;
    for sink in &prepared {
        attempted += 1; // Includes an uncertain write outcome for rollback.
        match configure_sink(controller, sink) {
            Ok(member) => configured.push(member),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }

    let mut cleanup_errors = Vec::new();
    for sink in prepared[..attempted].iter().rev() {
        if let Err(error) = release_if_needed(controller, sink) {
            cleanup_errors.push(error);
        }
    }
    if let Some(error) = failure {
        return Err(if cleanup_errors.is_empty() {
            format!("R4.2 Config Codec failed and rollback confirmed: {error}")
        } else {
            format!(
                "R4.2 Config Codec failed: {error}; rollback UNCONFIRMED: {}",
                cleanup_errors.join("; ")
            )
        });
    }
    if !cleanup_errors.is_empty() {
        return Err(format!(
            "R4.2 configured successfully but Release/Idle cleanup FAILED: {}",
            cleanup_errors.join("; ")
        ));
    }

    Ok(CodecConfiguredSetSnapshot {
        members: configured,
        all_released_to_idle: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bumble_profiles::ascs::{
        AseReasonCode, AseResponse,
    };

    fn response(opcode: AseOpcode, code: AseResponseCode) -> AseEvent {
        AseEvent::ControlPoint(AseControlResponse {
            opcode,
            responses: vec![AseResponse {
                ase_id: 1,
                code,
                reason: AseReasonCode::NONE,
            }],
        })
    }

    fn state(state: AseState) -> AseEvent {
        AseEvent::Status(AseStatus {
            ase_id: 1,
            state,
            details: AseStatusDetails::None,
        })
    }

    #[test]
    fn cp_then_state_completes_gate() {
        let mut gate = ResponseGate::new(AseOpcode::ConfigCodec, 1, AseState::CODEC_CONFIGURED);
        gate.accept(response(AseOpcode::ConfigCodec, AseResponseCode::SUCCESS)).unwrap();
        assert!(!gate.complete());
        gate.accept(state(AseState::CODEC_CONFIGURED)).unwrap();
        assert!(gate.complete());
    }

    #[test]
    fn status_before_cp_completes_gate() {
        let mut gate = ResponseGate::new(AseOpcode::ConfigCodec, 1, AseState::CODEC_CONFIGURED);
        gate.accept(state(AseState::CODEC_CONFIGURED)).unwrap();
        assert!(!gate.complete());
        gate.accept(response(AseOpcode::ConfigCodec, AseResponseCode::SUCCESS)).unwrap();
        assert!(gate.complete());
    }

    #[test]
    fn rejects_wrong_opcode_or_negative_response() {
        let mut gate = ResponseGate::new(AseOpcode::ConfigCodec, 1, AseState::CODEC_CONFIGURED);
        assert!(gate.accept(response(AseOpcode::Release, AseResponseCode::SUCCESS)).is_err());
        assert!(gate.accept(response(
            AseOpcode::ConfigCodec, AseResponseCode::UNSUPPORTED_AUDIO_CAPABILITIES
        )).is_err());
    }

    #[test]
    fn release_gate_accepts_transient_releasing_then_idle() {
        let mut gate = ResponseGate::new(AseOpcode::Release, 1, AseState::IDLE);
        gate.accept(state(AseState::RELEASING)).unwrap();
        gate.accept(response(AseOpcode::Release, AseResponseCode::SUCCESS)).unwrap();
        assert!(!gate.complete());
        gate.accept(state(AseState::IDLE)).unwrap();
        assert!(gate.complete());
    }

    #[test]
    fn rejects_wrong_state_and_duplicate_cp() {
        let mut gate = ResponseGate::new(AseOpcode::ConfigCodec, 1, AseState::CODEC_CONFIGURED);
        assert!(gate.accept(state(AseState::QOS_CONFIGURED)).is_err());
        gate.accept(response(AseOpcode::ConfigCodec, AseResponseCode::SUCCESS)).unwrap();
        assert!(gate.accept(response(AseOpcode::ConfigCodec, AseResponseCode::SUCCESS)).is_err());
    }
}
