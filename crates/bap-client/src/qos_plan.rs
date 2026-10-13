//! R5.1: pure, deterministic BAP unicast QoS + CIG planning.
//!
//! Maps two confirmed R4.2 Codec Configured snapshots into:
//!   * two Bumble-RS ASCS Config QoS parameter sets
//!   * one Bumble-RS CIG with two strictly downlink CIS entries
//!   * a byte-validated HCI LE Set CIG Parameters command.
//!
//! Zephyr's bt_audio_verify_qos / bt_bap_valid_qos_pref and AOSP Fluoride's
//! unidirectional CIG construction provide cross-checks. This module never
//! opens HCI, creates a CIG or writes a GATT characteristic.

use bumble_adapter::IsoControllerCapabilities;
use bumble_hci::{CodingFormat, Command};
use bumble_host::{CigParameters, CisParameters};
use bumble_profiles::ascs::{AseOperation, ConfigQosParameters};
use device_policy::StreamTopology;

use crate::codec_config::{CodecConfiguredMember, CodecConfiguredSetSnapshot};
use crate::codec_plan::{CodecConfigurationPlan, PlannedSink, StereoSide};

pub const FIRST_SOUND_CIG_ID: u8 = 1;
pub const LEFT_CIS_ID: u8 = 1;
pub const RIGHT_CIS_ID: u8 = 2;
const PHY_1M: u8 = 0x01;
const PHY_2M: u8 = 0x02;
const PHY_PREFERENCE_MASK: u8 = 0x07;
const NO_PREFERRED_PRESENTATION_DELAY: u32 = 0x00FF_FFFF;
const MAX_PRESENTATION_DELAY_US: u32 = 0x00FF_FFFF;
const MIN_TRANSPORT_LATENCY_MS: u16 = 5;
const MAX_TRANSPORT_LATENCY_MS: u16 = 4_000;
const MIN_SDU_INTERVAL_US: u32 = 0x0000_00FF;
const MAX_SDU_INTERVAL_US: u32 = 0x000F_FFFF;
const MAX_ISO_SDU_BYTES: u16 = 0x0FFF;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedQosMember {
    pub side: StereoSide,
    pub identity_address: String,
    /// Connection handle is live only during the originating R4.2 session.
    pub acl_handle: u16,
    pub cis_id: u8,
    pub ascs: ConfigQosParameters,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StereoQosPlan {
    pub presentation_delay_us: u32,
    pub sdu_interval_us: u32,
    pub phy_c_to_p: u8,
    pub max_transport_latency_ms: u16,
    pub controller: IsoControllerCapabilities,
    pub cig: CigParameters,
    pub left: PlannedQosMember,
    pub right: PlannedQosMember,
}

impl StereoQosPlan {
    /// Upstream Bumble-RS encodes the HCI packet; no local wire serializer.
    pub fn hci_set_cig_command(&self) -> Command {
        let cig = &self.cig;
        let cis = cig
            .cis_parameters
            .iter()
            .copied()
            .map(CisParameters::normalized)
            .collect::<Vec<_>>();
        Command::LeSetCigParameters {
            cig_id: cig.cig_id,
            sdu_interval_c_to_p: cig.sdu_interval_c_to_p,
            sdu_interval_p_to_c: cig.sdu_interval_p_to_c,
            worst_case_sca: cig.worst_case_sca,
            packing: cig.packing,
            framing: cig.framing,
            max_transport_latency_c_to_p: cig.max_transport_latency_c_to_p,
            max_transport_latency_p_to_c: cig.max_transport_latency_p_to_c,
            cis_id: cis.iter().map(|v| v.cis_id).collect(),
            max_sdu_c_to_p: cis.iter().map(|v| v.max_sdu_c_to_p).collect(),
            max_sdu_p_to_c: cis.iter().map(|v| v.max_sdu_p_to_c).collect(),
            phy_c_to_p: cis.iter().map(|v| v.phy_c_to_p).collect(),
            phy_p_to_c: cis.iter().map(|v| v.phy_p_to_c).collect(),
            rtn_c_to_p: cis.iter().map(|v| v.rtn_c_to_p).collect(),
            rtn_p_to_c: cis.iter().map(|v| v.rtn_p_to_c).collect(),
        }
    }

    pub fn ascs_config_qos(&self, side: StereoSide) -> AseOperation {
        let member = match side {
            StereoSide::Left => &self.left,
            StereoSide::Right => &self.right,
        };
        AseOperation::ConfigQos(vec![member.ascs.clone()])
    }

    /// Check actual wire encoders and ASCS parsing without transmitting.
    pub fn validate_serialization(&self) -> Result<(), String> {
        let hci = self.hci_set_cig_command().to_bytes();
        if hci.len() < 4 || hci[0] != 0x01 || hci[1] != 0x62 || hci[2] != 0x20
            || usize::from(hci[3]) != hci.len() - 4
        {
            return Err("Bumble-RS returned an invalid HCI LE Set CIG Parameters wire packet".into());
        }
        for side in [StereoSide::Left, StereoSide::Right] {
            let operation = self.ascs_config_qos(side);
            let bytes = operation.to_bytes()
                .map_err(|error| format!("ASCS Config QoS encoding failed: {error}"))?;
            let parsed = AseOperation::from_bytes(&bytes)
                .map_err(|error| format!("ASCS Config QoS roundtrip failed: {error}"))?;
            if parsed != operation {
                return Err("ASCS Config QoS roundtrip changed planned parameters".into());
            }
        }
        Ok(())
    }
}

fn validate_member(
    planned: &PlannedSink,
    configured: &CodecConfiguredMember,
) -> Result<(), String> {
    let side = planned.side.as_str();
    if configured.side != planned.side
        || configured.ase_id != planned.ase_id
        || !configured.identity_address.eq_ignore_ascii_case(&planned.identity_address)
    {
        return Err(format!("{side}: Codec Configured result does not match the R4.1 member/ASE identity"));
    }
    if configured.codec_specific_configuration != planned.codec_specific_configuration.to_bytes() {
        return Err(format!("{side}: Codec Configured LC3 LTV differs from the R4.1 plan"));
    }
    if planned.codec_id != CodingFormat::LC3 {
        return Err(format!("{side}: first-sound QoS currently supports LC3 only"));
    }
    if configured.preferred_framing > 1 {
        return Err(format!("{side}: invalid preferred framing {}", configured.preferred_framing));
    }
    if configured.preferred_phy & !PHY_PREFERENCE_MASK != 0 {
        return Err(format!("{side}: invalid preferred PHY bit mask 0x{:02X}", configured.preferred_phy));
    }
    if !(MIN_TRANSPORT_LATENCY_MS..=MAX_TRANSPORT_LATENCY_MS)
        .contains(&configured.preferred_max_transport_latency)
    {
        return Err(format!("{side}: preferred transport latency {}ms outside 5..=4000",
            configured.preferred_max_transport_latency));
    }
    let supported_min = configured.supported_presentation_delay_min;
    let supported_max = configured.supported_presentation_delay_max;
    if supported_min > supported_max || supported_max > MAX_PRESENTATION_DELAY_US {
        return Err(format!("{side}: invalid supported presentation delay {}..{}us", supported_min, supported_max));
    }
    let preferred_min = configured.preferred_presentation_delay_min;
    let preferred_max = configured.preferred_presentation_delay_max;
    if preferred_min != NO_PREFERRED_PRESENTATION_DELAY
        && !(supported_min..=supported_max).contains(&preferred_min)
    {
        return Err(format!("{side}: preferred minimum presentation delay is outside the supported range"));
    }
    if preferred_max != NO_PREFERRED_PRESENTATION_DELAY
        && !(supported_min..=supported_max).contains(&preferred_max)
    {
        return Err(format!("{side}: preferred maximum presentation delay is outside the supported range"));
    }
    if preferred_min != NO_PREFERRED_PRESENTATION_DELAY
        && preferred_max != NO_PREFERRED_PRESENTATION_DELAY
        && preferred_min > preferred_max
    {
        return Err(format!("{side}: preferred presentation delay range is inverted"));
    }
    Ok(())
}

/// Intersection of hard presentation-delay ranges, with both sides' optional
/// preferred ranges used only if they themselves intersect. This never lets
/// an empty *preference* intersection invalidate a legal common delay.
fn choose_presentation_delay(
    left: &CodecConfiguredMember,
    right: &CodecConfiguredMember,
) -> Result<u32, String> {
    let minimum = left.supported_presentation_delay_min
        .max(right.supported_presentation_delay_min);
    let maximum = left.supported_presentation_delay_max
        .min(right.supported_presentation_delay_max);
    if minimum > maximum {
        return Err(format!(
            "left/right supported presentation-delay ranges do not intersect (left {}..{}, right {}..{})",
            left.supported_presentation_delay_min, left.supported_presentation_delay_max,
            right.supported_presentation_delay_min, right.supported_presentation_delay_max,
        ));
    }
    let min_pref = |m: &CodecConfiguredMember| {
        if m.preferred_presentation_delay_min == NO_PREFERRED_PRESENTATION_DELAY {
            m.supported_presentation_delay_min
        } else {
            m.preferred_presentation_delay_min
        }
    };
    let max_pref = |m: &CodecConfiguredMember| {
        if m.preferred_presentation_delay_max == NO_PREFERRED_PRESENTATION_DELAY {
            m.supported_presentation_delay_max
        } else {
            m.preferred_presentation_delay_max
        }
    };
    let preferred_min = minimum.max(min_pref(left)).max(min_pref(right));
    let preferred_max = maximum.min(max_pref(left)).min(max_pref(right));
    Ok(if preferred_min <= preferred_max { preferred_min } else { minimum })
}

fn planned_member(
    planned: &PlannedSink,
    configured: &CodecConfiguredMember,
    cig_id: u8,
    cis_id: u8,
    interval: u32,
    framing: u8,
    phy: u8,
    max_sdu: u16,
    latency: u16,
    presentation_delay: u32,
) -> PlannedQosMember {
    PlannedQosMember {
        side: planned.side,
        identity_address: planned.identity_address.clone(),
        acl_handle: planned.acl_handle,
        cis_id,
        ascs: ConfigQosParameters {
            ase_id: planned.ase_id,
            cig_id,
            cis_id,
            sdu_interval: interval,
            framing,
            phy,
            max_sdu,
            retransmission_number: configured.preferred_retransmission_number,
            max_transport_latency: latency,
            presentation_delay,
        },
    }
}

/// Product policy: CIG 1, left CIS 1, right CIS 2, unidirectional downlink.
/// HCI reverse direction retains a *valid* interval/latency/PHY even though
/// Max_SDU=P->C=0 and RTN=P->C=0, matching AOSP / Bumble-RS conventions.
///
/// Controller ability to schedule two CIS at the requested RTN/latency is
/// unknown until R5.2 HCI Set CIG Parameters actually returns success.
pub fn plan_first_sound_qos(
    codec: &CodecConfigurationPlan,
    confirmed: &CodecConfiguredSetSnapshot,
    controller: IsoControllerCapabilities,
) -> Result<StereoQosPlan, String> {
    if codec.stream.topology != StreamTopology::StereoDualMonoAse
        || codec.left.side != StereoSide::Left
        || codec.right.side != StereoSide::Right
    {
        return Err("R5.1 requires one left and one right mono Sink ASE".into());
    }
    if codec.left.acl_handle == codec.right.acl_handle
        || codec.left.identity_address.eq_ignore_ascii_case(&codec.right.identity_address)
    {
        return Err("R5.1 requires two distinct bonded members and ACL handles".into());
    }
    if !confirmed.all_released_to_idle || confirmed.members.len() != 2 {
        return Err("R5.1 requires two confirmed, released Codec Configured results".into());
    }

    let left = confirmed.members.iter()
        .find(|member| member.side == StereoSide::Left)
        .ok_or("missing left Codec Configured result")?;
    let right = confirmed.members.iter()
        .find(|member| member.side == StereoSide::Right)
        .ok_or("missing right Codec Configured result")?;
    validate_member(&codec.left, left)?;
    validate_member(&codec.right, right)?;

    if codec.left.codec_specific_configuration.codec_frames_per_sdu != Some(1)
        || codec.right.codec_specific_configuration.codec_frames_per_sdu != Some(1)
    {
        return Err("R5.1 first-sound supports exactly one LC3 frame per SDU".into());
    }
    let interval = codec.stream.frame_duration_us;
    if !(MIN_SDU_INTERVAL_US..=MAX_SDU_INTERVAL_US).contains(&interval)
        || !matches!(interval, 7_500 | 10_000)
    {
        return Err(format!("first-sound SDU interval {}us is unsupported", interval));
    }
    let max_sdu = codec.stream.octets_per_codec_frame;
    if max_sdu == 0 || max_sdu > MAX_ISO_SDU_BYTES {
        return Err(format!("invalid ISO Max SDU {max_sdu} bytes"));
    }
    controller.validate_for_unfragmented_cis(max_sdu)?;

    let presentation_delay = choose_presentation_delay(left, right)?;
    let framing = left.preferred_framing.max(right.preferred_framing);
    let prefers_2m = left.preferred_phy & PHY_2M != 0
        && right.preferred_phy & PHY_2M != 0;
    let phy = if controller.le_2m_phy && prefers_2m { PHY_2M } else { PHY_1M };
    let latency = left.preferred_max_transport_latency
        .min(right.preferred_max_transport_latency);

    let left_plan = planned_member(
        &codec.left, left, FIRST_SOUND_CIG_ID, LEFT_CIS_ID,
        interval, framing, phy, max_sdu, latency, presentation_delay,
    );
    let right_plan = planned_member(
        &codec.right, right, FIRST_SOUND_CIG_ID, RIGHT_CIS_ID,
        interval, framing, phy, max_sdu, latency, presentation_delay,
    );
    let cis = |id, retransmission_number| CisParameters {
        cis_id: id,
        max_sdu_c_to_p: max_sdu,
        max_sdu_p_to_c: 0,
        phy_c_to_p: phy,
        // Keep 1M as the valid reverse (control/ACK) direction.
        phy_p_to_c: PHY_1M,
        rtn_c_to_p: retransmission_number,
        rtn_p_to_c: 0,
    };
    let cig = CigParameters {
        cig_id: FIRST_SOUND_CIG_ID,
        cis_parameters: vec![
            cis(LEFT_CIS_ID, left_plan.ascs.retransmission_number),
            cis(RIGHT_CIS_ID, right_plan.ascs.retransmission_number),
        ],
        sdu_interval_c_to_p: interval,
        sdu_interval_p_to_c: interval,
        worst_case_sca: 0,
        packing: 0,
        framing,
        max_transport_latency_c_to_p: latency,
        max_transport_latency_p_to_c: latency,
    };
    let plan = StereoQosPlan {
        presentation_delay_us: presentation_delay,
        sdu_interval_us: interval,
        phy_c_to_p: phy,
        max_transport_latency_ms: latency,
        controller,
        cig,
        left: left_plan,
        right: right_plan,
    };
    plan.validate_serialization()?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bumble_profiles::bap::{
        AudioLocation, CodecSpecificConfiguration, ContextType, FrameDuration,
        SamplingFrequency,
    };
    use device_policy::{StreamPlan, StreamTopology};

    fn capabilities() -> IsoControllerCapabilities {
        IsoControllerCapabilities {
            cis_central: true,
            le_2m_phy: true,
            set_cig_parameters: true,
            create_cis: true,
            remove_cig: true,
            setup_iso_data_path: true,
            iso_data_packet_length: 248,
            iso_packet_count: 8,
        }
    }

    fn fixture() -> (CodecConfigurationPlan, CodecConfiguredSetSnapshot) {
        let config_for = |side| CodecSpecificConfiguration {
            sampling_frequency: Some(SamplingFrequency::from_hz(48_000).unwrap()),
            frame_duration: Some(FrameDuration::DURATION_10000_US),
            audio_channel_allocation: Some(match side {
                StereoSide::Left => AudioLocation::FRONT_LEFT,
                StereoSide::Right => AudioLocation::FRONT_RIGHT,
            }),
            octets_per_codec_frame: Some(100),
            codec_frames_per_sdu: Some(1),
        };
        let sink = |side, id, acl, rank| PlannedSink {
            side,
            identity_address: id.into(),
            rank,
            acl_handle: acl,
            ase_id: 1,
            ase_value_handle: 0x0046,
            ascs_control_point_handle: 0x0043,
            audio_location: match side {
                StereoSide::Left => AudioLocation::FRONT_LEFT,
                StereoSide::Right => AudioLocation::FRONT_RIGHT,
            },
            codec_id: CodingFormat::LC3,
            codec_specific_configuration: config_for(side),
        };
        let codec = CodecConfigurationPlan {
            context: ContextType::MEDIA,
            stream: StreamPlan::first_sound(StreamTopology::StereoDualMonoAse, 100),
            left: sink(StereoSide::Left, "02:00:00:00:00:02", 0x0801, 2),
            right: sink(StereoSide::Right, "02:00:00:00:00:01", 0x0800, 1),
        };
        let result = |sink: &PlannedSink| CodecConfiguredMember {
            side: sink.side,
            identity_address: sink.identity_address.clone(),
            ase_id: sink.ase_id,
            preferred_framing: 0,
            preferred_phy: 2,
            preferred_retransmission_number: 13,
            preferred_max_transport_latency: 95,
            supported_presentation_delay_min: 40_000,
            supported_presentation_delay_max: 40_000,
            preferred_presentation_delay_min: 40_000,
            preferred_presentation_delay_max: 40_000,
            codec_specific_configuration: sink.codec_specific_configuration.to_bytes(),
        };
        let configured = CodecConfiguredSetSnapshot {
            members: vec![result(&codec.right), result(&codec.left)],
            all_released_to_idle: true,
        };
        (codec, configured)
    }

    #[test]
    fn buds3pro_dual_mono_qos_uses_real_preferences() {
        let (codec, configured) = fixture();
        let plan = plan_first_sound_qos(&codec, &configured, capabilities()).unwrap();
        assert_eq!(plan.presentation_delay_us, 40_000);
        assert_eq!(plan.sdu_interval_us, 10_000);
        assert_eq!(plan.phy_c_to_p, 2);
        assert_eq!(plan.max_transport_latency_ms, 95);
        assert_eq!(plan.left.ascs.cis_id, LEFT_CIS_ID);
        assert_eq!(plan.right.ascs.cis_id, RIGHT_CIS_ID);
        assert_eq!(plan.left.ascs.ase_id, 1);
        assert_eq!(plan.cig.cis_parameters.len(), 2);
        for cis in &plan.cig.cis_parameters {
            assert_eq!(cis.max_sdu_c_to_p, 100);
            assert_eq!(cis.max_sdu_p_to_c, 0);
            assert_eq!(cis.phy_p_to_c, PHY_1M);
            assert_eq!(cis.rtn_c_to_p, 13);
            assert_eq!(cis.rtn_p_to_c, 0);
        }
        plan.validate_serialization().unwrap();
    }

    #[test]
    fn serializes_through_upstream_ascs_and_hci_encoders() {
        let (codec, configured) = fixture();
        let plan = plan_first_sound_qos(&codec, &configured, capabilities()).unwrap();
        let hci = plan.hci_set_cig_command().to_bytes();
        assert_eq!(&hci[0..4], &[0x01, 0x62, 0x20, 33]);
        assert_eq!(hci.len(), 37);
        for side in [StereoSide::Left, StereoSide::Right] {
            let bytes = plan.ascs_config_qos(side).to_bytes().unwrap();
            assert_eq!(bytes[0..2], [0x02, 0x01]);
            assert_eq!(bytes.len(), 18);
        }
    }

    #[test]
    fn output_is_deterministic_under_member_order_reversal() {
        let (codec, mut configured) = fixture();
        let expected = plan_first_sound_qos(&codec, &configured, capabilities()).unwrap();
        configured.members.reverse();
        assert_eq!(expected, plan_first_sound_qos(&codec, &configured, capabilities()).unwrap());
    }

    #[test]
    fn rejects_wrong_member_identity_or_codec_ltv() {
        let (codec, mut confirmed) = fixture();
        confirmed.members[0].identity_address = "wrong identity".into();
        assert!(plan_first_sound_qos(&codec, &confirmed, capabilities()).is_err());

        let (codec, mut confirmed) = fixture();
        confirmed.members[0].codec_specific_configuration.push(0xEE);
        assert!(plan_first_sound_qos(&codec, &confirmed, capabilities()).is_err());
    }

    #[test]
    fn rejects_duplicate_members_or_unconfirmed_release() {
        let (codec, mut confirmed) = fixture();
        confirmed.members[0].side = StereoSide::Left;
        assert!(plan_first_sound_qos(&codec, &confirmed, capabilities()).is_err());

        let (codec, mut confirmed) = fixture();
        confirmed.all_released_to_idle = false;
        assert!(plan_first_sound_qos(&codec, &confirmed, capabilities()).is_err());
    }

    #[test]
    fn rejects_nonintersecting_presentation_delay() {
        let (codec, mut confirmed) = fixture();
        confirmed.members[0].supported_presentation_delay_min = 30_000;
        confirmed.members[0].supported_presentation_delay_max = 35_000;
        confirmed.members[0].preferred_presentation_delay_min = 32_000;
        confirmed.members[0].preferred_presentation_delay_max = 32_000;
        assert!(plan_first_sound_qos(&codec, &confirmed, capabilities()).is_err());
    }

    #[test]
    fn falls_back_to_1m_when_controller_has_no_2m() {
        let (codec, confirmed) = fixture();
        let mut capability = capabilities();
        capability.le_2m_phy = false;
        let plan = plan_first_sound_qos(&codec, &confirmed, capability).unwrap();
        assert_eq!(plan.phy_c_to_p, PHY_1M);
        assert_eq!(plan.cig.cis_parameters[0].phy_c_to_p, PHY_1M);
    }

    #[test]
    fn requires_iso_buffer_capacity_and_commands() {
        let (codec, confirmed) = fixture();
        let mut cap = capabilities();
        cap.iso_data_packet_length = 64;
        assert!(plan_first_sound_qos(&codec, &confirmed, cap).is_err());
        cap = capabilities();
        cap.set_cig_parameters = false;
        assert!(plan_first_sound_qos(&codec, &confirmed, cap).is_err());
    }

    #[test]
    fn uses_distinct_per_cis_rtn_and_no_upstream_payload() {
        let (codec, mut confirmed) = fixture();
        confirmed.members[0].preferred_retransmission_number = 2;
        let plan = plan_first_sound_qos(&codec, &confirmed, capabilities()).unwrap();
        assert_eq!(plan.left.ascs.retransmission_number, 13);
        assert_eq!(plan.right.ascs.retransmission_number, 2);
        assert_eq!(plan.cig.cis_parameters[1].rtn_c_to_p, 2);
        assert_eq!(plan.cig.cis_parameters[1].rtn_p_to_c, 0);
    }

    #[test]
    fn rejects_illegal_qos_preference_bounds() {
        let (codec, mut confirmed) = fixture();
        confirmed.members[0].preferred_max_transport_latency = 2;
        assert!(plan_first_sound_qos(&codec, &confirmed, capabilities()).is_err());
        let (codec, mut confirmed) = fixture();
        confirmed.members[0].preferred_presentation_delay_max = 39_999;
        assert!(plan_first_sound_qos(&codec, &confirmed, capabilities()).is_err());
    }
}
