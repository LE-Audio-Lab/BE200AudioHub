//! R4.1: deterministic, offline BAP unicast codec planning.
//!
//! Reuses the pinned Bumble-RS BAP/PACS/ASCS types and LC3 LTV encoder.
//! Capability matching follows the checks in AOSP Fluoride's
//! bta/le_audio/le_audio_utils.cc::IsCodecConfigSupported.
//! No transport, GATT, ASCS Control Point, or persistent-state operations.

use crate::snapshot::{AudioMemberSnapshot, AudioSetSnapshot};
use bumble_hci::CodingFormat;
use bumble_profiles::ascs::{AseState, ConfigCodecParameters};
use bumble_profiles::bap::{
    AudioLocation, CodecSpecificCapabilities, CodecSpecificConfiguration, ContextType,
    FrameDuration, SamplingFrequency,
};
use bumble_profiles::pacs::PacCodecCapabilities;
use device_policy::{StreamPlan, StreamTopology};
use std::collections::BTreeSet;

const LEFT_LOCATIONS: u32 = AudioLocation::FRONT_LEFT.0
    | AudioLocation::BACK_LEFT.0
    | AudioLocation::FRONT_LEFT_OF_CENTER.0
    | AudioLocation::SIDE_LEFT.0
    | AudioLocation::TOP_FRONT_LEFT.0
    | AudioLocation::TOP_BACK_LEFT.0
    | AudioLocation::TOP_SIDE_LEFT.0
    | AudioLocation::BOTTOM_FRONT_LEFT.0
    | AudioLocation::FRONT_LEFT_WIDE.0
    | AudioLocation::LEFT_SURROUND.0;
const RIGHT_LOCATIONS: u32 = AudioLocation::FRONT_RIGHT.0
    | AudioLocation::BACK_RIGHT.0
    | AudioLocation::FRONT_RIGHT_OF_CENTER.0
    | AudioLocation::SIDE_RIGHT.0
    | AudioLocation::TOP_FRONT_RIGHT.0
    | AudioLocation::TOP_BACK_RIGHT.0
    | AudioLocation::TOP_SIDE_RIGHT.0
    | AudioLocation::BOTTOM_FRONT_RIGHT.0
    | AudioLocation::FRONT_RIGHT_WIDE.0
    | AudioLocation::RIGHT_SURROUND.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StereoSide {
    Left,
    Right,
}

impl StereoSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedSink {
    pub side: StereoSide,
    pub identity_address: String,
    pub rank: u8,
    pub acl_handle: u16,
    pub ase_id: u8,
    pub ase_value_handle: u16,
    pub ascs_control_point_handle: u16,
    pub audio_location: AudioLocation,
    pub codec_id: CodingFormat,
    pub codec_specific_configuration: CodecSpecificConfiguration,
}

impl PlannedSink {
    /// R4.2 supplies the target latency/PHY after checking controller support.
    /// This just serializes already-validated codec data using Bumble-RS.
    pub fn config_codec_parameters(&self, target_latency: u8, target_phy: u8) -> ConfigCodecParameters {
        ConfigCodecParameters {
            ase_id: self.ase_id,
            target_latency,
            target_phy,
            codec_id: self.codec_id,
            codec_specific_configuration: self.codec_specific_configuration.to_bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecConfigurationPlan {
    pub context: ContextType,
    pub stream: StreamPlan,
    pub left: PlannedSink,
    pub right: PlannedSink,
}

/// Buds3 Pro first-sound candidate; still validated against *both* peer PACs.
/// Policy is separate from device identity, and can be replaced later.
pub fn plan_first_sound(snapshot: &AudioSetSnapshot) -> Result<CodecConfigurationPlan, String> {
    plan_stereo_lc3(
        snapshot,
        StreamPlan::first_sound(StreamTopology::StereoDualMonoAse, 100),
        ContextType::MEDIA,
    )
}

/// Pure policy gate: one mono LC3 Sink ASE on each of two different members.
pub fn plan_stereo_lc3(
    snapshot: &AudioSetSnapshot,
    stream: StreamPlan,
    context: ContextType,
) -> Result<CodecConfigurationPlan, String> {
    snapshot.validate_sink_discovery()?;
    if snapshot.set_size != 2 || snapshot.members.len() != 2 {
        return Err("stereo dual-mono requires exactly two audio-set members".into());
    }
    if stream.topology != StreamTopology::StereoDualMonoAse {
        return Err("R4.1 planner requires StereoDualMonoAse topology".into());
    }
    if context.0 == 0 || stream.octets_per_codec_frame == 0 {
        return Err("codec policy has an empty context or zero octets per frame".into());
    }
    if snapshot.members[0]
        .identity_address
        .eq_ignore_ascii_case(&snapshot.members[1].identity_address)
    {
        return Err("both audio-set members have the same identity".into());
    }

    let frequency = SamplingFrequency::from_hz(stream.sample_rate_hz)
        .map_err(|error| format!("invalid sampling-frequency policy: {error}"))?;
    let duration = match stream.frame_duration_us {
        7_500 => FrameDuration::DURATION_7500_US,
        10_000 => FrameDuration::DURATION_10000_US,
        other => return Err(format!("unsupported LC3 frame-duration policy: {other} us")),
    };

    let mut left = None;
    let mut right = None;
    for member in &snapshot.members {
        let (side, allocation) = member_side_and_allocation(member)?;
        let selected = plan_member(member, side, allocation, context, stream, frequency, duration)?;
        let slot = match side {
            StereoSide::Left => &mut left,
            StereoSide::Right => &mut right,
        };
        if slot.replace(selected).is_some() {
            return Err(format!("multiple members claim {} Sink Audio Location", side.as_str()));
        }
    }

    Ok(CodecConfigurationPlan {
        context,
        stream,
        left: left.ok_or_else(|| "audio set has no left sink member".to_owned())?,
        right: right.ok_or_else(|| "audio set has no right sink member".to_owned())?,
    })
}

fn member_side_and_allocation(
    member: &AudioMemberSnapshot,
) -> Result<(StereoSide, AudioLocation), String> {
    let location = member.pacs.sink_audio_locations.ok_or_else(|| {
        format!("{} has no Sink Audio Location", member.identity_address)
    })?;
    let left = location.0 & LEFT_LOCATIONS;
    let right = location.0 & RIGHT_LOCATIONS;
    let (side, bits) = match (left != 0, right != 0) {
        (true, false) => (StereoSide::Left, left),
        (false, true) => (StereoSide::Right, right),
        (true, true) => {
            return Err(format!(
                "{} reports mixed left/right Sink Audio Location 0x{:08X}",
                member.identity_address, location.0
            ));
        }
        (false, false) => {
            return Err(format!(
                "{} has no left/right Sink Audio Location 0x{:08X}",
                member.identity_address, location.0
            ));
        }
    };
    // Exactly one audio channel per ear. Prefer the lowest assigned side bit
    // (front position when present); do not emit all advertised locations.
    Ok((side, AudioLocation(1u32 << bits.trailing_zeros())))
}

fn supports_policy(
    capability: &CodecSpecificCapabilities,
    stream: StreamPlan,
    frequency: SamplingFrequency,
    duration: FrameDuration,
) -> bool {
    let frequency_bit = 1u16 << (frequency.0 - 1);
    let duration_bit = 1u8 << duration.0;
    capability.supported_sampling_frequencies.0 & frequency_bit != 0
        && capability.supported_frame_durations.0 & duration_bit != 0
        && capability.supported_audio_channel_count.contains(&1)
        && capability.min_octets_per_codec_frame <= stream.octets_per_codec_frame
        && stream.octets_per_codec_frame <= capability.max_octets_per_codec_frame
        && capability.supported_max_codec_frames_per_sdu >= 1
}

fn plan_member(
    member: &AudioMemberSnapshot,
    side: StereoSide,
    allocation: AudioLocation,
    context: ContextType,
    stream: StreamPlan,
    frequency: SamplingFrequency,
    duration: FrameDuration,
) -> Result<PlannedSink, String> {
    let label = &member.identity_address;
    if member.pacs.supported_contexts.sink.0 & context.0 != context.0
        || member.pacs.available_contexts.sink.0 & context.0 != context.0
    {
        return Err(format!("{label}: requested sink context 0x{:04X} is not supported and available", context.0));
    }

    let mut has_lc3 = false;
    let mut matching_lc3 = false;
    for record in &member.pacs.sink_pac_records {
        if record.coding_format != CodingFormat::LC3 {
            continue;
        }
        has_lc3 = true;
        if let PacCodecCapabilities::Standard(capabilities) = &record.codec_specific_capabilities {
            if supports_policy(capabilities, stream, frequency, duration) {
                matching_lc3 = true;
                break;
            }
        }
    }
    if !matching_lc3 {
        return Err(if has_lc3 {
            format!("{label}: no LC3 Sink PAC supports {}Hz/{}us/mono/{}octets/1frame-per-SDU",
                stream.sample_rate_hz, stream.frame_duration_us, stream.octets_per_codec_frame)
        } else {
            format!("{label}: no LC3 Sink PAC record")
        });
    }

    let mut ase_ids = BTreeSet::new();
    for ase in &member.ascs.sink_ases {
        if ase.status.ase_id == 0 || !ase_ids.insert(ase.status.ase_id) {
            return Err(format!("{label}: invalid or duplicate Sink ASE ID {}", ase.status.ase_id));
        }
    }
    let ase = member
        .ascs
        .sink_ases
        .iter()
        .filter(|ase| ase.status.state == AseState::IDLE)
        .min_by_key(|ase| ase.status.ase_id)
        .ok_or_else(|| format!("{label}: no Idle Sink ASE"))?;
    if ase.value_handle == 0 || member.ascs.control_point_handle == 0 {
        return Err(format!("{label}: invalid Sink ASE/ASCS Control Point GATT handle"));
    }

    let config = CodecSpecificConfiguration {
        sampling_frequency: Some(frequency),
        frame_duration: Some(duration),
        audio_channel_allocation: Some(allocation),
        octets_per_codec_frame: Some(stream.octets_per_codec_frame),
        codec_frames_per_sdu: Some(1),
    };
    Ok(PlannedSink {
        side,
        identity_address: member.identity_address.clone(),
        rank: member.rank,
        acl_handle: member.connection_handle,
        ase_id: ase.status.ase_id,
        ase_value_handle: ase.value_handle,
        ascs_control_point_handle: member.ascs.control_point_handle,
        audio_location: allocation,
        codec_id: CodingFormat::LC3,
        codec_specific_configuration: config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{AseSnapshot, AscsSnapshot, GattServiceRange, PacsSnapshot};
    use bumble_profiles::ascs::{AseStatus, AseStatusDetails};
    use bumble_profiles::bap::{
        CodecSpecificCapabilities, SupportedFrameDuration, SupportedSamplingFrequency,
    };
    use bumble_profiles::le_audio::Metadata;
    use bumble_profiles::pacs::{AudioContexts, PacRecord};

    fn service(start: u16, end: u16) -> GattServiceRange {
        GattServiceRange { start_handle: start, end_handle: end }
    }

    fn member(side: StereoSide) -> AudioMemberSnapshot {
        let (identity, rank, acl, location) = match side {
            StereoSide::Left => ("E8:C9:13:F6:5C:E0", 2, 0x0801, AudioLocation::FRONT_LEFT),
            StereoSide::Right => ("E8:C9:13:EF:2F:23", 1, 0x0800, AudioLocation::FRONT_RIGHT),
        };
        let pac = PacRecord {
            coding_format: CodingFormat::LC3,
            codec_specific_capabilities: PacCodecCapabilities::Standard(CodecSpecificCapabilities {
                supported_sampling_frequencies: SupportedSamplingFrequency(0x00B4),
                supported_frame_durations: SupportedFrameDuration(0x03),
                supported_audio_channel_count: vec![1],
                min_octets_per_codec_frame: 26,
                max_octets_per_codec_frame: 155,
                supported_max_codec_frames_per_sdu: 1,
            }),
            metadata: Metadata::default(),
        };
        let ase = |id, handle| AseSnapshot {
            declaration_handle: handle - 1,
            value_handle: handle,
            end_group_handle: handle + 2,
            properties: 0x12,
            status: AseStatus { ase_id: id, state: AseState::IDLE, details: AseStatusDetails::None },
        };
        let contexts = AudioContexts { sink: ContextType(0x0F7F), source: ContextType(0x026F) };
        AudioMemberSnapshot {
            identity_address: identity.into(),
            rank,
            current_address: "00:00:00:00:00:01".into(),
            current_address_type: 1,
            connection_handle: acl,
            pacs: PacsSnapshot {
                service: service(0x004E, 0x0060),
                sink_pac_handle: Some(0x004F),
                source_pac_handle: None,
                sink_audio_location_handle: Some(0x0050),
                source_audio_location_handle: None,
                available_contexts_handle: 0x0051,
                supported_contexts_handle: 0x0052,
                sink_pac_records: vec![pac],
                source_pac_records: vec![],
                sink_audio_locations: Some(location),
                source_audio_locations: None,
                available_contexts: contexts,
                supported_contexts: contexts,
            },
            ascs: AscsSnapshot {
                service: service(0x0041, 0x004D),
                sink_ases: vec![ase(2, 0x0049), ase(1, 0x0046)],
                source_ases: vec![],
                control_point_handle: 0x0043,
            },
            cap: None,
        }
    }

    fn stereo() -> AudioSetSnapshot {
        // Deliberately reverse the member order and ASE order.
        AudioSetSnapshot {
            target_name: "test set".into(),
            set_size: 2,
            members: vec![member(StereoSide::Right), member(StereoSide::Left)],
        }
    }

    #[test]
    fn plans_real_buds3pro_capabilities_and_ascs_ltv() {
        let plan = plan_first_sound(&stereo()).unwrap();
        assert_eq!(plan.left.side, StereoSide::Left);
        assert_eq!(plan.left.rank, 2);
        assert_eq!(plan.right.rank, 1);
        assert_eq!(plan.left.ase_id, 1);
        assert_eq!(plan.right.ase_value_handle, 0x0046);
        assert_eq!(plan.left.audio_location, AudioLocation::FRONT_LEFT);
        assert_eq!(plan.right.audio_location, AudioLocation::FRONT_RIGHT);
        assert_eq!(plan.stream.sample_rate_hz, 48_000);
        assert_eq!(plan.context, ContextType::MEDIA);
        assert_eq!(plan.left.codec_id, CodingFormat::LC3);
        let bytes = plan.left.codec_specific_configuration.to_bytes();
        assert_eq!(bytes, vec![2, 1, 8, 2, 2, 1, 5, 3, 1, 0, 0, 0, 3, 4, 100, 0, 2, 5, 1]);
        assert_eq!(CodecSpecificConfiguration::from_bytes(&bytes).unwrap(),
            plan.left.codec_specific_configuration);
        let params = plan.left.config_codec_parameters(2, 2);
        assert_eq!(params.ase_id, 1);
        assert_eq!(params.codec_id, CodingFormat::LC3);
        assert_eq!(params.codec_specific_configuration, bytes);
    }

    #[test]
    fn is_deterministic_when_members_reordered() {
        let mut snapshot = stereo();
        let expected = plan_first_sound(&snapshot).unwrap();
        snapshot.members.reverse();
        for member in &mut snapshot.members { member.ascs.sink_ases.reverse(); }
        assert_eq!(plan_first_sound(&snapshot).unwrap(), expected);
    }

    #[test]
    fn rejects_unsupported_lc3_capabilities() {
        let mutations: Vec<Box<dyn Fn(&mut AudioSetSnapshot)>> = vec![
            Box::new(|s| s.members[0].pacs.sink_pac_records[0].coding_format = CodingFormat::TRANSPARENT),
            Box::new(|s| {
                if let PacCodecCapabilities::Standard(c) =
                    &mut s.members[0].pacs.sink_pac_records[0].codec_specific_capabilities {
                    c.supported_sampling_frequencies = SupportedSamplingFrequency::FREQ_16000;
                }
            }),
            Box::new(|s| {
                if let PacCodecCapabilities::Standard(c) =
                    &mut s.members[0].pacs.sink_pac_records[0].codec_specific_capabilities {
                    c.supported_frame_durations = SupportedFrameDuration::DURATION_7500_US_SUPPORTED;
                }
            }),
            Box::new(|s| {
                if let PacCodecCapabilities::Standard(c) =
                    &mut s.members[0].pacs.sink_pac_records[0].codec_specific_capabilities {
                    c.supported_audio_channel_count = vec![2];
                }
            }),
            Box::new(|s| {
                if let PacCodecCapabilities::Standard(c) =
                    &mut s.members[0].pacs.sink_pac_records[0].codec_specific_capabilities {
                    c.max_octets_per_codec_frame = 90;
                }
            }),
            Box::new(|s| {
                if let PacCodecCapabilities::Standard(c) =
                    &mut s.members[0].pacs.sink_pac_records[0].codec_specific_capabilities {
                    c.supported_max_codec_frames_per_sdu = 0;
                }
            }),
        ];
        for mutate in mutations {
            let mut snapshot = stereo();
            mutate(&mut snapshot);
            assert!(plan_first_sound(&snapshot).is_err());
        }
    }

    #[test]
    fn rejects_bad_context_ase_or_side() {
        let mut snapshot = stereo();
        snapshot.members[0].pacs.available_contexts.sink = ContextType::CONVERSATIONAL;
        assert!(plan_first_sound(&snapshot).unwrap_err().contains("context"));

        let mut snapshot = stereo();
        for ase in &mut snapshot.members[0].ascs.sink_ases { ase.status.state = AseState::STREAMING; }
        assert!(plan_first_sound(&snapshot).unwrap_err().contains("Idle"));

        let mut snapshot = stereo();
        snapshot.members[0].pacs.sink_audio_locations = Some(AudioLocation::FRONT_LEFT);
        assert!(plan_first_sound(&snapshot).unwrap_err().contains("multiple"));

        let mut snapshot = stereo();
        snapshot.members[1].pacs.sink_audio_locations =
            Some(AudioLocation(AudioLocation::FRONT_LEFT.0 | AudioLocation::FRONT_RIGHT.0));
        assert!(plan_first_sound(&snapshot).unwrap_err().contains("mixed"));
    }

    #[test]
    fn selects_one_channel_when_side_advertises_multiple_positions() {
        let mut snapshot = stereo();
        snapshot.members[1].pacs.sink_audio_locations =
            Some(AudioLocation(AudioLocation::FRONT_LEFT.0 | AudioLocation::BACK_LEFT.0));
        assert_eq!(plan_first_sound(&snapshot).unwrap().left.audio_location, AudioLocation::FRONT_LEFT);
    }

    #[test]
    fn rejects_duplicate_identity_and_ase_ids() {
        let mut snapshot = stereo();
        snapshot.members[1].identity_address = snapshot.members[0].identity_address.clone();
        assert!(plan_first_sound(&snapshot).unwrap_err().contains("same identity"));
        let mut snapshot = stereo();
        snapshot.members[0].ascs.sink_ases[1].status.ase_id = 2;
        assert!(plan_first_sound(&snapshot).unwrap_err().contains("duplicate"));
    }
}
