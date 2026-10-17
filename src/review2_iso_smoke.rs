use std::{
    thread,
    time::{Duration, Instant},
};

use bumble::Uuid;
use bumble_hci::CodingFormat;
use bumble_profiles::{
    ascs::{
        AseControlResponse, AseMetadataParameters, AseOperation,
        AseQosConfiguration, AseResponseCode, AseState, AseStatus,
        AseStatusDetails, ConfigCodecParameters, ConfigQosParameters,
        ASE_CONTROL_POINT_CHARACTERISTIC, SINK_ASE_CHARACTERISTIC,
    },
    bap::{
        AudioLocation, CodecSpecificConfiguration, ContextType,
        FrameDuration, SamplingFrequency,
    },
    le_audio::{Metadata, MetadataEntry, MetadataTag},
    pacs::SINK_AUDIO_LOCATION_CHARACTERISTIC,
};

use crate::{
    architecture::types::AudioSide,
    hci_host::{
        connection::ConnectionToken,
        gatt::{
            GattCharacteristic, GattOperation, GattReport, GattRequest,
            GattResult, GattValueEvent,
        },
        iso::{
            CigAllocation, CigConfig, CigConfigureState, CisAclBinding,
            CisConfig, CisCreateRequest, CisCreateState, EstablishedCis,
        },
        iso_credit::IsoCreditSnapshot,
        iso_lifecycle::{CigRemoveState, CisDisconnectState},
        iso_path::{
            IsoDataPathConfig, IsoDataPathDirection,
            IsoPathOperationState,
        },
        iso_tx::{
            IsoDrainRequest, IsoDrainState, IsoSduGroupRequest,
            IsoSduRequest,
        },
        mailbox::HciReport,
        stack::HciStack,
    },
    review2_bond_smoke::{
        open_dual_encrypted_bond_session_for_iso,
        parse_bond_smoke_args, BondSmokeConfig, BondSmokeExecuteError,
        BondSmokeMemberResult, BondSmokeResult, BondSmokeTracker,
    },
    review2_gatt_smoke::{
        execute_gatt, required_service, ASCS_UUID, PACS_UUID,
    },
    runtime::mailbox::{RecvError, SendError},
};

const DEFAULT_FRAMES: usize = 100;
const MAX_FRAMES: usize = 500;
const OCTETS_PER_SDU: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSmokeConfig {
    pub bond: BondSmokeConfig,
    pub frames: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoTransportFrame {
    pub left: Vec<u8>,
    pub right: Vec<u8>,
}

pub fn parse_iso_smoke_args(
    args: &[String],
) -> Result<IsoSmokeConfig, String> {
    let mut bond_args = Vec::new();
    let mut frames = DEFAULT_FRAMES;
    let mut confirmed = false;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--frames" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --frames".to_string())?;
                frames = value
                    .parse::<usize>()
                    .map_err(|_| "--frames must be an integer".to_string())?;
                if !(1..=MAX_FRAMES).contains(&frames) {
                    return Err(format!(
                        "--frames must be between 1 and {MAX_FRAMES}"
                    ));
                }
                index += 2;
            }
            "--confirm-iso-hil" => {
                confirmed = true;
                index += 1;
            }
            "--sfi" | "--ddc" | "--bond-root" | "--name"
            | "--connect-seconds" => {
                let flag = args[index].clone();
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {flag}"))?
                    .clone();
                bond_args.push(flag);
                bond_args.push(value);
                index += 2;
            }
            unknown => {
                return Err(format!(
                    "unknown review2 iso-smoke argument: {unknown}"
                ))
            }
        }
    }

    if !confirmed {
        return Err(
            "--confirm-iso-hil is required for real stored-bond ISO HIL"
                .into(),
        );
    }

    bond_args.push("--confirm-stored-bond-hil".into());
    let bond = parse_bond_smoke_args(&bond_args)?;
    Ok(IsoSmokeConfig { bond, frames })
}

/// Review-2D smoke payloads are deliberately codec-opaque. The smoke validates
/// HCI ISO packetization, controller credits, USB TX and drain/teardown; it does
/// not claim LC3 or acoustic correctness. Product Media/LC3 remains a later
/// architecture phase.
pub fn prepare_transport_frames(
    frame_count: usize,
) -> Result<Vec<IsoTransportFrame>, String> {
    if !(1..=MAX_FRAMES).contains(&frame_count) {
        return Err(format!(
            "frame count must be between 1 and {MAX_FRAMES}"
        ));
    }

    Ok((0..frame_count)
        .map(|frame| {
            let seed = frame as u8;
            let mut left = vec![0u8; OCTETS_PER_SDU];
            let mut right = vec![0u8; OCTETS_PER_SDU];
            for (index, byte) in left.iter_mut().enumerate() {
                *byte = seed.wrapping_add(index as u8);
            }
            for (index, byte) in right.iter_mut().enumerate() {
                *byte = seed
                    .wrapping_add(index as u8)
                    .wrapping_add(0x55);
            }
            IsoTransportFrame { left, right }
        })
        .collect())
}


const GATT_TIMEOUT: Duration = Duration::from_secs(8);
const CCCD_UUID: u16 = 0x2902;
const CIG_ID: u8 = 1;
const LEFT_CIS_ID: u8 = 1;
const RIGHT_CIS_ID: u8 = 2;
const SDU_INTERVAL_US: u32 = 10_000;
const MAX_TRANSPORT_LATENCY_MS: u16 = 95;
const PRESENTATION_DELAY_US: u32 = 40_000;
const RTN: u8 = 13;
const PHY_2M: u8 = 0x02;

const LEFT_LOCATION_MASK: u32 =
    AudioLocation::FRONT_LEFT.0
        | AudioLocation::BACK_LEFT.0
        | AudioLocation::FRONT_LEFT_OF_CENTER.0
        | AudioLocation::SIDE_LEFT.0
        | AudioLocation::TOP_FRONT_LEFT.0
        | AudioLocation::TOP_BACK_LEFT.0
        | AudioLocation::TOP_SIDE_LEFT.0
        | AudioLocation::BOTTOM_FRONT_LEFT.0
        | AudioLocation::FRONT_LEFT_WIDE.0
        | AudioLocation::LEFT_SURROUND.0;
const RIGHT_LOCATION_MASK: u32 =
    AudioLocation::FRONT_RIGHT.0
        | AudioLocation::BACK_RIGHT.0
        | AudioLocation::FRONT_RIGHT_OF_CENTER.0
        | AudioLocation::SIDE_RIGHT.0
        | AudioLocation::TOP_FRONT_RIGHT.0
        | AudioLocation::TOP_BACK_RIGHT.0
        | AudioLocation::TOP_SIDE_RIGHT.0
        | AudioLocation::BOTTOM_FRONT_RIGHT.0
        | AudioLocation::FRONT_RIGHT_WIDE.0
        | AudioLocation::RIGHT_SURROUND.0;

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedAse {
    side: AudioSide,
    identity_address: String,
    connection: ConnectionToken,
    location: AudioLocation,
    ase_id: u8,
    ase_handle: u16,
    ase_cccd_handle: u16,
    control_point_handle: u16,
    control_point_cccd_handle: u16,
}

fn next_request_id(next: &mut u64) -> u64 {
    let value = *next;
    *next = next.saturating_add(1);
    value
}

fn run_gatt(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    connection: ConnectionToken,
    next: &mut u64,
    operation: GattOperation,
) -> Result<GattResult, String> {
    execute_gatt(
        stack,
        tracker,
        connection,
        next_request_id(next),
        operation,
        GATT_TIMEOUT,
    )
}

fn read_value(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    connection: ConnectionToken,
    next: &mut u64,
    handle: u16,
) -> Result<Vec<u8>, String> {
    match run_gatt(
        stack,
        tracker,
        connection,
        next,
        GattOperation::ReadValue {
            attribute_handle: handle,
        },
    )? {
        GattResult::Value {
            attribute_handle,
            value,
        } if attribute_handle == handle => Ok(value),
        other => Err(format!(
            "unexpected read result for 0x{handle:04X}: {other:?}"
        )),
    }
}

fn classify_location(
    raw: u32,
) -> Result<(AudioSide, AudioLocation), String> {
    let left = raw & LEFT_LOCATION_MASK;
    let right = raw & RIGHT_LOCATION_MASK;
    let (side, bits) = match (left != 0, right != 0) {
        (true, false) => (AudioSide::Left, left),
        (false, true) => (AudioSide::Right, right),
        (true, true) => {
            return Err(format!(
                "mixed left/right Sink Audio Location 0x{raw:08X}"
            ))
        }
        (false, false) => {
            return Err(format!(
                "no left/right Sink Audio Location in 0x{raw:08X}"
            ))
        }
    };
    Ok((side, AudioLocation(1u32 << bits.trailing_zeros())))
}

fn parse_ase_status_compat(value: &[u8]) -> Result<AseStatus, String> {
    if value.len() >= 2 && value[1] == AseState::IDLE.0 {
        let ase_id = value[0];
        if ase_id == 0 {
            return Err("ASE ID zero is invalid".into());
        }
        return Ok(AseStatus {
            ase_id,
            state: AseState::IDLE,
            details: AseStatusDetails::None,
        });
    }
    AseStatus::from_bytes(value)
        .map_err(|error| format!("ASE status parse: {error}"))
}

fn characteristic<'a>(
    values: &'a [GattCharacteristic],
    uuid: u16,
    label: &str,
) -> Result<&'a GattCharacteristic, String> {
    let wanted = Uuid::from_16_bits(uuid);
    let matches = values
        .iter()
        .filter(|value| value.uuid == wanted)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "{label}: expected exactly one characteristic UUID 0x{uuid:04X}, found {}",
            matches.len()
        ));
    }
    Ok(matches[0])
}

fn discover_cccd(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    connection: ConnectionToken,
    next: &mut u64,
    characteristic: &GattCharacteristic,
    label: &str,
) -> Result<u16, String> {
    let start = characteristic.handle.saturating_add(1);
    if start > characteristic.end_group_handle {
        return Err(format!("{label}: characteristic has no descriptor range"));
    }
    let descriptors = match run_gatt(
        stack,
        tracker,
        connection,
        next,
        GattOperation::DiscoverDescriptors {
            starting_handle: start,
            ending_handle: characteristic.end_group_handle,
        },
    )? {
        GattResult::Descriptors(values) => values,
        other => {
            return Err(format!(
                "{label}: unexpected descriptor result {other:?}"
            ))
        }
    };
    let wanted = Uuid::from_16_bits(CCCD_UUID);
    let cccds = descriptors
        .iter()
        .filter(|descriptor| descriptor.uuid == wanted)
        .collect::<Vec<_>>();
    if cccds.len() != 1 {
        return Err(format!(
            "{label}: expected exactly one CCCD, found {}",
            cccds.len()
        ));
    }
    Ok(cccds[0].handle)
}

fn subscribe(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    connection: ConnectionToken,
    next: &mut u64,
    value_handle: u16,
    cccd_handle: u16,
    label: &str,
) -> Result<(), String> {
    match run_gatt(
        stack,
        tracker,
        connection,
        next,
        GattOperation::Subscribe {
            value_handle,
            cccd_handle,
            indicate: false,
        },
    )? {
        GattResult::Subscription {
            value_handle: actual_value,
            cccd_handle: actual_cccd,
            enabled: true,
            ..
        } if actual_value == value_handle && actual_cccd == cccd_handle => {
            Ok(())
        }
        other => Err(format!(
            "{label}: unexpected subscription result {other:?}"
        )),
    }
}

fn prepare_member(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    member: &BondSmokeMemberResult,
    next: &mut u64,
) -> Result<PreparedAse, String> {
    match run_gatt(
        stack,
        tracker,
        member.connection,
        next,
        GattOperation::ExchangeMtu {
            client_rx_mtu: 247,
        },
    )? {
        GattResult::Mtu { mtu } if mtu >= 64 => {}
        GattResult::Mtu { mtu } => {
            return Err(format!(
                "{} ATT MTU {mtu} is too small for ASCS smoke",
                member.identity_address
            ))
        }
        other => {
            return Err(format!(
                "{} unexpected MTU result: {other:?}",
                member.identity_address
            ))
        }
    }

    let services = match run_gatt(
        stack,
        tracker,
        member.connection,
        next,
        GattOperation::DiscoverServices,
    )? {
        GattResult::Services(values) => values,
        other => {
            return Err(format!(
                "{} unexpected service discovery result: {other:?}",
                member.identity_address
            ))
        }
    };
    let pacs = required_service(&services, PACS_UUID, "PACS")?;
    let ascs = required_service(&services, ASCS_UUID, "ASCS")?;

    let pacs_chars = match run_gatt(
        stack,
        tracker,
        member.connection,
        next,
        GattOperation::DiscoverCharacteristics {
            service_handle: pacs.start_handle,
            end_group_handle: pacs.end_handle,
        },
    )? {
        GattResult::Characteristics(values) => values,
        other => {
            return Err(format!(
                "{} unexpected PACS characteristic result: {other:?}",
                member.identity_address
            ))
        }
    };
    let location_char = characteristic(
        &pacs_chars,
        SINK_AUDIO_LOCATION_CHARACTERISTIC,
        "PACS Sink Audio Location",
    )?;
    let location_bytes = read_value(
        stack,
        tracker,
        member.connection,
        next,
        location_char.handle,
    )?;
    let location_raw = u32::from_le_bytes(
        location_bytes
            .as_slice()
            .try_into()
            .map_err(|_| {
                format!(
                    "{} Sink Audio Location has {} bytes, expected 4",
                    member.identity_address,
                    location_bytes.len()
                )
            })?,
    );
    let (side, location) = classify_location(location_raw)?;

    let ascs_chars = match run_gatt(
        stack,
        tracker,
        member.connection,
        next,
        GattOperation::DiscoverCharacteristics {
            service_handle: ascs.start_handle,
            end_group_handle: ascs.end_handle,
        },
    )? {
        GattResult::Characteristics(values) => values,
        other => {
            return Err(format!(
                "{} unexpected ASCS characteristic result: {other:?}",
                member.identity_address
            ))
        }
    };
    let ase = characteristic(
        &ascs_chars,
        SINK_ASE_CHARACTERISTIC,
        "ASCS Sink ASE",
    )?;
    let control_point = characteristic(
        &ascs_chars,
        ASE_CONTROL_POINT_CHARACTERISTIC,
        "ASCS Control Point",
    )?;
    let initial = parse_ase_status_compat(&read_value(
        stack,
        tracker,
        member.connection,
        next,
        ase.handle,
    )?)?;
    if initial.state != AseState::IDLE {
        return Err(format!(
            "{} ASE {} is not Idle before smoke: state=0x{:02X}",
            member.identity_address,
            initial.ase_id,
            initial.state.0
        ));
    }

    let ase_cccd = discover_cccd(
        stack,
        tracker,
        member.connection,
        next,
        ase,
        "Sink ASE",
    )?;
    let cp_cccd = discover_cccd(
        stack,
        tracker,
        member.connection,
        next,
        control_point,
        "ASCS Control Point",
    )?;
    subscribe(
        stack,
        tracker,
        member.connection,
        next,
        ase.handle,
        ase_cccd,
        "Sink ASE",
    )?;
    subscribe(
        stack,
        tracker,
        member.connection,
        next,
        control_point.handle,
        cp_cccd,
        "ASCS Control Point",
    )?;

    Ok(PreparedAse {
        side,
        identity_address: member.identity_address.clone(),
        connection: member.connection,
        location,
        ase_id: initial.ase_id,
        ase_handle: ase.handle,
        ase_cccd_handle: ase_cccd,
        control_point_handle: control_point.handle,
        control_point_cccd_handle: cp_cccd,
    })
}

fn notification_parts(
    event: &GattValueEvent,
) -> (ConnectionToken, u16, &[u8]) {
    match event {
        GattValueEvent::Notification {
            connection,
            attribute_handle,
            value,
        }
        | GattValueEvent::Indication {
            connection,
            attribute_handle,
            value,
        } => (*connection, *attribute_handle, value),
    }
}

fn perform_ascs_operation(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    prepared: &PreparedAse,
    next: &mut u64,
    operation: AseOperation,
    expected_state: AseState,
) -> Result<AseStatus, String> {
    let request_id = next_request_id(next);
    let opcode = operation.opcode();
    let value = operation
        .to_bytes()
        .map_err(|error| format!("ASCS {opcode:?} encode: {error}"))?;

    stack
        .control()
        .try_begin_gatt(GattRequest {
            request_id,
            connection: prepared.connection,
            operation: GattOperation::WriteRequest {
                attribute_handle: prepared.control_point_handle,
                value,
            },
        })
        .map_err(|error| {
            format!("ASCS {opcode:?} submit: {error:?}")
        })?;

    let deadline = Instant::now() + GATT_TIMEOUT;
    let mut write_complete = false;
    let mut cp_success = false;
    let mut notified_status = None;

    while !(write_complete && cp_success && notified_status.is_some()) {
        match stack.reports().try_recv() {
            Ok(report) => {
                tracker.observe(&report);
                match report {
                    HciReport::Gatt(GattReport::Complete {
                        request_id: actual_id,
                        connection,
                        result:
                            GattResult::WriteComplete {
                                attribute_handle,
                                with_response: true,
                            },
                    }) if actual_id == request_id
                        && connection == prepared.connection
                        && attribute_handle
                            == prepared.control_point_handle =>
                    {
                        write_complete = true;
                    }
                    HciReport::Gatt(GattReport::Failed(failure))
                        if failure.request_id == request_id
                            && failure.connection
                                == prepared.connection =>
                    {
                        return Err(format!(
                            "ASCS {opcode:?} ATT write failed: {:?}",
                            failure.kind
                        ))
                    }
                    HciReport::Gatt(GattReport::Value(event)) => {
                        let (connection, handle, bytes) =
                            notification_parts(&event);
                        if connection != prepared.connection {
                            continue;
                        }
                        if handle == prepared.control_point_handle {
                            let response =
                                AseControlResponse::from_bytes(bytes)
                                    .map_err(|error| {
                                        format!(
                                            "ASCS CP response parse: {error}"
                                        )
                                    })?;
                            if response.opcode == opcode {
                                let item = response
                                    .responses
                                    .iter()
                                    .find(|item| {
                                        item.ase_id == prepared.ase_id
                                    })
                                    .ok_or_else(|| {
                                        format!(
                                            "ASCS {opcode:?} response omitted ASE {}",
                                            prepared.ase_id
                                        )
                                    })?;
                                if item.code != AseResponseCode::SUCCESS {
                                    return Err(format!(
                                        "ASCS {opcode:?} rejected ASE {}: code=0x{:02X} reason=0x{:02X}",
                                        prepared.ase_id,
                                        item.code.0,
                                        item.reason.0
                                    ));
                                }
                                cp_success = true;
                            }
                        } else if handle == prepared.ase_handle {
                            let status =
                                parse_ase_status_compat(bytes)?;
                            if status.ase_id == prepared.ase_id
                                && status.state == expected_state
                            {
                                notified_status = Some(status);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "ASCS {opcode:?} timed out: write={write_complete} cp={cp_success} notify={}",
                        notified_status.is_some()
                    ));
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err("HCI report lane disconnected".into())
            }
        }
    }

    let readback = parse_ase_status_compat(&read_value(
        stack,
        tracker,
        prepared.connection,
        next,
        prepared.ase_handle,
    )?)?;
    if readback.ase_id != prepared.ase_id
        || readback.state != expected_state
    {
        return Err(format!(
            "ASCS {opcode:?} readback mismatch: ASE={} state=0x{:02X}",
            readback.ase_id, readback.state.0
        ));
    }
    Ok(readback)
}

fn codec_configuration(location: AudioLocation) -> Vec<u8> {
    CodecSpecificConfiguration {
        sampling_frequency: Some(SamplingFrequency::FREQ_48000),
        frame_duration: Some(FrameDuration::DURATION_10000_US),
        audio_channel_allocation: Some(location),
        octets_per_codec_frame: Some(OCTETS_PER_SDU as u16),
        codec_frames_per_sdu: Some(1),
    }
    .to_bytes()
}

fn config_codec(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    prepared: &PreparedAse,
    next: &mut u64,
) -> Result<AseStatus, String> {
    let requested = codec_configuration(prepared.location);
    let status = perform_ascs_operation(
        stack,
        tracker,
        prepared,
        next,
        AseOperation::ConfigCodec(vec![ConfigCodecParameters {
            ase_id: prepared.ase_id,
            target_latency: 0x02,
            target_phy: 0x01,
            codec_id: CodingFormat::LC3,
            codec_specific_configuration: requested.clone(),
        }]),
        AseState::CODEC_CONFIGURED,
    )?;
    match &status.details {
        AseStatusDetails::CodecConfigured {
            codec_id,
            codec_specific_configuration,
            ..
        } if *codec_id == CodingFormat::LC3
            && codec_specific_configuration == &requested =>
        {
            Ok(status)
        }
        other => Err(format!(
            "{:?} Codec Configured readback mismatch: {other:?}",
            prepared.side
        )),
    }
}

fn qos_parameters(prepared: &PreparedAse) -> ConfigQosParameters {
    ConfigQosParameters {
        ase_id: prepared.ase_id,
        cig_id: CIG_ID,
        cis_id: match prepared.side {
            AudioSide::Left => LEFT_CIS_ID,
            AudioSide::Right => RIGHT_CIS_ID,
        },
        sdu_interval: SDU_INTERVAL_US,
        framing: 0,
        phy: PHY_2M,
        max_sdu: OCTETS_PER_SDU as u16,
        retransmission_number: RTN,
        max_transport_latency: MAX_TRANSPORT_LATENCY_MS,
        presentation_delay: PRESENTATION_DELAY_US,
    }
}

fn config_qos(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    prepared: &PreparedAse,
    next: &mut u64,
) -> Result<(), String> {
    let planned = qos_parameters(prepared);
    let status = perform_ascs_operation(
        stack,
        tracker,
        prepared,
        next,
        AseOperation::ConfigQos(vec![planned.clone()]),
        AseState::QOS_CONFIGURED,
    )?;
    match status.details {
        AseStatusDetails::QosConfigured(actual)
            if actual
                == AseQosConfiguration {
                    cig_id: planned.cig_id,
                    cis_id: planned.cis_id,
                    sdu_interval: planned.sdu_interval,
                    framing: planned.framing,
                    phy: planned.phy,
                    max_sdu: planned.max_sdu,
                    retransmission_number:
                        planned.retransmission_number,
                    max_transport_latency:
                        planned.max_transport_latency,
                    presentation_delay:
                        planned.presentation_delay,
                } =>
        {
            Ok(())
        }
        other => Err(format!(
            "{:?} QoS Configured readback mismatch: {other:?}",
            prepared.side
        )),
    }
}

fn media_metadata() -> Result<Vec<u8>, String> {
    Metadata::new(vec![MetadataEntry::new(
        MetadataTag::STREAMING_AUDIO_CONTEXTS,
        ContextType::MEDIA.0.to_le_bytes(),
    )])
    .to_bytes()
    .map_err(|error| format!("MEDIA metadata encode: {error}"))
}

fn enable(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    prepared: &PreparedAse,
    next: &mut u64,
) -> Result<(), String> {
    let metadata = media_metadata()?;
    let status = perform_ascs_operation(
        stack,
        tracker,
        prepared,
        next,
        AseOperation::Enable(vec![AseMetadataParameters {
            ase_id: prepared.ase_id,
            metadata,
        }]),
        AseState::ENABLING,
    )?;
    match status.details {
        AseStatusDetails::Metadata {
            cig_id,
            cis_id,
            metadata,
        } if cig_id == CIG_ID
            && cis_id
                == match prepared.side {
                    AudioSide::Left => LEFT_CIS_ID,
                    AudioSide::Right => RIGHT_CIS_ID,
                }
            && metadata.entries.iter().any(|entry| {
                entry.tag == MetadataTag::STREAMING_AUDIO_CONTEXTS
                    && entry.data
                        == ContextType::MEDIA.0.to_le_bytes()
            }) =>
        {
            Ok(())
        }
        other => Err(format!(
            "{:?} Enable readback mismatch: {other:?}",
            prepared.side
        )),
    }
}

fn disable_and_release(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    prepared: &PreparedAse,
    next: &mut u64,
) -> Result<(), String> {
    let current = parse_ase_status_compat(&read_value(
        stack,
        tracker,
        prepared.connection,
        next,
        prepared.ase_handle,
    )?)?;
    if matches!(
        current.state,
        AseState::ENABLING | AseState::STREAMING
    ) {
        perform_ascs_operation(
            stack,
            tracker,
            prepared,
            next,
            AseOperation::Disable(vec![prepared.ase_id]),
            AseState::QOS_CONFIGURED,
        )?;
    }

    let current = parse_ase_status_compat(&read_value(
        stack,
        tracker,
        prepared.connection,
        next,
        prepared.ase_handle,
    )?)?;
    if current.state != AseState::IDLE {
        perform_ascs_operation(
            stack,
            tracker,
            prepared,
            next,
            AseOperation::Release(vec![prepared.ase_id]),
            AseState::IDLE,
        )?;
    }
    Ok(())
}


const HCI_STAGE_TIMEOUT: Duration = Duration::from_secs(8);

fn cig_config() -> CigConfig {
    let cis = |cis_id| CisConfig {
        cis_id,
        max_sdu_c_to_p: OCTETS_PER_SDU as u16,
        max_sdu_p_to_c: 0,
        phy_c_to_p: PHY_2M,
        phy_p_to_c: PHY_2M,
        rtn_c_to_p: RTN,
        rtn_p_to_c: 0,
    };
    CigConfig {
        cig_id: CIG_ID,
        sdu_interval_c_to_p: SDU_INTERVAL_US,
        sdu_interval_p_to_c: SDU_INTERVAL_US,
        worst_case_sca: 0,
        packing: 0,
        framing: 0,
        max_transport_latency_c_to_p: MAX_TRANSPORT_LATENCY_MS,
        max_transport_latency_p_to_c: MAX_TRANSPORT_LATENCY_MS,
        cis: vec![cis(LEFT_CIS_ID), cis(RIGHT_CIS_ID)],
    }
}

fn wait_hci<T>(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
    stage: &str,
    mut predicate: impl FnMut(&HciReport) -> Option<Result<T, String>>,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match stack.reports().try_recv() {
            Ok(report) => {
                tracker.observe(&report);
                if let Some(result) = predicate(&report) {
                    return result;
                }
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err(format!("{stage} timed out"));
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err(format!(
                    "{stage}: HCI report lane disconnected"
                ))
            }
        }
    }
}

fn configure_cig(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
) -> Result<CigAllocation, String> {
    stack
        .control()
        .try_configure_cig(cig_config())
        .map_err(|error| {
            format!("Set CIG submit rejected: {error:?}")
        })?;
    wait_hci(
        stack,
        tracker,
        HCI_STAGE_TIMEOUT,
        "Set CIG",
        |report| match report {
            HciReport::CigConfigureState(
                CigConfigureState::Ready(allocation),
            ) => Some(Ok(allocation.clone())),
            HciReport::CigConfigureState(
                CigConfigureState::Failed(failure),
            ) => Some(Err(format!(
                "Set CIG failed: {failure:?}"
            ))),
            _ => None,
        },
    )
}

fn create_cis(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    left: &PreparedAse,
    right: &PreparedAse,
) -> Result<Vec<EstablishedCis>, String> {
    stack
        .control()
        .try_create_cis(CisCreateRequest {
            cig_id: CIG_ID,
            bindings: vec![
                CisAclBinding {
                    cis_id: LEFT_CIS_ID,
                    acl: left.connection,
                },
                CisAclBinding {
                    cis_id: RIGHT_CIS_ID,
                    acl: right.connection,
                },
            ],
        })
        .map_err(|error| {
            format!("Create CIS submit rejected: {error:?}")
        })?;

    wait_hci(
        stack,
        tracker,
        HCI_STAGE_TIMEOUT,
        "Create CIS",
        |report| match report {
            HciReport::CisCreateState(CisCreateState::Ready {
                cig_id,
                established,
            }) if *cig_id == CIG_ID => Some(Ok(established.clone())),
            HciReport::CisCreateState(CisCreateState::Failed(
                failure,
            )) => Some(Err(format!(
                "Create CIS failed: {failure:?}"
            ))),
            _ => None,
        },
    )
}

fn cis_handle(
    established: &[EstablishedCis],
    cis_id: u8,
) -> Result<u16, String> {
    established
        .iter()
        .find(|entry| entry.cis_id == cis_id)
        .map(|entry| entry.link.connection_handle)
        .ok_or_else(|| {
            format!("established CIS set omitted CIS {cis_id}")
        })
}

fn setup_iso_path(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    connection_handle: u16,
) -> Result<(), String> {
    stack
        .control()
        .try_setup_iso_path(
            IsoDataPathConfig::hci_input(connection_handle),
        )
        .map_err(|error| {
            format!(
                "ISO path setup 0x{connection_handle:04X} submit: {error:?}"
            )
        })?;

    wait_hci(
        stack,
        tracker,
        HCI_STAGE_TIMEOUT,
        "Setup ISO Data Path",
        |report| match report {
            HciReport::IsoPathState(
                IsoPathOperationState::SetupComplete {
                    connection_handle: actual,
                    direction: IsoDataPathDirection::Input,
                },
            ) if *actual == connection_handle => Some(Ok(())),
            HciReport::IsoPathState(
                IsoPathOperationState::Failed(failure),
            ) => Some(Err(format!(
                "Setup ISO Data Path failed: {failure:?}"
            ))),
            _ => None,
        },
    )
}

fn remove_iso_path(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    connection_handle: u16,
) -> Result<(), String> {
    const INPUT_DIRECTION_MASK: u8 = 0x01;
    stack
        .control()
        .try_remove_iso_path(
            connection_handle,
            INPUT_DIRECTION_MASK,
        )
        .map_err(|error| {
            format!(
                "ISO path remove 0x{connection_handle:04X} submit: {error:?}"
            )
        })?;

    wait_hci(
        stack,
        tracker,
        HCI_STAGE_TIMEOUT,
        "Remove ISO Data Path",
        |report| match report {
            HciReport::IsoPathState(
                IsoPathOperationState::RemoveComplete {
                    connection_handle: actual,
                    directions,
                },
            ) if *actual == connection_handle
                && *directions == INPUT_DIRECTION_MASK =>
            {
                Some(Ok(()))
            }
            HciReport::IsoPathState(
                IsoPathOperationState::Failed(failure),
            ) => Some(Err(format!(
                "Remove ISO Data Path failed: {failure:?}"
            ))),
            _ => None,
        },
    )
}

fn disconnect_cis(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    connection_handle: u16,
) -> Result<u8, String> {
    stack
        .control()
        .try_disconnect_cis(connection_handle)
        .map_err(|error| {
            format!(
                "CIS disconnect 0x{connection_handle:04X} submit: {error:?}"
            )
        })?;

    wait_hci(
        stack,
        tracker,
        HCI_STAGE_TIMEOUT,
        "Disconnect CIS",
        |report| match report {
            HciReport::CisDisconnectState(
                CisDisconnectState::Disconnected {
                    connection_handle: actual,
                    event_reason,
                },
            ) if *actual == connection_handle => Some(Ok(*event_reason)),
            HciReport::CisDisconnectState(
                CisDisconnectState::Failed(failure),
            ) => Some(Err(format!(
                "Disconnect CIS failed: {failure:?}"
            ))),
            _ => None,
        },
    )
}

fn remove_cig(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
) -> Result<(), String> {
    stack
        .control()
        .try_remove_cig(CIG_ID)
        .map_err(|error| {
            format!("Remove CIG submit rejected: {error:?}")
        })?;
    wait_hci(
        stack,
        tracker,
        HCI_STAGE_TIMEOUT,
        "Remove CIG",
        |report| match report {
            HciReport::CigRemoveState(
                CigRemoveState::Removed { cig_id },
            ) if *cig_id == CIG_ID => Some(Ok(())),
            HciReport::CigRemoveState(
                CigRemoveState::Failed(failure),
            ) => Some(Err(format!(
                "Remove CIG failed: {failure:?}"
            ))),
            _ => None,
        },
    )
}

fn begin_and_wait_iso_drain(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    handles: [u16; 2],
) -> Result<(), String> {
    let request = IsoDrainRequest {
        connection_handles: handles.to_vec(),
    };
    stack
        .control()
        .try_begin_iso_drain(request.clone())
        .map_err(|error| {
            format!("ISO drain submit rejected: {error:?}")
        })?;
    wait_hci(
        stack,
        tracker,
        HCI_STAGE_TIMEOUT,
        "ISO drain",
        |report| match report {
            HciReport::IsoDrainState(IsoDrainState::Drained(
                actual,
            )) if actual == &request => Some(Ok(())),
            _ => None,
        },
    )
}

fn read_iso_audit(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
) -> Result<IsoCreditSnapshot, String> {
    stack
        .control()
        .try_read_iso_tx_audit()
        .map_err(|error| {
            format!("ISO audit query rejected: {error:?}")
        })?;
    wait_hci(
        stack,
        tracker,
        Duration::from_secs(2),
        "ISO audit snapshot",
        |report| match report {
            HciReport::IsoTxAudit(snapshot) => {
                Some(Ok(snapshot.clone()))
            }
            _ => None,
        },
    )
}


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSmokeResult {
    pub bond: BondSmokeResult,
    pub frames: usize,
    pub left_identity_address: String,
    pub right_identity_address: String,
    pub left_cis_handle: u16,
    pub right_cis_handle: u16,
    pub audit_before: IsoCreditSnapshot,
    pub audit_after: IsoCreditSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsoSmokeExecuteError {
    Bond(BondSmokeExecuteError),
    Probe {
        detail: String,
        bond: BondSmokeResult,
    },
    ProbeAndCleanup {
        detail: String,
        bond: Box<BondSmokeExecuteError>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IsoProbeResult {
    frames: usize,
    left_identity_address: String,
    right_identity_address: String,
    left_cis_handle: u16,
    right_cis_handle: u16,
    audit_before: IsoCreditSnapshot,
    audit_after: IsoCreditSnapshot,
}

fn stereo_prepared(
    prepared: Vec<PreparedAse>,
) -> Result<(PreparedAse, PreparedAse), String> {
    if prepared.len() != 2 {
        return Err(format!(
            "ISO smoke requires exactly two prepared sink ASEs, found {}",
            prepared.len()
        ));
    }
    let mut left = None;
    let mut right = None;
    for item in prepared {
        let slot = match item.side {
            AudioSide::Left => &mut left,
            AudioSide::Right => &mut right,
        };
        if slot.replace(item).is_some() {
            return Err("two members claimed the same physical side".into());
        }
    }
    Ok((
        left.ok_or_else(|| "no left sink member discovered".to_string())?,
        right.ok_or_else(|| "no right sink member discovered".to_string())?,
    ))
}

fn wait_streaming(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    prepared: &PreparedAse,
    next: &mut u64,
) -> Result<(), String> {
    let expected_cis = match prepared.side {
        AudioSide::Left => LEFT_CIS_ID,
        AudioSide::Right => RIGHT_CIS_ID,
    };
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let status = parse_ase_status_compat(&read_value(
            stack,
            tracker,
            prepared.connection,
            next,
            prepared.ase_handle,
        )?)?;
        match &status.details {
            AseStatusDetails::Metadata {
                cig_id,
                cis_id,
                ..
            } if *cig_id == CIG_ID && *cis_id == expected_cis => {}
            other => {
                return Err(format!(
                    "{:?} ASE metadata/CIG/CIS mismatch while waiting for Streaming: {other:?}",
                    prepared.side
                ))
            }
        }

        if status.state == AseState::STREAMING {
            return Ok(());
        }
        if status.state != AseState::ENABLING {
            return Err(format!(
                "{:?} ASE left Enabling/Streaming path: state=0x{:02X}",
                prepared.side, status.state.0
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{:?} ASE did not reach Streaming within timeout",
                prepared.side
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn submit_transport_frames(
    stack: &HciStack,
    frames: &[IsoTransportFrame],
    handles: [u16; 2],
) -> Result<(), String> {
    let iso = stack.iso_tx();
    let start = Instant::now();
    for (index, frame) in frames.iter().enumerate() {
        let target = start
            + Duration::from_micros(
                SDU_INTERVAL_US as u64 * index as u64,
            );
        let now = Instant::now();
        if target > now {
            thread::sleep(target - now);
        }

        let group = IsoSduGroupRequest {
            sdus: vec![
                IsoSduRequest {
                    connection_handle: handles[0],
                    payload: frame.left.clone(),
                },
                IsoSduRequest {
                    connection_handle: handles[1],
                    payload: frame.right.clone(),
                },
            ],
        };
        let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            match iso.try_submit_group(group.clone()) {
                Ok(()) => break,
                Err(SendError::Full) => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "ISO data lane remained full while submitting interval {index}"
                        ));
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(SendError::Disconnected) => {
                    return Err(
                        "ISO data lane disconnected during submission".into(),
                    )
                }
            }
        }
    }
    Ok(())
}

fn wait_submitted_packets(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    baseline: &IsoCreditSnapshot,
    expected_delta: u64,
) -> Result<IsoCreditSnapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let snapshot = read_iso_audit(stack, tracker)?;
        let delta = snapshot
            .total_submitted
            .saturating_sub(baseline.total_submitted);
        if delta == expected_delta {
            return Ok(snapshot);
        }
        if delta > expected_delta {
            return Err(format!(
                "ISO Host submitted {delta} packets, expected exactly {expected_delta}"
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "ISO Host submitted only {delta}/{expected_delta} expected packets before timeout"
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn validate_completed_audit(
    before: &IsoCreditSnapshot,
    after: &IsoCreditSnapshot,
    handles: [u16; 2],
    frames: usize,
) -> Result<(), String> {
    let expected = frames as u64;
    let total_expected = expected * 2;

    let submitted = after
        .total_submitted
        .saturating_sub(before.total_submitted);
    let completed = after
        .total_completed
        .saturating_sub(before.total_completed);
    if submitted != total_expected || completed != total_expected {
        return Err(format!(
            "ISO audit mismatch: submitted={submitted} completed={completed} expected={total_expected}"
        ));
    }
    if after.in_flight != 0 {
        return Err(format!(
            "ISO drain completed with {} packet(s) still in flight",
            after.in_flight
        ));
    }

    for handle in handles {
        let submitted_on = after
            .submitted_per_handle
            .get(&handle)
            .copied()
            .unwrap_or(0)
            .saturating_sub(
                before
                    .submitted_per_handle
                    .get(&handle)
                    .copied()
                    .unwrap_or(0),
            );
        let completed_on = after
            .completed_per_handle
            .get(&handle)
            .copied()
            .unwrap_or(0)
            .saturating_sub(
                before
                    .completed_per_handle
                    .get(&handle)
                    .copied()
                    .unwrap_or(0),
            );
        if submitted_on != expected || completed_on != expected {
            return Err(format!(
                "ISO handle 0x{handle:04X} audit mismatch: submitted={submitted_on} completed={completed_on} expected={expected}"
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn run_iso_probe(
    session: &mut crate::review2_bond_smoke::DualEncryptedBondSession,
    frame_count: usize,
) -> Result<IsoProbeResult, String> {
    let frames = prepare_transport_frames(frame_count)?;
    let stack = &session.stack;
    let tracker = &mut session.tracker;
    let mut next = 1u64;

    let mut prepared = Vec::new();
    let mut cig_may_exist = false;
    let mut allocated_handles: Option<[u16; 2]> = None;
    let mut paths_may_exist = Vec::<u16>::new();
    let mut drained = false;

    let stage = (|| -> Result<IsoProbeResult, String> {
        for member in session.members.clone() {
            prepared.push(prepare_member(
                stack,
                tracker,
                &member,
                &mut next,
            )?);
        }
        let (left, right) = stereo_prepared(prepared.clone())?;

        config_codec(stack, tracker, &left, &mut next)?;
        config_codec(stack, tracker, &right, &mut next)?;

        // Set the cleanup flag before submission: a malformed success response
        // may still mean the controller allocated CIG state.
        cig_may_exist = true;
        let allocation = configure_cig(stack, tracker)?;
        let left_alloc = allocation
            .cis
            .iter()
            .find(|entry| entry.cis_id == LEFT_CIS_ID)
            .ok_or_else(|| "CIG allocation omitted left CIS".to_string())?;
        let right_alloc = allocation
            .cis
            .iter()
            .find(|entry| entry.cis_id == RIGHT_CIS_ID)
            .ok_or_else(|| "CIG allocation omitted right CIS".to_string())?;
        let allocated = [
            left_alloc.connection_handle,
            right_alloc.connection_handle,
        ];
        allocated_handles = Some(allocated);

        config_qos(stack, tracker, &left, &mut next)?;
        config_qos(stack, tracker, &right, &mut next)?;
        enable(stack, tracker, &left, &mut next)?;
        enable(stack, tracker, &right, &mut next)?;

        let established = create_cis(
            stack, tracker, &left, &right,
        )?;
        let handles = [
            cis_handle(&established, LEFT_CIS_ID)?,
            cis_handle(&established, RIGHT_CIS_ID)?,
        ];
        if handles != allocated {
            return Err(format!(
                "Create CIS handle mapping changed: allocated={allocated:04X?} established={handles:04X?}"
            ));
        }

        for handle in handles {
            paths_may_exist.push(handle);
            setup_iso_path(stack, tracker, handle)?;
        }

        wait_streaming(stack, tracker, &left, &mut next)?;
        wait_streaming(stack, tracker, &right, &mut next)?;

        let audit_before = read_iso_audit(stack, tracker)?;
        let limit = audit_before
            .limit
            .ok_or_else(|| "controller ISO packet pool is not configured".to_string())?;
        if limit == 0 {
            return Err("controller ISO packet pool has zero capacity".into());
        }

        submit_transport_frames(stack, &frames, handles)?;
        let expected_packets = (frame_count as u64) * 2;
        let _submitted = wait_submitted_packets(
            stack,
            tracker,
            &audit_before,
            expected_packets,
        )?;

        begin_and_wait_iso_drain(stack, tracker, handles)?;
        drained = true;
        let audit_after = read_iso_audit(stack, tracker)?;
        validate_completed_audit(
            &audit_before,
            &audit_after,
            handles,
            frame_count,
        )?;

        for handle in handles.into_iter().rev() {
            remove_iso_path(stack, tracker, handle)?;
            paths_may_exist.retain(|value| *value != handle);
        }
        for handle in handles.into_iter().rev() {
            let _ = disconnect_cis(stack, tracker, handle)?;
        }
        allocated_handles = None;

        disable_and_release(stack, tracker, &right, &mut next)?;
        disable_and_release(stack, tracker, &left, &mut next)?;
        remove_cig(stack, tracker)?;
        cig_may_exist = false;

        Ok(IsoProbeResult {
            frames: frame_count,
            left_identity_address: left.identity_address,
            right_identity_address: right.identity_address,
            left_cis_handle: handles[0],
            right_cis_handle: handles[1],
            audit_before,
            audit_after,
        })
    })();

    if stage.is_err() {
        let mut cleanup_errors = Vec::new();

        if let Some(handles) = allocated_handles {
            if !drained {
                if let Err(error) =
                    begin_and_wait_iso_drain(stack, tracker, handles)
                {
                    cleanup_errors.push(format!(
                        "ISO drain UNCONFIRMED: {error}"
                    ));
                }
            }

            for &handle in paths_may_exist.iter().rev() {
                if let Err(error) =
                    remove_iso_path(stack, tracker, handle)
                {
                    cleanup_errors.push(format!(
                        "ISO path remove 0x{handle:04X} UNCONFIRMED: {error}"
                    ));
                }
            }

            for handle in handles.into_iter().rev() {
                if let Err(error) =
                    disconnect_cis(stack, tracker, handle)
                {
                    cleanup_errors.push(format!(
                        "CIS disconnect 0x{handle:04X} UNCONFIRMED: {error}"
                    ));
                }
            }
        }

        for item in prepared.iter().rev() {
            if let Err(error) =
                disable_and_release(stack, tracker, item, &mut next)
            {
                cleanup_errors.push(format!(
                    "{:?} ASE cleanup UNCONFIRMED: {error}",
                    item.side
                ));
            }
        }

        if cig_may_exist {
            if let Err(error) = remove_cig(stack, tracker) {
                cleanup_errors.push(format!(
                    "Remove CIG UNCONFIRMED: {error}"
                ));
            }
        }

        if !cleanup_errors.is_empty() {
            return Err(format!(
                "{}; rollback UNCONFIRMED: {}",
                stage.unwrap_err(),
                cleanup_errors.join("; ")
            ));
        }
    }

    stage
}

#[cfg(windows)]
pub fn execute_iso_smoke(
    config: IsoSmokeConfig,
) -> Result<IsoSmokeResult, IsoSmokeExecuteError> {
    let frame_count = config.frames;
    let mut session =
        open_dual_encrypted_bond_session_for_iso(config.bond)
            .map_err(IsoSmokeExecuteError::Bond)?;

    let probe = run_iso_probe(&mut session, frame_count);
    let finish = session.finish();

    match (probe, finish) {
        (Ok(probe), Ok(bond)) => Ok(IsoSmokeResult {
            bond,
            frames: probe.frames,
            left_identity_address: probe.left_identity_address,
            right_identity_address: probe.right_identity_address,
            left_cis_handle: probe.left_cis_handle,
            right_cis_handle: probe.right_cis_handle,
            audit_before: probe.audit_before,
            audit_after: probe.audit_after,
        }),
        (Ok(_), Err(error)) => Err(IsoSmokeExecuteError::Bond(error)),
        (Err(detail), Ok(bond)) => {
            Err(IsoSmokeExecuteError::Probe { detail, bond })
        }
        (Err(detail), Err(bond)) => {
            Err(IsoSmokeExecuteError::ProbeAndCleanup {
                detail,
                bond: Box::new(bond),
            })
        }
    }
}


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSmokeProbeResult {
    pub left_cis_handle: u16,
    pub right_cis_handle: u16,
    pub frames_submitted: usize,
    pub audit_before: IsoCreditSnapshot,
    pub audit_after: IsoCreditSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSmokeResult {
    pub bond: BondSmokeResult,
    pub probe: IsoSmokeProbeResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsoSmokeExecuteError {
    Bond(BondSmokeExecuteError),
    Probe {
        detail: String,
        cleanup: BondSmokeCleanupSummary,
        bond_unchanged: bool,
        exit: HciStackExit,
    },
    ProbeAndCleanup {
        detail: String,
        cleanup: Box<BondSmokeExecuteError>,
    },
}

fn wait_hci_report<T>(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    timeout: Duration,
    stage: &str,
    mut predicate: impl FnMut(&HciReport) -> Option<Result<T, String>>,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match stack.reports().try_recv() {
            Ok(report) => {
                tracker.observe(&report);
                if let Some(result) = predicate(&report) {
                    return result;
                }
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err(format!("{stage}: timed out"));
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err(format!("{stage}: report lane disconnected"));
            }
        }
    }
}

fn smoke_cig_config() -> CigConfig {
    let cis = |cis_id| CisConfig {
        cis_id,
        max_sdu_c_to_p: OCTETS_PER_SDU as u16,
        max_sdu_p_to_c: 0,
        phy_c_to_p: PHY_2M,
        phy_p_to_c: 0x01,
        rtn_c_to_p: RTN,
        rtn_p_to_c: 0,
    };
    CigConfig {
        cig_id: CIG_ID,
        sdu_interval_c_to_p: SDU_INTERVAL_US,
        sdu_interval_p_to_c: SDU_INTERVAL_US,
        worst_case_sca: 0,
        packing: 0,
        framing: 0,
        max_transport_latency_c_to_p: MAX_TRANSPORT_LATENCY_MS,
        max_transport_latency_p_to_c: MAX_TRANSPORT_LATENCY_MS,
        cis: vec![cis(LEFT_CIS_ID), cis(RIGHT_CIS_ID)],
    }
}

fn wait_cig_ready(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
) -> Result<CigAllocation, String> {
    wait_hci_report(
        stack,
        tracker,
        Duration::from_secs(5),
        "Set CIG",
        |report| match report {
            HciReport::CigConfigureState(
                CigConfigureState::Ready(allocation),
            ) => Some(Ok(allocation.clone())),
            HciReport::CigConfigureState(
                CigConfigureState::Failed(failure),
            ) => Some(Err(format!(
                "Set CIG failed: {failure:?}"
            ))),
            _ => None,
        },
    )
}

fn wait_cis_ready(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
) -> Result<Vec<EstablishedCis>, (String, Vec<EstablishedCis>)> {
    wait_hci_report(
        stack,
        tracker,
        Duration::from_secs(8),
        "Create CIS",
        |report| match report {
            HciReport::CisCreateState(CisCreateState::Ready {
                established,
                ..
            }) => Some(Ok(established.clone())),
            HciReport::CisCreateState(CisCreateState::Failed(
                failure,
            )) => Some(Err((
                format!("Create CIS failed: {failure:?}"),
                failure.established.clone(),
            ))),
            _ => None,
        },
    )
    .map_err(|detail| (detail, Vec::new()))?
}

fn wait_iso_path_setup(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    handle: u16,
) -> Result<(), String> {
    wait_hci_report(
        stack,
        tracker,
        Duration::from_secs(5),
        "Setup ISO Data Path",
        |report| match report {
            HciReport::IsoPathState(
                IsoPathOperationState::SetupComplete {
                    connection_handle,
                    direction: IsoDataPathDirection::Input,
                },
            ) if *connection_handle == handle => Some(Ok(())),
            HciReport::IsoPathState(IsoPathOperationState::Failed(
                failure,
            )) if failure.connection_handle == handle => {
                Some(Err(format!(
                    "Setup ISO Data Path 0x{handle:04X} failed: {failure:?}"
                )))
            }
            _ => None,
        },
    )
}

fn wait_iso_path_removed(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    handle: u16,
) -> Result<(), String> {
    wait_hci_report(
        stack,
        tracker,
        Duration::from_secs(5),
        "Remove ISO Data Path",
        |report| match report {
            HciReport::IsoPathState(
                IsoPathOperationState::RemoveComplete {
                    connection_handle,
                    directions: 0x01,
                },
            ) if *connection_handle == handle => Some(Ok(())),
            HciReport::IsoPathState(IsoPathOperationState::Failed(
                failure,
            )) if failure.connection_handle == handle => {
                Some(Err(format!(
                    "Remove ISO Data Path 0x{handle:04X} failed: {failure:?}"
                )))
            }
            _ => None,
        },
    )
}

fn read_iso_audit(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
) -> Result<IsoCreditSnapshot, String> {
    stack
        .control()
        .try_read_iso_tx_audit()
        .map_err(|error| {
            format!("read ISO TX audit submit: {error:?}")
        })?;
    wait_hci_report(
        stack,
        tracker,
        Duration::from_secs(2),
        "read ISO TX audit",
        |report| match report {
            HciReport::IsoTxAudit(snapshot) => {
                Some(Ok(snapshot.clone()))
            }
            _ => None,
        },
    )
}

fn wait_submitted_count(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    baseline: &IsoCreditSnapshot,
    expected_delta: u64,
) -> Result<IsoCreditSnapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = read_iso_audit(stack, tracker)?;
        if snapshot
            .total_submitted
            .saturating_sub(baseline.total_submitted)
            >= expected_delta
        {
            return Ok(snapshot);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "ISO Host submission count stalled: submitted_delta={} expected={expected_delta}",
                snapshot
                    .total_submitted
                    .saturating_sub(baseline.total_submitted)
            ));
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn wait_iso_drained(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    request: &IsoDrainRequest,
) -> Result<(), String> {
    wait_hci_report(
        stack,
        tracker,
        Duration::from_secs(5),
        "ISO drain",
        |report| match report {
            HciReport::IsoDrainState(IsoDrainState::Drained(
                actual,
            )) if actual == request => Some(Ok(())),
            _ => None,
        },
    )
}

fn wait_cis_disconnected(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    handle: u16,
) -> Result<u8, String> {
    wait_hci_report(
        stack,
        tracker,
        Duration::from_secs(5),
        "CIS disconnect",
        |report| match report {
            HciReport::CisDisconnectState(
                CisDisconnectState::Disconnected {
                    connection_handle,
                    event_reason,
                },
            ) if *connection_handle == handle => Some(Ok(*event_reason)),
            HciReport::CisDisconnectState(
                CisDisconnectState::Failed(failure),
            ) if failure.connection_handle == handle => {
                Some(Err(format!(
                    "CIS disconnect 0x{handle:04X} failed: {failure:?}"
                )))
            }
            _ => None,
        },
    )
}

fn wait_cig_removed(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
) -> Result<(), String> {
    wait_hci_report(
        stack,
        tracker,
        Duration::from_secs(5),
        "Remove CIG",
        |report| match report {
            HciReport::CigRemoveState(CigRemoveState::Removed {
                cig_id: CIG_ID,
            }) => Some(Ok(())),
            HciReport::CigRemoveState(CigRemoveState::Failed(
                failure,
            )) => Some(Err(format!(
                "Remove CIG failed: {failure:?}"
            ))),
            _ => None,
        },
    )
}

fn verify_post_cis_ase(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    prepared: &PreparedAse,
    next: &mut u64,
) -> Result<(), String> {
    let status = parse_ase_status_compat(&read_value(
        stack,
        tracker,
        prepared.connection,
        next,
        prepared.ase_handle,
    )?)?;
    if !matches!(
        status.state,
        AseState::ENABLING | AseState::STREAMING
    ) {
        return Err(format!(
            "{:?} ASE {} post-CIS state=0x{:02X}",
            prepared.side,
            prepared.ase_id,
            status.state.0
        ));
    }
    match status.details {
        AseStatusDetails::Metadata {
            cig_id,
            cis_id,
            ..
        } if cig_id == CIG_ID
            && cis_id
                == match prepared.side {
                    AudioSide::Left => LEFT_CIS_ID,
                    AudioSide::Right => RIGHT_CIS_ID,
                } =>
        {
            Ok(())
        }
        other => Err(format!(
            "{:?} post-CIS metadata mismatch: {other:?}",
            prepared.side
        )),
    }
}

fn cleanup_iso_probe(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    prepared: &[PreparedAse],
    next: &mut u64,
    cis_handles: &[u16],
    paths_may_exist: &[u16],
    cig_may_exist: bool,
) -> Vec<String> {
    let mut errors = Vec::new();

    if !cis_handles.is_empty() {
        let request = IsoDrainRequest {
            connection_handles: cis_handles.to_vec(),
        };
        match stack.control().try_begin_iso_drain(request.clone()) {
            Ok(()) => {
                if let Err(error) =
                    wait_iso_drained(stack, tracker, &request)
                {
                    errors.push(format!(
                        "ISO drain before teardown UNCONFIRMED: {error}"
                    ));
                }
            }
            Err(error) => errors.push(format!(
                "ISO drain submit UNCONFIRMED: {error:?}"
            )),
        }
    }

    for &handle in paths_may_exist.iter().rev() {
        match stack.control().try_remove_iso_path(handle, 0x01) {
            Ok(()) => {
                if let Err(error) =
                    wait_iso_path_removed(stack, tracker, handle)
                {
                    errors.push(error);
                }
            }
            Err(error) => errors.push(format!(
                "remove ISO path 0x{handle:04X} submit: {error:?}"
            )),
        }
    }

    for &handle in cis_handles.iter().rev() {
        match stack.control().try_disconnect_cis(handle) {
            Ok(()) => {
                if let Err(error) =
                    wait_cis_disconnected(stack, tracker, handle)
                {
                    errors.push(error);
                }
            }
            Err(error) => errors.push(format!(
                "disconnect CIS 0x{handle:04X} submit: {error:?}"
            )),
        }
    }

    for ase in prepared.iter().rev() {
        if let Err(error) =
            disable_and_release(stack, tracker, ase, next)
        {
            errors.push(format!(
                "{:?} ASE cleanup: {error}",
                ase.side
            ));
        }
    }

    if cig_may_exist {
        match stack.control().try_remove_cig(CIG_ID) {
            Ok(()) => {
                if let Err(error) =
                    wait_cig_removed(stack, tracker)
                {
                    errors.push(error);
                }
            }
            Err(error) => errors.push(format!(
                "Remove CIG submit: {error:?}"
            )),
        }
    }

    errors
}

fn run_iso_probe(
    stack: &HciStack,
    tracker: &mut BondSmokeTracker,
    members: &[BondSmokeMemberResult],
    frames: &[IsoTransportFrame],
) -> Result<IsoSmokeProbeResult, String> {
    let mut next = 1u64;
    let mut prepared = Vec::with_capacity(2);
    let mut cig_may_exist = false;
    let mut cis_handles = Vec::<u16>::new();
    let mut paths_may_exist = Vec::<u16>::new();

    let stage = (|| -> Result<IsoSmokeProbeResult, String> {
        for member in members {
            prepared.push(prepare_member(
                stack,
                tracker,
                member,
                &mut next,
            )?);
        }
        if prepared.len() != 2 {
            return Err(format!(
                "ISO smoke requires exactly two prepared ASEs; got {}",
                prepared.len()
            ));
        }
        let left_count = prepared
            .iter()
            .filter(|ase| ase.side == AudioSide::Left)
            .count();
        let right_count = prepared
            .iter()
            .filter(|ase| ase.side == AudioSide::Right)
            .count();
        if left_count != 1 || right_count != 1 {
            return Err(format!(
                "physical side mapping must be 1L+1R; got left={left_count} right={right_count}"
            ));
        }
        prepared.sort_by_key(|ase| match ase.side {
            AudioSide::Left => 0,
            AudioSide::Right => 1,
        });
        let left = prepared[0].clone();
        let right = prepared[1].clone();

        config_codec(stack, tracker, &left, &mut next)?;
        config_codec(stack, tracker, &right, &mut next)?;

        stack
            .control()
            .try_configure_cig(smoke_cig_config())
            .map_err(|error| {
                format!("Set CIG submit: {error:?}")
            })?;
        cig_may_exist = true;
        let _allocation = wait_cig_ready(stack, tracker)?;

        config_qos(stack, tracker, &left, &mut next)?;
        config_qos(stack, tracker, &right, &mut next)?;
        enable(stack, tracker, &left, &mut next)?;
        enable(stack, tracker, &right, &mut next)?;

        stack
            .control()
            .try_create_cis(CisCreateRequest {
                cig_id: CIG_ID,
                bindings: vec![
                    CisAclBinding {
                        cis_id: LEFT_CIS_ID,
                        acl: left.connection,
                    },
                    CisAclBinding {
                        cis_id: RIGHT_CIS_ID,
                        acl: right.connection,
                    },
                ],
            })
            .map_err(|error| {
                format!("Create CIS submit: {error:?}")
            })?;

        let established = match wait_cis_ready(stack, tracker) {
            Ok(established) => established,
            Err((detail, partial)) => {
                cis_handles = partial
                    .iter()
                    .map(|entry| entry.link.connection_handle)
                    .collect();
                return Err(detail);
            }
        };
        cis_handles = established
            .iter()
            .map(|entry| entry.link.connection_handle)
            .collect();

        let handle_for = |cis_id| {
            established
                .iter()
                .find(|entry| entry.cis_id == cis_id)
                .map(|entry| entry.link.connection_handle)
                .ok_or_else(|| {
                    format!("Create CIS result omitted CIS {cis_id}")
                })
        };
        let left_handle = handle_for(LEFT_CIS_ID)?;
        let right_handle = handle_for(RIGHT_CIS_ID)?;
        if left_handle == right_handle {
            return Err("left/right CIS handles are identical".into());
        }

        verify_post_cis_ase(
            stack,
            tracker,
            &left,
            &mut next,
        )?;
        verify_post_cis_ase(
            stack,
            tracker,
            &right,
            &mut next,
        )?;

        for handle in [left_handle, right_handle] {
            stack
                .control()
                .try_setup_iso_path(
                    IsoDataPathConfig::hci_input(handle),
                )
                .map_err(|error| {
                    format!(
                        "Setup ISO Data Path 0x{handle:04X} submit: {error:?}"
                    )
                })?;
            paths_may_exist.push(handle);
            wait_iso_path_setup(stack, tracker, handle)?;
        }

        let audit_before = read_iso_audit(stack, tracker)?;
        let iso_tx = stack.iso_tx();
        let started = Instant::now();
        for (index, frame) in frames.iter().enumerate() {
            let target = started
                + Duration::from_millis(
                    (index as u64).saturating_mul(10),
                );
            if let Some(delay) = target.checked_duration_since(
                Instant::now(),
            ) {
                thread::sleep(delay);
            }

            iso_tx
                .try_submit_group(IsoSduGroupRequest {
                    sdus: vec![
                        IsoSduRequest {
                            connection_handle: left_handle,
                            payload: frame.left.clone(),
                        },
                        IsoSduRequest {
                            connection_handle: right_handle,
                            payload: frame.right.clone(),
                        },
                    ],
                })
                .map_err(|error| {
                    format!(
                        "ISO data lane backpressure at interval {index}: {error:?}"
                    )
                })?;
        }

        let expected_packets =
            u64::try_from(frames.len() * 2).unwrap_or(u64::MAX);
        let submitted = wait_submitted_count(
            stack,
            tracker,
            &audit_before,
            expected_packets,
        )?;
        let left_submitted = submitted
            .submitted_per_handle
            .get(&left_handle)
            .copied()
            .unwrap_or(0)
            .saturating_sub(
                audit_before
                    .submitted_per_handle
                    .get(&left_handle)
                    .copied()
                    .unwrap_or(0),
            );
        let right_submitted = submitted
            .submitted_per_handle
            .get(&right_handle)
            .copied()
            .unwrap_or(0)
            .saturating_sub(
                audit_before
                    .submitted_per_handle
                    .get(&right_handle)
                    .copied()
                    .unwrap_or(0),
            );
        if left_submitted != frames.len() as u64
            || right_submitted != frames.len() as u64
        {
            return Err(format!(
                "ISO Host submitted counts are not stereo-exact: left={left_submitted} right={right_submitted} expected={}",
                frames.len()
            ));
        }

        let drain = IsoDrainRequest {
            connection_handles: vec![left_handle, right_handle],
        };
        stack
            .control()
            .try_begin_iso_drain(drain.clone())
            .map_err(|error| {
                format!("Begin ISO drain submit: {error:?}")
            })?;
        wait_iso_drained(stack, tracker, &drain)?;

        let audit_after = read_iso_audit(stack, tracker)?;
        let submitted_delta = audit_after
            .total_submitted
            .saturating_sub(audit_before.total_submitted);
        let completed_delta = audit_after
            .total_completed
            .saturating_sub(audit_before.total_completed);
        if audit_after.in_flight != 0
            || submitted_delta != expected_packets
            || completed_delta != expected_packets
        {
            return Err(format!(
                "ISO drain audit mismatch: submitted_delta={submitted_delta} completed_delta={completed_delta} in_flight={} expected={expected_packets}",
                audit_after.in_flight
            ));
        }

        Ok(IsoSmokeProbeResult {
            left_cis_handle: left_handle,
            right_cis_handle: right_handle,
            frames_submitted: frames.len(),
            audit_before,
            audit_after,
        })
    })();

    let cleanup = cleanup_iso_probe(
        stack,
        tracker,
        &prepared,
        &mut next,
        &cis_handles,
        &paths_may_exist,
        cig_may_exist,
    );

    match stage {
        Ok(result) if cleanup.is_empty() => Ok(result),
        Ok(_) => Err(format!(
            "ISO data probe passed but cleanup UNCONFIRMED: {}",
            cleanup.join("; ")
        )),
        Err(error) if cleanup.is_empty() => Err(format!(
            "ISO data probe failed; rollback confirmed: {error}"
        )),
        Err(error) => Err(format!(
            "ISO data probe failed: {error}; cleanup UNCONFIRMED: {}",
            cleanup.join("; ")
        )),
    }
}

#[cfg(windows)]
pub fn execute_iso_smoke(
    config: IsoSmokeConfig,
) -> Result<IsoSmokeResult, IsoSmokeExecuteError> {
    let frames = prepare_transport_frames(config.frames)
        .map_err(|detail| {
            IsoSmokeExecuteError::Bond(
                BondSmokeExecuteError::Preflight(detail),
            )
        })?;

    let mut session =
        open_dual_encrypted_bond_session_for_iso(config.bond)
            .map_err(IsoSmokeExecuteError::Bond)?;
    let members = session.members.clone();
    let probe = run_iso_probe(
        &session.stack,
        &mut session.tracker,
        &members,
        &frames,
    );
    let finish = session.finish();

    match (probe, finish) {
        (Ok(probe), Ok(bond)) => {
            Ok(IsoSmokeResult { bond, probe })
        }
        (Ok(_), Err(error)) => {
            Err(IsoSmokeExecuteError::Bond(error))
        }
        (Err(detail), Ok(bond)) => {
            Err(IsoSmokeExecuteError::Probe {
                detail,
                cleanup: bond.cleanup,
                bond_unchanged: bond.bond_unchanged,
                exit: bond.exit,
            })
        }
        (Err(detail), Err(cleanup)) => {
            Err(IsoSmokeExecuteError::ProbeAndCleanup {
                detail,
                cleanup: Box::new(cleanup),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> Vec<String> {
        vec![
            "--sfi".into(),
            "a.sfi".into(),
            "--ddc".into(),
            "a.ddc".into(),
            "--bond-root".into(),
            "bonding".into(),
            "--name".into(),
            "Galaxy Buds3 Pro (2F23)".into(),
        ]
    }

    #[test]
    fn parser_requires_iso_specific_hil_consent() {
        assert!(parse_iso_smoke_args(&base_args()).is_err());

        let mut args = base_args();
        args.push("--confirm-iso-hil".into());
        let config = parse_iso_smoke_args(&args).unwrap();
        assert_eq!(config.frames, DEFAULT_FRAMES);
    }

    #[test]
    fn parser_accepts_bounded_frame_count() {
        let mut args = base_args();
        args.extend([
            "--frames".into(),
            "25".into(),
            "--confirm-iso-hil".into(),
        ]);
        assert_eq!(parse_iso_smoke_args(&args).unwrap().frames, 25);

        let mut invalid = base_args();
        invalid.extend([
            "--frames".into(),
            "501".into(),
            "--confirm-iso-hil".into(),
        ]);
        assert!(parse_iso_smoke_args(&invalid).is_err());
    }

    #[test]
    fn transport_frames_are_dual_opaque_100_byte_sdus() {
        let frames = prepare_transport_frames(3).unwrap();
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|frame| {
            frame.left.len() == OCTETS_PER_SDU
                && frame.right.len() == OCTETS_PER_SDU
        }));
        assert_ne!(frames[1].left, frames[1].right);
    }

    #[test]
    fn audio_location_selects_one_physical_side() {
        assert_eq!(
            classify_location(AudioLocation::FRONT_LEFT.0).unwrap(),
            (AudioSide::Left, AudioLocation::FRONT_LEFT)
        );
        assert_eq!(
            classify_location(AudioLocation::FRONT_RIGHT.0).unwrap(),
            (AudioSide::Right, AudioLocation::FRONT_RIGHT)
        );
        assert!(
            classify_location(
                AudioLocation::FRONT_LEFT.0
                    | AudioLocation::FRONT_RIGHT.0
            )
            .is_err()
        );
    }

    #[test]
    fn hci_smoke_cig_is_dual_unidirectional_10ms_100b() {
        let config = cig_config();
        assert_eq!(config.cig_id, CIG_ID);
        assert_eq!(config.sdu_interval_c_to_p, SDU_INTERVAL_US);
        assert_eq!(config.cis.len(), 2);
        assert!(config.cis.iter().all(|cis| {
            cis.max_sdu_c_to_p == OCTETS_PER_SDU as u16
                && cis.max_sdu_p_to_c == 0
                && cis.phy_c_to_p == PHY_2M
                && cis.rtn_c_to_p == RTN
                && cis.rtn_p_to_c == 0
        }));
    }

    #[test]
    fn codec_and_qos_smoke_parameters_match_sealed_policy() {
        let codec = codec_configuration(AudioLocation::FRONT_LEFT);
        let parsed =
            CodecSpecificConfiguration::from_bytes(&codec).unwrap();
        assert_eq!(
            parsed.sampling_frequency,
            Some(SamplingFrequency::FREQ_48000)
        );
        assert_eq!(
            parsed.frame_duration,
            Some(FrameDuration::DURATION_10000_US)
        );
        assert_eq!(
            parsed.octets_per_codec_frame,
            Some(OCTETS_PER_SDU as u16)
        );
    }
}
