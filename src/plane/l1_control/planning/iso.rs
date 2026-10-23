use crate::{
    plane::l1_control::interface::stream::AudioSide,
    plane::l1_hci::{
        acl::connection::ConnectionToken,
        iso::cig_cis::{CigConfig, CisAclBinding, CisConfig, CisCreateRequest},
    },
};

use super::codec::StereoCodecPlan;

pub const FIRST_CIG_ID: u8 = 1;
pub const FIRST_LEFT_CIS_ID: u8 = 1;
pub const FIRST_RIGHT_CIS_ID: u8 = 2;
pub const FIRST_SDU_INTERVAL_US: u32 = 10_000;
pub const FIRST_MAX_TRANSPORT_LATENCY_MS: u16 = 95;
pub const FIRST_RTN: u8 = 13;
pub const FIRST_PHY_2M: u8 = 0x02;

pub const EXTREME_MAX_TRANSPORT_LATENCY_MS: u16 = 10;
pub const EXTREME_RTN: u8 = 3;
pub const EXTREME_PRESENTATION_DELAY_US: u32 = 40_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamLatencyProfile {
    #[default]
    Daily,
    ExtremeLowLatency,
}

impl StreamLatencyProfile {
    pub const fn max_transport_latency_ms(self) -> u16 {
        match self {
            Self::Daily => FIRST_MAX_TRANSPORT_LATENCY_MS,
            Self::ExtremeLowLatency => EXTREME_MAX_TRANSPORT_LATENCY_MS,
        }
    }

    pub const fn rtn(self) -> u8 {
        match self {
            Self::Daily => FIRST_RTN,
            Self::ExtremeLowLatency => EXTREME_RTN,
        }
    }

    pub const fn presentation_delay_us(self) -> u32 {
        match self {
            Self::Daily => 40_000,
            // Galaxy Buds3 Pro reports supported/preferred PD as exactly
            // 40_000 us for the current LC3 codec configuration. The extreme
            // profile therefore isolates transport latency/RTN only.
            Self::ExtremeLowLatency => EXTREME_PRESENTATION_DELAY_US,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedCis {
    pub side: AudioSide,
    pub cis_id: u8,
    pub acl: ConnectionToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StereoIsoPlan {
    pub cig_id: u8,
    pub octets_per_sdu: u16,
    pub sdu_interval_us: u32,
    pub max_transport_latency_ms: u16,
    pub phy: u8,
    pub rtn: u8,
    pub left: PlannedCis,
    pub right: PlannedCis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsoPlanError {
    InvalidFormat {
        sample_rate_hz: u32,
        frame_duration_us: u32,
        octets_per_sdu: u16,
    },
    DuplicateAcl(ConnectionToken),
}

pub fn plan_first_iso(
    codec: &StereoCodecPlan,
) -> Result<StereoIsoPlan, IsoPlanError> {
    plan_first_iso_with_profile(codec, StreamLatencyProfile::Daily)
}

pub fn plan_first_iso_with_profile(
    codec: &StereoCodecPlan,
    profile: StreamLatencyProfile,
) -> Result<StereoIsoPlan, IsoPlanError> {
    if codec.format.sample_rate_hz != 48_000
        || codec.format.frame_duration_us != 10_000
        || codec.format.octets_per_sdu != 100
    {
        return Err(IsoPlanError::InvalidFormat {
            sample_rate_hz: codec.format.sample_rate_hz,
            frame_duration_us: codec.format.frame_duration_us,
            octets_per_sdu: codec.format.octets_per_sdu,
        });
    }

    if codec.left.connection == codec.right.connection {
        return Err(IsoPlanError::DuplicateAcl(
            codec.left.connection,
        ));
    }

    Ok(StereoIsoPlan {
        cig_id: FIRST_CIG_ID,
        octets_per_sdu: codec.format.octets_per_sdu,
        sdu_interval_us: FIRST_SDU_INTERVAL_US,
        max_transport_latency_ms: profile.max_transport_latency_ms(),
        phy: FIRST_PHY_2M,
        rtn: profile.rtn(),
        left: PlannedCis {
            side: AudioSide::Left,
            cis_id: FIRST_LEFT_CIS_ID,
            acl: codec.left.connection,
        },
        right: PlannedCis {
            side: AudioSide::Right,
            cis_id: FIRST_RIGHT_CIS_ID,
            acl: codec.right.connection,
        },
    })
}

impl StereoIsoPlan {
    pub fn cig_config(&self) -> CigConfig {
        let cis = |planned: PlannedCis| CisConfig {
            cis_id: planned.cis_id,
            max_sdu_c_to_p: self.octets_per_sdu,
            max_sdu_p_to_c: 0,
            phy_c_to_p: self.phy,
            phy_p_to_c: self.phy,
            rtn_c_to_p: self.rtn,
            rtn_p_to_c: 0,
        };

        CigConfig {
            cig_id: self.cig_id,
            sdu_interval_c_to_p: self.sdu_interval_us,
            sdu_interval_p_to_c: self.sdu_interval_us,
            worst_case_sca: 0,
            packing: 0,
            framing: 0,
            max_transport_latency_c_to_p:
                self.max_transport_latency_ms,
            max_transport_latency_p_to_c:
                self.max_transport_latency_ms,
            cis: vec![cis(self.left), cis(self.right)],
        }
    }

    pub fn cis_create_request(&self) -> CisCreateRequest {
        CisCreateRequest {
            cig_id: self.cig_id,
            bindings: vec![
                CisAclBinding {
                    cis_id: self.left.cis_id,
                    acl: self.left.acl,
                },
                CisAclBinding {
                    cis_id: self.right.cis_id,
                    acl: self.right.acl,
                },
            ],
        }
    }

    pub fn cis_id(&self, side: AudioSide) -> u8 {
        match side {
            AudioSide::Left => self.left.cis_id,
            AudioSide::Right => self.right.cis_id,
        }
    }
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
                planning::codec::{PlannedSink, StereoCodecPlan},
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
    fn first_iso_plan_matches_real_review2d_hil_policy() {
        let plan = plan_first_iso(&codec_plan()).unwrap();
        assert_eq!(plan.cig_id, 1);
        assert_eq!(plan.left.cis_id, 1);
        assert_eq!(plan.right.cis_id, 2);
        assert_eq!(plan.sdu_interval_us, 10_000);
        assert_eq!(plan.octets_per_sdu, 100);
        assert_eq!(plan.phy, 0x02);
        assert_eq!(plan.rtn, 13);
        assert_eq!(plan.max_transport_latency_ms, 95);

        let cig = plan.cig_config();
        assert_eq!(cig.cig_id, 1);
        assert_eq!(cig.cis.len(), 2);
        assert_eq!(cig.cis[0].cis_id, 1);
        assert_eq!(cig.cis[1].cis_id, 2);
        assert!(cig.cis.iter().all(|cis| {
            cis.max_sdu_c_to_p == 100
                && cis.max_sdu_p_to_c == 0
                && cis.phy_c_to_p == 0x02
                && cis.phy_p_to_c == 0x02
                && cis.rtn_c_to_p == 13
                && cis.rtn_p_to_c == 0
        }));

        let create = plan.cis_create_request();
        assert_eq!(create.cig_id, 1);
        assert_eq!(create.bindings.len(), 2);
        assert_eq!(create.bindings[0].cis_id, 1);
        assert_eq!(create.bindings[0].acl.connection_handle, 0x0801);
        assert_eq!(create.bindings[1].cis_id, 2);
        assert_eq!(create.bindings[1].acl.connection_handle, 0x0800);
    }

    #[test]
    fn extreme_profile_pushes_transport_to_low_latency_edge() {
        let plan = plan_first_iso_with_profile(
            &codec_plan(),
            StreamLatencyProfile::ExtremeLowLatency,
        )
        .unwrap();

        assert_eq!(plan.sdu_interval_us, 10_000);
        assert_eq!(plan.phy, 0x02);
        assert_eq!(plan.rtn, 3);
        assert_eq!(plan.max_transport_latency_ms, 10);

        let cig = plan.cig_config();
        assert_eq!(cig.max_transport_latency_c_to_p, 10);
        assert!(cig.cis.iter().all(|cis| cis.rtn_c_to_p == 3));
    }

    #[test]
    fn planner_rejects_duplicate_acl_and_wrong_format() {
        let mut codec = codec_plan();
        codec.right.connection = codec.left.connection;
        assert!(matches!(
            plan_first_iso(&codec),
            Err(IsoPlanError::DuplicateAcl(_))
        ));

        let mut codec = codec_plan();
        codec.format.octets_per_sdu = 80;
        assert_eq!(
            plan_first_iso(&codec),
            Err(IsoPlanError::InvalidFormat {
                sample_rate_hz: 48_000,
                frame_duration_us: 10_000,
                octets_per_sdu: 80,
            })
        );
    }
}
