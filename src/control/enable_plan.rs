use bumble_profiles::{
    ascs::AseMetadataParameters,
    bap::ContextType,
    le_audio::{Metadata, MetadataEntry, MetadataTag},
};

use crate::architecture::types::AudioSide;

use super::{
    codec_plan::StereoCodecPlan,
    iso_plan::StereoIsoPlan,
    qos_plan::StereoQosPlan,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedEnableSink {
    pub side: AudioSide,
    pub member_index: usize,
    pub ase_id: u8,
    pub cig_id: u8,
    pub cis_id: u8,
    pub metadata: Vec<u8>,
}

impl PlannedEnableSink {
    pub fn parameters(&self) -> AseMetadataParameters {
        AseMetadataParameters {
            ase_id: self.ase_id,
            metadata: self.metadata.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StereoEnablePlan {
    pub context: ContextType,
    pub left: PlannedEnableSink,
    pub right: PlannedEnableSink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnablePlanError {
    UnsupportedContext(ContextType),
    QosIdentityMismatch {
        side: AudioSide,
    },
    Encode(String),
}

pub fn plan_first_enable(
    codec: &StereoCodecPlan,
    iso: &StereoIsoPlan,
    qos: &StereoQosPlan,
) -> Result<StereoEnablePlan, EnablePlanError> {
    if codec.context != ContextType::MEDIA {
        return Err(EnablePlanError::UnsupportedContext(
            codec.context,
        ));
    }

    let metadata = Metadata::new(vec![MetadataEntry::new(
        MetadataTag::STREAMING_AUDIO_CONTEXTS,
        codec.context.0.to_le_bytes(),
    )])
    .to_bytes()
    .map_err(|error| EnablePlanError::Encode(error.to_string()))?;

    let left = plan_side(
        AudioSide::Left,
        codec.left.member_index,
        codec.left.ase_id,
        iso.left.cis_id,
        &qos.left,
        iso.cig_id,
        &metadata,
    )?;
    let right = plan_side(
        AudioSide::Right,
        codec.right.member_index,
        codec.right.ase_id,
        iso.right.cis_id,
        &qos.right,
        iso.cig_id,
        &metadata,
    )?;

    Ok(StereoEnablePlan {
        context: codec.context,
        left,
        right,
    })
}

fn plan_side(
    side: AudioSide,
    member_index: usize,
    ase_id: u8,
    cis_id: u8,
    qos: &super::qos_plan::PlannedQosSink,
    cig_id: u8,
    metadata: &[u8],
) -> Result<PlannedEnableSink, EnablePlanError> {
    if qos.side != side
        || qos.member_index != member_index
        || qos.parameters.ase_id != ase_id
        || qos.parameters.cig_id != cig_id
        || qos.parameters.cis_id != cis_id
    {
        return Err(EnablePlanError::QosIdentityMismatch { side });
    }

    Ok(PlannedEnableSink {
        side,
        member_index,
        ase_id,
        cig_id,
        cis_id,
        metadata: metadata.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use bumble_hci::CodingFormat;
    use bumble_profiles::{
        bap::{
            AudioLocation, CodecSpecificConfiguration, ContextType,
            FrameDuration, SamplingFrequency,
        },
        le_audio::Metadata,
    };

    use crate::{
        architecture::{
            control::LeAudioFormat,
            types::AudioSide,
        },
        control::{
            audio_snapshot::GattCharacteristicRange,
            codec_plan::{PlannedSink, StereoCodecPlan},
            iso_plan::plan_first_iso,
            qos_plan::plan_first_qos,
        },
        hci_host::connection::ConnectionToken,
    };

    use super::*;

    fn sink(
        side: AudioSide,
        member_index: usize,
        handle: u16,
        generation: u64,
        location: AudioLocation,
    ) -> PlannedSink {
        PlannedSink {
            side,
            member_index,
            connection: ConnectionToken {
                connection_handle: handle,
                generation,
            },
            ase_id: 1,
            ase_value_handle: 0x46,
            ase_end_group_handle: 0x48,
            ascs_control_point: GattCharacteristicRange {
                declaration_handle: 0x42,
                value_handle: 0x43,
                end_group_handle: 0x45,
                properties: 0x12,
            },
            audio_location: location,
            codec_id: CodingFormat::LC3,
            codec_specific_configuration:
                CodecSpecificConfiguration {
                    sampling_frequency: Some(
                        SamplingFrequency::FREQ_48000,
                    ),
                    frame_duration: Some(
                        FrameDuration::DURATION_10000_US,
                    ),
                    audio_channel_allocation: Some(location),
                    octets_per_codec_frame: Some(100),
                    codec_frames_per_sdu: Some(1),
                },
        }
    }

    fn codec_plan() -> StereoCodecPlan {
        StereoCodecPlan {
            format: LeAudioFormat {
                sample_rate_hz: 48_000,
                frame_duration_us: 10_000,
                octets_per_sdu: 100,
            },
            context: ContextType::MEDIA,
            left: sink(
                AudioSide::Left,
                1,
                0x0801,
                2,
                AudioLocation::FRONT_LEFT,
            ),
            right: sink(
                AudioSide::Right,
                0,
                0x0800,
                1,
                AudioLocation::FRONT_RIGHT,
            ),
        }
    }

    #[test]
    fn first_enable_plan_carries_media_context_and_cis_identity() {
        let codec = codec_plan();
        let iso = plan_first_iso(&codec).unwrap();
        let qos = plan_first_qos(&codec, &iso).unwrap();
        let plan = plan_first_enable(&codec, &iso, &qos).unwrap();

        assert_eq!(plan.context, ContextType::MEDIA);
        assert_eq!(plan.left.ase_id, 1);
        assert_eq!(plan.left.cig_id, 1);
        assert_eq!(plan.left.cis_id, 1);
        assert_eq!(plan.right.cis_id, 2);

        for sink in [&plan.left, &plan.right] {
            let decoded = Metadata::from_bytes(&sink.metadata).unwrap();
            assert!(decoded.entries.iter().any(|entry| {
                entry.tag == MetadataTag::STREAMING_AUDIO_CONTEXTS
                    && entry.data
                        == ContextType::MEDIA.0.to_le_bytes()
            }));
            assert_eq!(
                sink.parameters(),
                AseMetadataParameters {
                    ase_id: sink.ase_id,
                    metadata: sink.metadata.clone(),
                }
            );
        }
    }
}
