//! R4.2: ASCS Config Codec transactions on already encrypted, bonded ACLs.
//!
//! Reuses Bumble-RS's ASCS client, ATT/GATT stack, response and ASE parsers.
//! Every write has a matched Control Point response AND an ASE state gate.
//! The hardware smoke test RELEASES every attempted ASE back to Idle before
//! terminating ACLs. No SMP, pairing store, CIG, CIS, QoS or ISO side effects.

use std::time::{Duration, Instant};

use bumble_adapter::{AttachedController, AttValueEvent, CigAllocation, IsoControllerCapabilities};
use bumble_profiles::ascs::{
    AseControlResponse, AseEvent, AseOpcode, AseOperation, AseResponseCode,
    AseQosConfiguration, AseState, AseStatus, AseStatusDetails,
    AudioStreamControlServiceProxy, ConfigQosParameters,
};
use bumble_profiles::bap::CodecSpecificConfiguration;
use bumble_transport::Error;

use crate::ase_read::{parse_ase_status_compat, read_ase_checked};
use crate::codec_plan::{CodecConfigurationPlan, PlannedSink, StereoSide};
use crate::qos_plan::{plan_live_codec_configured_qos, StereoQosPlan, FIRST_SOUND_CIG_ID};

const GATT_TIMEOUT: Duration = Duration::from_secs(12);
const CP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(8);
const QOS_RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);
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

pub(crate) struct PreparedSink<'a> {
    pub(crate) sink: &'a PlannedSink,
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
                    } else if status.state == AseState::RELEASING
                        || (self.expected_state == AseState::CODEC_CONFIGURED
                            && status.state == AseState::IDLE)
                        || (self.expected_state == AseState::IDLE
                            && status.state == AseState::CODEC_CONFIGURED)
                        || (self.expected_state == AseState::QOS_CONFIGURED
                            && status.state == AseState::CODEC_CONFIGURED)
                        || (self.expected_state == AseState::IDLE
                            && matches!(
                                status.state,
                                AseState::QOS_CONFIGURED | AseState::ENABLING
                                    | AseState::STREAMING | AseState::DISABLING
                            ))
                        || (self.expected_state == AseState::ENABLING
                            && status.state == AseState::QOS_CONFIGURED)
                        || (self.expected_state == AseState::QOS_CONFIGURED
                            && matches!(
                                status.state,
                                AseState::ENABLING | AseState::STREAMING | AseState::DISABLING
                            ))
                    {
                        // A pre-command snapshot or a delayed notification from
                        // the prior state is not proof of this command's success.
                        // Keep waiting for BOTH CP success and the target state.
                    } else {
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
            // Keep upstream Control Point parsing strict. Only the selected
            // ASE status notification may use the observed Idle compatibility
            // gate; this is the same parser used for GATT readbacks.
            let parsed = if event.attribute_handle == prepared.sink.ase_value_handle {
                AseEvent::Status(
                    parse_ase_status_compat(&event.value)
                        .map_err(|error| format!("invalid selected ASE notification: {error}"))?,
                )
            } else {
                prepared.proxy
                    .event_from_notification(event.attribute_handle, &event.value)
                    .map_err(|error| format!("invalid ASCS Control Point notification: {error}"))?
            };
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

pub(crate) fn read_sink_status(
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
            read_ase_checked(handle, characteristic, client, transport)
                .map_err(|error| Error::Remote(format!("ASE status read failed: {error}")))
        }
    )
    .map_err(|error| format!(
        "{} ASE {} status read failed: {error}",
        prepared.sink.side.as_str(), prepared.sink.ase_id
    ))
}

pub(crate) fn prepare_sink<'a>(
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
            let current = read_ase_checked(
                sink.acl_handle, selected_ase, client, transport
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

pub(crate) fn perform_operation(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
    operation: AseOperation,
    expected_state: AseState,
    deadline_window: Duration,
) -> Result<AseStatus, String> {
    let opcode = operation.opcode();
    let handle = prepared.sink.acl_handle;
    // A second ear may have generated notifications while this ACL was idle.
    // Drain before submitting a new CP opcode so stale events cannot satisfy
    // (or accidentally reject) the new operation's response gate.
    let stale = controller.drain_att_value_events(&[handle])
        .map_err(|error| format!("ASCS pre-command event drain failed: {error}"))?;
    if !stale.is_empty() {
        println!(
            "[R4.2/EVENT] {} discarded {} pre-command ATT value event(s)",
            prepared.sink.side.as_str(), stale.len()
        );
    }
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

pub(crate) fn configure_sink(
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


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QosConfiguredMember {
    pub side: StereoSide,
    pub identity_address: String,
    pub ase_id: u8,
    pub actual: AseQosConfiguration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QosConfiguredSetSnapshot {
    pub members: Vec<QosConfiguredMember>,
    /// Assigned handles from the transient controller CIG (invalid after exit).
    pub allocation: CigAllocation,
    pub released_to_idle: bool,
    pub cig_removed: bool,
}

fn verify_qos_status(
    sink: &PlannedSink,
    planned: &ConfigQosParameters,
    status: AseStatus,
) -> Result<QosConfiguredMember, String> {
    if status.ase_id != planned.ase_id || status.state != AseState::QOS_CONFIGURED {
        return Err(format!(
            "{} QoS status wrong ASE/state: ASE={} state=0x{:02X}",
            sink.side.as_str(), status.ase_id, status.state.0
        ));
    }
    let AseStatusDetails::QosConfigured(actual) = status.details else {
        return Err(format!("{} QoSConfigured status has no decoded QoS data", sink.side.as_str()));
    };
    if actual.cig_id != planned.cig_id || actual.cis_id != planned.cis_id
        || actual.sdu_interval != planned.sdu_interval || actual.framing != planned.framing
        || actual.phy != planned.phy || actual.max_sdu != planned.max_sdu
        || actual.retransmission_number != planned.retransmission_number
        || actual.max_transport_latency != planned.max_transport_latency
        || actual.presentation_delay != planned.presentation_delay
    {
        return Err(format!(
            "{} ASE={} QoS readback differs from plan: actual={actual:?} planned={planned:?}",
            sink.side.as_str(), status.ase_id
        ));
    }
    Ok(QosConfiguredMember {
        side: sink.side,
        identity_address: sink.identity_address.clone(),
        ase_id: sink.ase_id,
        actual,
    })
}

pub(crate) fn configure_qos_sink(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
    qos: &StereoQosPlan,
) -> Result<QosConfiguredMember, String> {
    let parameter = if prepared.sink.side == StereoSide::Left {
        &qos.left.ascs
    } else {
        &qos.right.ascs
    };
    let status = perform_operation(
        controller, prepared,
        AseOperation::ConfigQos(vec![parameter.clone()]),
        AseState::QOS_CONFIGURED, QOS_RESPONSE_TIMEOUT,
    )?;
    let member = verify_qos_status(prepared.sink, parameter, status)?;
    println!(
        "[R5.2/QOS] {} ASE={} CIS={} QoSConfigured PASS (CP + notify + readback)",
        prepared.sink.side.as_str(), prepared.sink.ase_id, parameter.cis_id
    );
    Ok(member)
}

/// R5.2-A: both Codec Configured -> Set CIG -> both QoS Configured,
/// then Release each ASE to Idle and Remove the still-unestablished CIG.
/// Does not send HCI Create CIS, ASCS Enable or any ISO data.
pub fn configure_qos_and_release(
    controller: &mut AttachedController,
    codec: &CodecConfigurationPlan,
    capabilities: IsoControllerCapabilities,
) -> Result<QosConfiguredSetSnapshot, String> {
    if codec.left.acl_handle == codec.right.acl_handle {
        return Err("R5.2-A requires two distinct active ACL handles".into());
    }
    capabilities.validate_for_unfragmented_cis(codec.stream.octets_per_codec_frame)?;
    // Both subscription/Idle gates finish before the first mutation.
    let prepared = vec![
        prepare_sink(controller, &codec.left)?,
        prepare_sink(controller, &codec.right)?,
    ];
    println!("[R5.2/TXN] both ASEs prepared; Codec -> CIG -> QoS");

    let mut attempted = 0usize;
    let mut cig_may_exist = false;
    let mut allocation = None::<CigAllocation>;
    let mut codec_members = Vec::with_capacity(2);
    let mut qos_members = Vec::with_capacity(2);

    let stage = (|| -> Result<(), String> {
        for sink in &prepared {
            attempted += 1; // include any uncertain ATT write in rollback
            codec_members.push(configure_sink(controller, sink)?);
        }
        let live = CodecConfiguredSetSnapshot {
            members: codec_members.clone(),
            all_released_to_idle: false,
        };
        let qos = plan_live_codec_configured_qos(codec, &live, capabilities)?;
        println!(
            "[R5.2/PLAN] CIG={} CIS={} PHY={} interval={}us RTN(L/R)={}/{} latency={}ms presentation={}us",
            qos.cig.cig_id, qos.cig.cis_parameters.len(), qos.phy_c_to_p,
            qos.sdu_interval_us, qos.left.ascs.retransmission_number,
            qos.right.ascs.retransmission_number, qos.max_transport_latency_ms,
            qos.presentation_delay_us
        );
        let cis_ids = qos.cig.cis_parameters.iter().map(|cis| cis.cis_id).collect::<Vec<_>>();
        let new_allocation = controller.set_cig_parameters_unestablished(
            qos.hci_set_cig_command(), qos.cig.cig_id, &cis_ids
        ).map_err(|failure| {
            cig_may_exist = failure.may_have_allocated;
            format!("HCI Set CIG Parameters failed: {failure}")
        })?;
        cig_may_exist = true;
        if new_allocation.cis.iter().any(|cis| {
            cis.connection_handle == codec.left.acl_handle
                || cis.connection_handle == codec.right.acl_handle
        }) {
            return Err("BE200 returned a CIS handle overlapping an active ACL".into());
        }
        allocation = Some(new_allocation);
        for sink in &prepared {
            qos_members.push(configure_qos_sink(controller, sink, &qos)?);
        }
        Ok(())
    })();

    // Guaranteed cleanup boundary for all *attempted* ASE mutations, even
    // when a CP write succeeded but the response was lost.
    let mut cleanup_errors = Vec::new();
    for sink in prepared[..attempted].iter().rev() {
        if let Err(error) = release_if_needed(controller, sink) {
            cleanup_errors.push(format!("ASE Release/Idle: {error}"));
        }
    }
    if cig_may_exist {
        if let Err(error) = controller.remove_unestablished_cig(FIRST_SOUND_CIG_ID) {
            cleanup_errors.push(format!("HCI Remove CIG unconfirmed: {error}"));
        }
    }
    if let Err(error) = stage {
        return Err(if cleanup_errors.is_empty() {
            format!("R5.2-A failed; cleanup confirmed: {error}")
        } else {
            format!(
                "R5.2-A failed: {error}; cleanup UNCONFIRMED: {}",
                cleanup_errors.join("; ")
            )
        });
    }
    if !cleanup_errors.is_empty() {
        return Err(format!(
            "R5.2-A both QoS Configured but cleanup UNCONFIRMED: {}",
            cleanup_errors.join("; ")
        ));
    }
    let allocation = allocation.ok_or("R5.2-A missing controller CIG allocation")?;
    Ok(QosConfiguredSetSnapshot {
        members: qos_members,
        allocation,
        released_to_idle: true,
        cig_removed: true,
    })
}

pub(crate) fn release_if_needed(
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

    fn qos_gate_fixture() -> (PlannedSink, ConfigQosParameters, AseStatus) {
        let sink = PlannedSink {
            side: StereoSide::Left,
            identity_address: "02:00:00:00:00:02".into(),
            rank: 2,
            acl_handle: 0x0801,
            ase_id: 1,
            ase_value_handle: 0x0046,
            ascs_control_point_handle: 0x0043,
            audio_location: bumble_profiles::bap::AudioLocation::FRONT_LEFT,
            codec_id: bumble_hci::CodingFormat::LC3,
            codec_specific_configuration: CodecSpecificConfiguration {
                sampling_frequency: None,
                frame_duration: None,
                audio_channel_allocation: None,
                octets_per_codec_frame: None,
                codec_frames_per_sdu: None,
            },
        };
        let parameters = ConfigQosParameters {
            ase_id: 1, cig_id: 1, cis_id: 1,
            sdu_interval: 10_000, framing: 0, phy: 2,
            max_sdu: 100, retransmission_number: 13,
            max_transport_latency: 95, presentation_delay: 40_000,
        };
        let qos = AseQosConfiguration {
            cig_id: 1, cis_id: 1, sdu_interval: 10_000,
            framing: 0, phy: 2, max_sdu: 100,
            retransmission_number: 13, max_transport_latency: 95,
            presentation_delay: 40_000,
        };
        let status = AseStatus {
            ase_id: 1,
            state: AseState::QOS_CONFIGURED,
            details: AseStatusDetails::QosConfigured(qos),
        };
        (sink, parameters, status)
    }

    #[test]
    fn qos_status_readback_must_match_all_nine_parameters() {
        let (sink, parameters, status) = qos_gate_fixture();
        let good = verify_qos_status(&sink, &parameters, status.clone()).unwrap();
        assert_eq!(good.actual.presentation_delay, 40_000);
        let mut changed = status.clone();
        if let AseStatusDetails::QosConfigured(ref mut actual) = changed.details {
            actual.max_sdu = 101;
        }
        assert!(verify_qos_status(&sink, &parameters, changed).is_err());
        let mut wrong_cis = status.clone();
        if let AseStatusDetails::QosConfigured(ref mut actual) = wrong_cis.details {
            actual.cis_id = 2;
        }
        assert!(verify_qos_status(&sink, &parameters, wrong_cis).is_err());
        let mut wrong_state = status;
        wrong_state.state = AseState::CODEC_CONFIGURED;
        assert!(verify_qos_status(&sink, &parameters, wrong_state).is_err());
    }

    #[test]
    fn qos_gate_requires_cp_success_and_qos_state_even_out_of_order() {
        let mut gate = ResponseGate::new(AseOpcode::ConfigQos, 1, AseState::QOS_CONFIGURED);
        // Delayed Codec Configured notification is not a successful QoS gate.
        gate.accept(state(AseState::CODEC_CONFIGURED)).unwrap();
        assert!(!gate.complete());
        gate.accept(response(AseOpcode::ConfigQos, AseResponseCode::SUCCESS)).unwrap();
        assert!(!gate.complete());
        gate.accept(state(AseState::QOS_CONFIGURED)).unwrap();
        assert!(gate.complete());
    }

    #[test]
    fn disable_gate_accepts_delayed_enabling_and_streaming_snapshots() {
        let mut gate = ResponseGate::new(AseOpcode::Disable, 1, AseState::QOS_CONFIGURED);
        gate.accept(state(AseState::ENABLING)).unwrap();
        gate.accept(state(AseState::STREAMING)).unwrap();
        gate.accept(response(AseOpcode::Disable, AseResponseCode::SUCCESS)).unwrap();
        assert!(!gate.complete());
        gate.accept(state(AseState::QOS_CONFIGURED)).unwrap();
        assert!(gate.complete());
    }

    #[test]
    fn release_gate_tolerates_delayed_previous_qos_status() {
        let mut gate = ResponseGate::new(AseOpcode::Release, 1, AseState::IDLE);
        gate.accept(state(AseState::QOS_CONFIGURED)).unwrap();
        gate.accept(state(AseState::RELEASING)).unwrap();
        gate.accept(response(AseOpcode::Release, AseResponseCode::SUCCESS)).unwrap();
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
