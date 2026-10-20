use bumble_profiles::ascs::{
    AseQosConfiguration, ConfigQosParameters,
};

use crate::plane::l1_control::interface::stream::AudioSide;

use super::{
    codec::StereoCodecPlan,
    iso::{StereoIsoPlan, StreamLatencyProfile},
};

pub const FIRST_PRESENTATION_DELAY_US: u32 = 40_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedQosSink {
    pub side: AudioSide,
    pub member_index: usize,
    pub parameters: ConfigQosParameters,
}

impl PlannedQosSink {
    pub fn expected_readback(&self) -> AseQosConfiguration {
        AseQosConfiguration {
            cig_id: self.parameters.cig_id,
            cis_id: self.parameters.cis_id,
            sdu_interval: self.parameters.sdu_interval,
            framing: self.parameters.framing,
            phy: self.parameters.phy,
            max_sdu: self.parameters.max_sdu,
            retransmission_number:
                self.parameters.retransmission_number,
            max_transport_latency:
                self.parameters.max_transport_latency,
            presentation_delay:
                self.parameters.presentation_delay,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StereoQosPlan {
    pub left: PlannedQosSink,
    pub right: PlannedQosSink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QosPlanError {
    ConnectionMismatch {
        side: AudioSide,
    },
    CisSideMismatch {
        side: AudioSide,
    },
}

pub fn plan_first_qos(
    codec: &StereoCodecPlan,
    iso: &StereoIsoPlan,
) -> Result<StereoQosPlan, QosPlanError> {
    plan_first_qos_with_profile(
        codec,
        iso,
        StreamLatencyProfile::Daily,
    )
}

pub fn plan_first_qos_with_profile(
    codec: &StereoCodecPlan,
    iso: &StereoIsoPlan,
    profile: StreamLatencyProfile,
) -> Result<StereoQosPlan, QosPlanError> {
    let left = plan_side(
        AudioSide::Left,
        &codec.left,
        iso.left,
        iso,
        profile,
    )?;
    let right = plan_side(
        AudioSide::Right,
        &codec.right,
        iso.right,
        iso,
        profile,
    )?;

    Ok(StereoQosPlan { left, right })
}

fn plan_side(
    side: AudioSide,
    sink: &super::codec::PlannedSink,
    cis: super::iso::PlannedCis,
    iso: &StereoIsoPlan,
    profile: StreamLatencyProfile,
) -> Result<PlannedQosSink, QosPlanError> {
    if sink.connection != cis.acl {
        return Err(QosPlanError::ConnectionMismatch { side });
    }
    if cis.side != side {
        return Err(QosPlanError::CisSideMismatch { side });
    }

    Ok(PlannedQosSink {
        side,
        member_index: sink.member_index,
        parameters: ConfigQosParameters {
            ase_id: sink.ase_id,
            cig_id: iso.cig_id,
            cis_id: cis.cis_id,
            sdu_interval: iso.sdu_interval_us,
            framing: 0,
            phy: iso.phy,
            max_sdu: iso.octets_per_sdu,
            retransmission_number: iso.rtn,
            max_transport_latency:
                iso.max_transport_latency_ms,
            presentation_delay: profile.presentation_delay_us(),
        },
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
    };

    use crate::{
        plane::l1_control::interface::stream::{AudioSide, LeAudioFormat},
        plane::{
            l1_control::{
                discovery::snapshot::GattCharacteristicRange,
                planning::{
                    codec::{PlannedSink, StereoCodecPlan},
                    iso::plan_first_iso,
                },
            },
            l1_hci::acl::connection::ConnectionToken,
        },
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
    fn first_qos_plan_matches_real_review2d_hil_policy() {
        let codec = codec_plan();
        let iso = plan_first_iso(&codec).unwrap();
        let qos = plan_first_qos(&codec, &iso).unwrap();

        assert_eq!(qos.left.side, AudioSide::Left);
        assert_eq!(qos.left.member_index, 1);
        assert_eq!(qos.left.parameters.ase_id, 1);
        assert_eq!(qos.left.parameters.cig_id, 1);
        assert_eq!(qos.left.parameters.cis_id, 1);

        assert_eq!(qos.right.side, AudioSide::Right);
        assert_eq!(qos.right.member_index, 0);
        assert_eq!(qos.right.parameters.cis_id, 2);

        for planned in [&qos.left, &qos.right] {
            let params = &planned.parameters;
            assert_eq!(params.sdu_interval, 10_000);
            assert_eq!(params.framing, 0);
            assert_eq!(params.phy, 0x02);
            assert_eq!(params.max_sdu, 100);
            assert_eq!(params.retransmission_number, 13);
            assert_eq!(params.max_transport_latency, 95);
            assert_eq!(params.presentation_delay, 40_000);
            assert_eq!(
                planned.expected_readback(),
                AseQosConfiguration {
                    cig_id: params.cig_id,
                    cis_id: params.cis_id,
                    sdu_interval: 10_000,
                    framing: 0,
                    phy: 0x02,
                    max_sdu: 100,
                    retransmission_number: 13,
                    max_transport_latency: 95,
                    presentation_delay: 40_000,
                }
            );
        }
    }

    #[test]
    fn extreme_profile_keeps_peer_required_presentation_delay() {
        let codec = codec_plan();
        let iso = crate::plane::l1_control::planning::iso::plan_first_iso_with_profile(
            &codec,
            StreamLatencyProfile::ExtremeLowLatency,
        )
        .unwrap();
        let qos = plan_first_qos_with_profile(
            &codec,
            &iso,
            StreamLatencyProfile::ExtremeLowLatency,
        )
        .unwrap();

        for planned in [&qos.left, &qos.right] {
            assert_eq!(planned.parameters.retransmission_number, 2);
            assert_eq!(planned.parameters.max_transport_latency, 10);
            assert_eq!(planned.parameters.presentation_delay, 40_000);
        }
    }

    #[test]
    fn qos_plan_rejects_codec_iso_connection_mismatch() {
        let codec = codec_plan();
        let mut iso = plan_first_iso(&codec).unwrap();
        iso.left.acl = iso.right.acl;

        assert_eq!(
            plan_first_qos(&codec, &iso),
            Err(QosPlanError::ConnectionMismatch {
                side: AudioSide::Left,
            })
        );
    }
}
