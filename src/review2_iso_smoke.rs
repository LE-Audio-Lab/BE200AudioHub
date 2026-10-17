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
        parse_bond_smoke_args, BondSmokeConfig, BondSmokeMemberResult,
        BondSmokeTracker,
    },
    review2_gatt_smoke::{
        execute_gatt, required_service, ASCS_UUID, PACS_UUID,
    },
    runtime::mailbox::RecvError,
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
