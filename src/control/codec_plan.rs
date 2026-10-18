use std::collections::BTreeSet;

use bumble_hci::CodingFormat;
use bumble_profiles::{
    ascs::{AseState, ConfigCodecParameters},
    bap::{
        AudioLocation, CodecSpecificCapabilities,
        CodecSpecificConfiguration, ContextType, FrameDuration,
        SamplingFrequency,
    },
    pacs::PacCodecCapabilities,
};

use crate::{
    architecture::{
        control::LeAudioFormat,
        types::AudioSide,
    },
    hci_host::connection::ConnectionToken,
};

use super::audio_snapshot::{
    AudioMemberSnapshot, AudioSetSnapshot, SinkAseSnapshot,
};

pub const FIRST_STREAM_FORMAT: LeAudioFormat = LeAudioFormat {
    sample_rate_hz: 48_000,
    frame_duration_us: 10_000,
    octets_per_sdu: 100,
};
pub const FIRST_STREAM_CONTEXT: ContextType = ContextType::MEDIA;
pub const FIRST_STREAM_TARGET_LATENCY: u8 = 0x02;
pub const FIRST_STREAM_TARGET_PHY: u8 = 0x01;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedSink {
    pub side: AudioSide,
    pub member_index: usize,
    pub connection: ConnectionToken,
    pub ase_id: u8,
    pub ase_value_handle: u16,
    pub ascs_control_point_handle: u16,
    pub audio_location: AudioLocation,
    pub codec_id: CodingFormat,
    pub codec_specific_configuration: CodecSpecificConfiguration,
}

impl PlannedSink {
    pub fn config_codec_parameters(&self) -> ConfigCodecParameters {
        ConfigCodecParameters {
            ase_id: self.ase_id,
            target_latency: FIRST_STREAM_TARGET_LATENCY,
            target_phy: FIRST_STREAM_TARGET_PHY,
            codec_id: self.codec_id,
            codec_specific_configuration:
                self.codec_specific_configuration.to_bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StereoCodecPlan {
    pub format: LeAudioFormat,
    pub context: ContextType,
    pub left: PlannedSink,
    pub right: PlannedSink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecPlanError {
    AudioSet(String),
    ContextUnavailable {
        member_index: usize,
        context: u16,
    },
    NoLc3SinkPac {
        member_index: usize,
    },
    Lc3PolicyUnsupported {
        member_index: usize,
    },
    MissingLeftRightLocation {
        member_index: usize,
        raw: u32,
    },
    MixedLeftRightLocation {
        member_index: usize,
        raw: u32,
    },
    DuplicateSide(AudioSide),
    MissingSide(AudioSide),
    InvalidAseSet {
        member_index: usize,
        detail: String,
    },
    NoIdleSinkAse {
        member_index: usize,
    },
    InvalidAseHandle {
        member_index: usize,
    },
}

pub fn plan_first_stream(
    snapshot: &AudioSetSnapshot,
) -> Result<StereoCodecPlan, CodecPlanError> {
    snapshot
        .validate_dual_sink()
        .map_err(CodecPlanError::AudioSet)?;

    let frequency = SamplingFrequency::from_hz(
        FIRST_STREAM_FORMAT.sample_rate_hz,
    )
    .expect("frozen 48 kHz policy must be valid");
    let duration = FrameDuration::DURATION_10000_US;

    let mut left = None;
    let mut right = None;

    for member in &snapshot.members {
        let (side, allocation) =
            member_side_and_allocation(member)?;
        let planned = plan_member(
            member,
            side,
            allocation,
            frequency,
            duration,
        )?;

        let slot = match side {
            AudioSide::Left => &mut left,
            AudioSide::Right => &mut right,
        };
        if slot.replace(planned).is_some() {
            return Err(CodecPlanError::DuplicateSide(side));
        }
    }

    Ok(StereoCodecPlan {
        format: FIRST_STREAM_FORMAT,
        context: FIRST_STREAM_CONTEXT,
        left: left.ok_or(CodecPlanError::MissingSide(
            AudioSide::Left,
        ))?,
        right: right.ok_or(CodecPlanError::MissingSide(
            AudioSide::Right,
        ))?,
    })
}

fn member_side_and_allocation(
    member: &AudioMemberSnapshot,
) -> Result<(AudioSide, AudioLocation), CodecPlanError> {
    let raw = member.pacs.sink_audio_location.0;
    let left = raw & LEFT_LOCATIONS;
    let right = raw & RIGHT_LOCATIONS;

    let (side, bits) = match (left != 0, right != 0) {
        (true, false) => (AudioSide::Left, left),
        (false, true) => (AudioSide::Right, right),
        (true, true) => {
            return Err(CodecPlanError::MixedLeftRightLocation {
                member_index: member.member_index,
                raw,
            })
        }
        (false, false) => {
            return Err(CodecPlanError::MissingLeftRightLocation {
                member_index: member.member_index,
                raw,
            })
        }
    };

    Ok((
        side,
        AudioLocation(1u32 << bits.trailing_zeros()),
    ))
}

fn plan_member(
    member: &AudioMemberSnapshot,
    side: AudioSide,
    allocation: AudioLocation,
    frequency: SamplingFrequency,
    duration: FrameDuration,
) -> Result<PlannedSink, CodecPlanError> {
    let context = FIRST_STREAM_CONTEXT.0;
    if member.pacs.supported_contexts.sink.0 & context != context
        || member.pacs.available_contexts.sink.0 & context != context
    {
        return Err(CodecPlanError::ContextUnavailable {
            member_index: member.member_index,
            context,
        });
    }

    let mut has_lc3 = false;
    let mut matching_lc3 = false;
    for record in &member.pacs.sink_pac_records {
        if record.coding_format != CodingFormat::LC3 {
            continue;
        }
        has_lc3 = true;
        if let PacCodecCapabilities::Standard(capability) =
            &record.codec_specific_capabilities
        {
            if supports_policy(capability, frequency, duration) {
                matching_lc3 = true;
                break;
            }
        }
    }

    if !has_lc3 {
        return Err(CodecPlanError::NoLc3SinkPac {
            member_index: member.member_index,
        });
    }
    if !matching_lc3 {
        return Err(CodecPlanError::Lc3PolicyUnsupported {
            member_index: member.member_index,
        });
    }

    let ase = select_idle_ase(member)?;

    Ok(PlannedSink {
        side,
        member_index: member.member_index,
        connection: member.connection,
        ase_id: ase.status.ase_id,
        ase_value_handle: ase.value_handle,
        ascs_control_point_handle:
            member.ascs.control_point_handle,
        audio_location: allocation,
        codec_id: CodingFormat::LC3,
        codec_specific_configuration:
            CodecSpecificConfiguration {
                sampling_frequency: Some(frequency),
                frame_duration: Some(duration),
                audio_channel_allocation: Some(allocation),
                octets_per_codec_frame: Some(
                    FIRST_STREAM_FORMAT.octets_per_sdu,
                ),
                codec_frames_per_sdu: Some(1),
            },
    })
}

fn select_idle_ase(
    member: &AudioMemberSnapshot,
) -> Result<&SinkAseSnapshot, CodecPlanError> {
    let mut ids = BTreeSet::new();
    for ase in &member.ascs.sink_ases {
        if ase.status.ase_id == 0
            || !ids.insert(ase.status.ase_id)
        {
            return Err(CodecPlanError::InvalidAseSet {
                member_index: member.member_index,
                detail: format!(
                    "invalid or duplicate Sink ASE ID {}",
                    ase.status.ase_id
                ),
            });
        }
    }

    let ase = member
        .ascs
        .sink_ases
        .iter()
        .filter(|ase| ase.status.state == AseState::IDLE)
        .min_by_key(|ase| ase.status.ase_id)
        .ok_or(CodecPlanError::NoIdleSinkAse {
            member_index: member.member_index,
        })?;

    if ase.value_handle == 0
        || member.ascs.control_point_handle == 0
    {
        return Err(CodecPlanError::InvalidAseHandle {
            member_index: member.member_index,
        });
    }

    Ok(ase)
}

fn supports_policy(
    capability: &CodecSpecificCapabilities,
    frequency: SamplingFrequency,
    duration: FrameDuration,
) -> bool {
    let frequency_bit = 1u16 << (frequency.0 - 1);
    let duration_bit = 1u8 << duration.0;

    capability.supported_sampling_frequencies.0 & frequency_bit != 0
        && capability.supported_frame_durations.0 & duration_bit != 0
        && capability.supported_audio_channel_count.contains(&1)
        && capability.min_octets_per_codec_frame
            <= FIRST_STREAM_FORMAT.octets_per_sdu
        && FIRST_STREAM_FORMAT.octets_per_sdu
            <= capability.max_octets_per_codec_frame
        && capability.supported_max_codec_frames_per_sdu >= 1
}

#[cfg(test)]
mod tests {
    use bumble_profiles::{
        ascs::AseStatusDetails,
        bap::{
            CodecSpecificCapabilities, SupportedFrameDuration,
            SupportedSamplingFrequency,
        },
        le_audio::Metadata,
        pacs::{AudioContexts, PacRecord},
    };

    use super::*;
    use crate::control::audio_snapshot::{
        AscsSinkSnapshot, CasSnapshot, GattServiceRange,
        PacsSinkSnapshot, SinkAseSnapshot,
    };

    fn service() -> GattServiceRange {
        GattServiceRange {
            start_handle: 1,
            end_handle: 20,
        }
    }

    fn pac() -> PacRecord {
        PacRecord {
            coding_format: CodingFormat::LC3,
            codec_specific_capabilities:
                PacCodecCapabilities::Standard(
                    CodecSpecificCapabilities {
                        supported_sampling_frequencies:
                            SupportedSamplingFrequency::FREQ_48000,
                        supported_frame_durations:
                            SupportedFrameDuration::
                                DURATION_10000_US_SUPPORTED,
                        supported_audio_channel_count: vec![1],
                        min_octets_per_codec_frame: 26,
                        max_octets_per_codec_frame: 155,
                        supported_max_codec_frames_per_sdu: 1,
                    },
                ),
            metadata: Metadata::default(),
        }
    }

    fn ase(id: u8, handle: u16) -> SinkAseSnapshot {
        SinkAseSnapshot {
            declaration_handle: handle - 1,
            value_handle: handle,
            end_group_handle: handle + 2,
            properties: 0x12,
            status: bumble_profiles::ascs::AseStatus {
                ase_id: id,
                state: AseState::IDLE,
                details: AseStatusDetails::None,
            },
        }
    }

    fn member(
        member_index: usize,
        location: AudioLocation,
    ) -> AudioMemberSnapshot {
        let contexts = AudioContexts {
            sink: ContextType::MEDIA,
            source: ContextType::PROHIBITED,
        };
        AudioMemberSnapshot {
            member_index,
            connection: ConnectionToken {
                connection_handle: 0x0800 + member_index as u16,
                generation: member_index as u64 + 1,
            },
            mtu: 247,
            pacs: PacsSinkSnapshot {
                service: service(),
                sink_pac_handle: 2,
                sink_audio_location_handle: 3,
                available_contexts_handle: 4,
                supported_contexts_handle: 5,
                sink_pac_records: vec![pac()],
                sink_audio_location: location,
                available_contexts: contexts,
                supported_contexts: contexts,
            },
            ascs: AscsSinkSnapshot {
                service: service(),
                // Deliberately reversed: policy must choose ASE ID 1.
                sink_ases: vec![ase(2, 0x49), ase(1, 0x46)],
                control_point_handle: 0x43,
            },
            cas: CasSnapshot {
                service: service(),
                included_csis: service(),
            },
        }
    }

    #[test]
    fn planner_maps_physical_side_and_lowest_idle_ase() {
        let snapshot = AudioSetSnapshot {
            // Deliberately right first: member order is not side.
            members: vec![
                member(0, AudioLocation::FRONT_RIGHT),
                member(1, AudioLocation::FRONT_LEFT),
            ],
        };

        let plan = plan_first_stream(&snapshot).unwrap();
        assert_eq!(plan.format, FIRST_STREAM_FORMAT);
        assert_eq!(plan.context, ContextType::MEDIA);

        assert_eq!(plan.left.side, AudioSide::Left);
        assert_eq!(plan.left.member_index, 1);
        assert_eq!(plan.left.ase_id, 1);
        assert_eq!(plan.left.ase_value_handle, 0x46);
        assert_eq!(
            plan.left.audio_location,
            AudioLocation::FRONT_LEFT
        );

        assert_eq!(plan.right.side, AudioSide::Right);
        assert_eq!(plan.right.member_index, 0);
        assert_eq!(plan.right.ase_id, 1);

        let params = plan.left.config_codec_parameters();
        assert_eq!(params.target_latency, 0x02);
        assert_eq!(params.target_phy, 0x01);
        assert_eq!(params.codec_id, CodingFormat::LC3);
        assert_eq!(
            params.codec_specific_configuration,
            plan.left.codec_specific_configuration.to_bytes()
        );
    }

    #[test]
    fn planner_rejects_duplicate_physical_side() {
        let snapshot = AudioSetSnapshot {
            members: vec![
                member(0, AudioLocation::FRONT_LEFT),
                member(1, AudioLocation::BACK_LEFT),
            ],
        };
        assert_eq!(
            plan_first_stream(&snapshot),
            Err(CodecPlanError::DuplicateSide(AudioSide::Left))
        );
    }

    #[test]
    fn planner_rejects_missing_media_context() {
        let mut snapshot = AudioSetSnapshot {
            members: vec![
                member(0, AudioLocation::FRONT_LEFT),
                member(1, AudioLocation::FRONT_RIGHT),
            ],
        };
        snapshot.members[0].pacs.available_contexts.sink =
            ContextType::GAME;

        assert_eq!(
            plan_first_stream(&snapshot),
            Err(CodecPlanError::ContextUnavailable {
                member_index: 0,
                context: ContextType::MEDIA.0,
            })
        );
    }

    #[test]
    fn planner_rejects_lc3_pac_without_100_byte_support() {
        let mut snapshot = AudioSetSnapshot {
            members: vec![
                member(0, AudioLocation::FRONT_LEFT),
                member(1, AudioLocation::FRONT_RIGHT),
            ],
        };
        let PacCodecCapabilities::Standard(capabilities) =
            &mut snapshot.members[0].pacs.sink_pac_records[0]
                .codec_specific_capabilities
        else {
            unreachable!()
        };
        capabilities.max_octets_per_codec_frame = 80;

        assert_eq!(
            plan_first_stream(&snapshot),
            Err(CodecPlanError::Lc3PolicyUnsupported {
                member_index: 0,
            })
        );
    }
}
