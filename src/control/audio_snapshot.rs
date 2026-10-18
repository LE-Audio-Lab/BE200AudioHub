use bumble_profiles::{
    ascs::AseStatus,
    bap::AudioLocation,
    pacs::{AudioContexts, PacRecord},
};

use crate::hci_host::connection::ConnectionToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GattServiceRange {
    pub start_handle: u16,
    pub end_handle: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GattCharacteristicRange {
    pub declaration_handle: u16,
    pub value_handle: u16,
    pub end_group_handle: u16,
    pub properties: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SinkAseSnapshot {
    pub declaration_handle: u16,
    pub value_handle: u16,
    pub end_group_handle: u16,
    pub properties: u8,
    pub status: AseStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacsSinkSnapshot {
    pub service: GattServiceRange,
    pub sink_pac_handle: u16,
    pub sink_audio_location_handle: u16,
    pub available_contexts_handle: u16,
    pub supported_contexts_handle: u16,
    pub sink_pac_records: Vec<PacRecord>,
    pub sink_audio_location: AudioLocation,
    pub available_contexts: AudioContexts,
    pub supported_contexts: AudioContexts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AscsSinkSnapshot {
    pub service: GattServiceRange,
    pub sink_ases: Vec<SinkAseSnapshot>,
    pub control_point: GattCharacteristicRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CasSnapshot {
    pub service: GattServiceRange,
    pub included_csis: GattServiceRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioMemberSnapshot {
    pub member_index: usize,
    pub connection: ConnectionToken,
    pub mtu: u16,
    pub pacs: PacsSinkSnapshot,
    pub ascs: AscsSinkSnapshot,
    pub cas: CasSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSetSnapshot {
    pub members: Vec<AudioMemberSnapshot>,
}

impl AudioSetSnapshot {
    pub fn validate_dual_sink(&self) -> Result<(), String> {
        if self.members.len() != 2 {
            return Err(format!(
                "audio set requires exactly 2 members, found {}",
                self.members.len()
            ));
        }

        if self.members[0].member_index == self.members[1].member_index {
            return Err("audio set contains duplicate member index".into());
        }

        for member in &self.members {
            if member.pacs.sink_pac_records.is_empty() {
                return Err(format!(
                    "member {} exposes no Sink PAC record",
                    member.member_index
                ));
            }
            if member.ascs.sink_ases.is_empty() {
                return Err(format!(
                    "member {} exposes no Sink ASE",
                    member.member_index
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bumble_hci::CodingFormat;
    use bumble_profiles::{
        ascs::{AseState, AseStatus, AseStatusDetails},
        bap::{
            CodecSpecificCapabilities, ContextType,
            SupportedFrameDuration, SupportedSamplingFrequency,
        },
        le_audio::Metadata,
        pacs::PacCodecCapabilities,
    };

    use super::*;

    fn member(index: usize) -> AudioMemberSnapshot {
        let service = GattServiceRange {
            start_handle: 1,
            end_handle: 10,
        };
        AudioMemberSnapshot {
            member_index: index,
            connection: ConnectionToken {
                connection_handle: 0x0800 + index as u16,
                generation: index as u64 + 1,
            },
            mtu: 247,
            pacs: PacsSinkSnapshot {
                service,
                sink_pac_handle: 2,
                sink_audio_location_handle: 3,
                available_contexts_handle: 4,
                supported_contexts_handle: 5,
                sink_pac_records: vec![PacRecord {
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
                }],
                sink_audio_location: AudioLocation(1 << index),
                available_contexts: AudioContexts {
                    sink: ContextType::MEDIA,
                    source: ContextType::PROHIBITED,
                },
                supported_contexts: AudioContexts {
                    sink: ContextType::MEDIA,
                    source: ContextType::PROHIBITED,
                },
            },
            ascs: AscsSinkSnapshot {
                service,
                sink_ases: vec![SinkAseSnapshot {
                    declaration_handle: 6,
                    value_handle: 7,
                    end_group_handle: 8,
                    properties: 0x12,
                    status: AseStatus {
                        ase_id: 1,
                        state: AseState::IDLE,
                        details: AseStatusDetails::None,
                    },
                }],
                control_point: GattCharacteristicRange {
                    declaration_handle: 8,
                    value_handle: 9,
                    end_group_handle: 10,
                    properties: 0x12,
                },
            },
            cas: CasSnapshot {
                service,
                included_csis: service,
            },
        }
    }

    #[test]
    fn dual_sink_snapshot_requires_two_distinct_populated_members() {
        let valid = AudioSetSnapshot {
            members: vec![member(0), member(1)],
        };
        valid.validate_dual_sink().unwrap();

        let one = AudioSetSnapshot {
            members: vec![member(0)],
        };
        assert!(one.validate_dual_sink().is_err());

        let duplicate = AudioSetSnapshot {
            members: vec![member(0), member(0)],
        };
        assert!(duplicate.validate_dual_sink().is_err());
    }
}
