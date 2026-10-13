//! R5.2-B: guarded, short-lived two-ear LE CIS establishment probe.
//!
//! Reuses R4.2/R5.2-A ASCS and CIG paths. The only new protocol operations
//! are ASCS Enable/Disable and HCI LE Create CIS/Disconnect. No Receiver Start
//! Ready is sent (both selected ASEs are Audio Sink), no ISO Data Path is
//! installed and no LC3/ISO packets are transmitted.

use std::time::{Duration, Instant};

use bumble_adapter::{AttachedController, CigAllocation, EstablishedCis, IsoControllerCapabilities};
use bumble_profiles::ascs::{
    AseMetadataParameters, AseOperation, AseState, AseStatus, AseStatusDetails,
};
use bumble_profiles::le_audio::{Metadata, MetadataEntry, MetadataTag};

use crate::codec_config::{
    configure_qos_sink, configure_sink, perform_operation, prepare_sink,
    read_sink_status, release_if_needed, CodecConfiguredSetSnapshot, PreparedSink,
    QosConfiguredMember,
};
use crate::codec_plan::{CodecConfigurationPlan, StereoSide};
use crate::qos_plan::{
    plan_live_codec_configured_qos, FIRST_SOUND_CIG_ID,
    LEFT_CIS_ID, RIGHT_CIS_ID,
};

const ENABLE_TIMEOUT: Duration = Duration::from_secs(8);
const DISABLE_TIMEOUT: Duration = Duration::from_secs(8);
const CIS_ESTABLISH_TIMEOUT: Duration = Duration::from_secs(12);
const CIS_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const RELEASE_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnabledMember {
    pub side: StereoSide,
    pub ase_id: u8,
    pub cig_id: u8,
    pub cis_id: u8,
    pub state_after_enable: AseState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DualCisProbeSnapshot {
    pub qos_members: Vec<QosConfiguredMember>,
    pub enabled: Vec<EnabledMember>,
    pub allocation: CigAllocation,
    /// Controller snapshots captured before deliberate CIS disconnection.
    pub established: Vec<EstablishedCis>,
    pub all_ases_idle: bool,
    pub all_cis_disconnected: bool,
    pub cig_removed: bool,
}

fn media_metadata(context: u16) -> Result<Metadata, String> {
    if context == 0 {
        return Err("R5.2-B Enable Streaming Audio Contexts cannot be empty".into());
    }
    Ok(Metadata::new(vec![
        MetadataEntry::new(
            MetadataTag::STREAMING_AUDIO_CONTEXTS,
            context.to_le_bytes().to_vec(),
        )
    ]))
}

fn verify_enabled_status(
    side: StereoSide,
    ase_id: u8,
    cig_id: u8,
    cis_id: u8,
    context: u16,
    status: AseStatus,
) -> Result<EnabledMember, String> {
    if status.ase_id != ase_id || status.state != AseState::ENABLING {
        return Err(format!(
            "{} Enable status wrong ASE/state: got ASE={} state=0x{:02X}, expected ASE={} Enabling",
            side.as_str(), status.ase_id, status.state.0, ase_id
        ));
    }
    let AseStatusDetails::Metadata {
        cig_id: got_cig, cis_id: got_cis, metadata,
    } = status.details else {
        return Err(format!("{} Enabling ASE has no CIG/CIS metadata", side.as_str()));
    };
    if got_cig != cig_id || got_cis != cis_id {
        return Err(format!(
            "{} Enabling CIG/CIS mismatch: returned {got_cig}/{got_cis}, expected {cig_id}/{cis_id}",
            side.as_str()
        ));
    }
    let media_context = metadata.entries.iter().any(|entry| {
        entry.tag == MetadataTag::STREAMING_AUDIO_CONTEXTS
            && entry.data.len() == 2
            && (u16::from_le_bytes([entry.data[0], entry.data[1]]) & context) == context
    });
    if !media_context {
        return Err(format!(
            "{} Enabling metadata omitted Streaming Audio Context 0x{context:04X}: {metadata:?}",
            side.as_str()
        ));
    }
    Ok(EnabledMember {
        side, ase_id, cig_id, cis_id,
        state_after_enable: AseState::ENABLING,
    })
}

fn enable_sink(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
    cig_id: u8,
    cis_id: u8,
    context: u16,
) -> Result<EnabledMember, String> {
    let metadata = media_metadata(context)?;
    let operation = AseOperation::Enable(vec![AseMetadataParameters {
        ase_id: prepared.sink.ase_id,
        metadata: metadata.to_bytes()
            .map_err(|error| format!("Enable metadata encode: {error}"))?,
    }]);
    let result = perform_operation(
        controller, prepared, operation, AseState::ENABLING, ENABLE_TIMEOUT,
    )?;
    let enabled = verify_enabled_status(
        prepared.sink.side, prepared.sink.ase_id,
        cig_id, cis_id, context, result,
    )?;
    println!(
        "[R5.2/ENABLE] {} ASE={} CIG={} CIS={} Enabling PASS (CP + notify + readback)",
        prepared.sink.side.as_str(), prepared.sink.ase_id, cig_id, cis_id
    );
    Ok(enabled)
}

fn verify_post_cis_ase_status(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
    cig_id: u8,
    cis_id: u8,
) -> Result<(), String> {
    let status = read_sink_status(controller, prepared)?;
    if status.ase_id != prepared.sink.ase_id {
        return Err(format!(
            "{} post-CIS ASE ID mismatch", prepared.sink.side.as_str()
        ));
    }
    if status.state != AseState::ENABLING && status.state != AseState::STREAMING {
        return Err(format!(
            "{} post-CIS unexpected ASE state=0x{:02X} (expected Enabling or Streaming)",
            prepared.sink.side.as_str(), status.state.0
        ));
    }
    match status.details {
        AseStatusDetails::Metadata {
            cig_id: actual_cig, cis_id: actual_cis, ..
        } if actual_cig == cig_id && actual_cis == cis_id => {
            println!(
                "[R5.2/STATE] {} ASE={} after CIS: state=0x{:02X} CIG={} CIS={} PASS",
                prepared.sink.side.as_str(), status.ase_id, status.state.0,
                actual_cig, actual_cis
            );
            Ok(())
        }
        details => Err(format!(
            "{} post-CIS ASE metadata/CIG/CIS mismatch: {details:?}",
            prepared.sink.side.as_str()
        )),
    }
}

/// Cleanup after Create CIS, including partial/uncertain establishment:
/// (a) caller disconnects any established CIS,
/// (b) sink Disable if still Enabling/Streaming,
/// (c) Release and read back Idle; then caller removes the CIG.
fn disable_if_streaming_and_release(
    controller: &mut AttachedController,
    prepared: &PreparedSink<'_>,
) -> Result<(), String> {
    let state = read_sink_status(controller, prepared)?;
    match state.state {
        AseState::ENABLING | AseState::STREAMING => {
            perform_operation(
                controller, prepared,
                AseOperation::Disable(vec![prepared.sink.ase_id]),
                AseState::QOS_CONFIGURED, DISABLE_TIMEOUT,
            )?;
            println!(
                "[R5.2/DISABLE] {} ASE={} QoSConfigured PASS (CP + notify + readback)",
                prepared.sink.side.as_str(), prepared.sink.ase_id
            );
        }
        AseState::RELEASING | AseState::DISABLING => {
            // A notification/remote autonomous transition can overtake the
            // readback. Poll for a stable state rather than issuing a second
            // CP operation while the server is already transitioning.
            let deadline = Instant::now() + RELEASE_SETTLE_TIMEOUT;
            loop {
                let current = read_sink_status(controller, prepared)?;
                if current.state != AseState::RELEASING
                    && current.state != AseState::DISABLING
                {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "{} ASE={} did not exit transient state 0x{:02X}",
                        prepared.sink.side.as_str(), prepared.sink.ase_id,
                        current.state.0
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        AseState::CODEC_CONFIGURED | AseState::QOS_CONFIGURED | AseState::IDLE => {}
        other => return Err(format!(
            "{} ASE={} cannot clean up unexpected state 0x{:02X}",
            prepared.sink.side.as_str(), prepared.sink.ase_id, other.0
        )),
    }
    // After a transient settles, it may again be Enabling/Streaming; recheck
    // before Release so errors stay visible instead of silently discarding.
    let current = read_sink_status(controller, prepared)?;
    if current.state == AseState::ENABLING || current.state == AseState::STREAMING {
        perform_operation(
            controller, prepared,
            AseOperation::Disable(vec![prepared.sink.ase_id]),
            AseState::QOS_CONFIGURED, DISABLE_TIMEOUT,
        )?;
    }
    release_if_needed(controller, prepared)
}

/// One-shot, write-confirmed real-HCI probe. Never configures an ISO Data
/// Path or sends any audio. On any stage failure, attempts reverse lifecycle
/// rollback and reports UNCONFIRMED if one cleanup gate cannot be proven.
pub fn establish_dual_cis_and_release(
    controller: &mut AttachedController,
    codec: &CodecConfigurationPlan,
    capabilities: IsoControllerCapabilities,
) -> Result<DualCisProbeSnapshot, String> {
    if codec.left.acl_handle == codec.right.acl_handle {
        return Err("R5.2-B requires distinct encrypted left/right ACLs".into());
    }
    capabilities.validate_for_unfragmented_cis(codec.stream.octets_per_codec_frame)?;
    let prepared = vec![
        prepare_sink(controller, &codec.left)?,
        prepare_sink(controller, &codec.right)?,
    ];
    println!("[R5.2-B/TXN] Codec -> CIG -> QoS -> Enable -> Create CIS -> Disconnect -> Disable -> Release -> Remove CIG");

    let mut attempted = 0usize;
    let mut cig_may_exist = false;
    let mut cis_create_attempted = false;
    let mut allocation = None::<CigAllocation>;
    let mut codec_members = Vec::with_capacity(2);
    let mut qos_members = Vec::with_capacity(2);
    let mut enabled = Vec::with_capacity(2);
    let mut established = Vec::with_capacity(2);

    let stage = (|| -> Result<(), String> {
        for sink in &prepared {
            attempted += 1;
            codec_members.push(configure_sink(controller, sink)?);
        }
        let live = CodecConfiguredSetSnapshot {
            members: codec_members.clone(),
            all_released_to_idle: false,
        };
        let qos = plan_live_codec_configured_qos(codec, &live, capabilities)?;
        let cis_ids = qos.cig.cis_parameters.iter()
            .map(|cis| cis.cis_id).collect::<Vec<_>>();
        println!(
            "[R5.2-B/PLAN] CIG={} CIS=2 interval={}us PHY={} RTN={}/{} latency={}ms delay={}us",
            qos.cig.cig_id, qos.sdu_interval_us, qos.phy_c_to_p,
            qos.left.ascs.retransmission_number, qos.right.ascs.retransmission_number,
            qos.max_transport_latency_ms, qos.presentation_delay_us
        );
        let allocated = controller.set_cig_parameters_unestablished(
            qos.hci_set_cig_command(), qos.cig.cig_id, &cis_ids,
        ).map_err(|failure| {
            cig_may_exist = failure.may_have_allocated;
            format!("Set CIG Parameters: {failure}")
        })?;
        cig_may_exist = true;
        if allocated.cis.iter().any(|entry|
            entry.connection_handle == codec.left.acl_handle
                || entry.connection_handle == codec.right.acl_handle
        ) {
            return Err("controller returned a CIS handle colliding with live ACL".into());
        }
        allocation = Some(allocated.clone());
        for sink in &prepared {
            qos_members.push(configure_qos_sink(controller, sink, &qos)?);
        }

        for (sink, cis_id) in [(&prepared[0], LEFT_CIS_ID), (&prepared[1], RIGHT_CIS_ID)] {
            enabled.push(enable_sink(
                controller, sink, allocated.cig_id, cis_id, codec.context.0,
            )?);
        }

        let mappings = [
            (LEFT_CIS_ID, codec.left.acl_handle),
            (RIGHT_CIS_ID, codec.right.acl_handle),
        ];
        cis_create_attempted = true;
        established = controller.establish_dual_cis(
            &allocated, &mappings, CIS_ESTABLISH_TIMEOUT,
        )?;
        println!(
            "[R5.2-B/CIS] DUAL CIS ESTABLISHED: {}/2 LE CIS Established PASS; no ISO Data Path configured",
            established.len()
        );
        verify_post_cis_ase_status(controller, &prepared[0], allocated.cig_id, LEFT_CIS_ID)?;
        verify_post_cis_ase_status(controller, &prepared[1], allocated.cig_id, RIGHT_CIS_ID)?;
        Ok(())
    })();

    let mut cleanup_errors = Vec::new();
    if cis_create_attempted {
        if let Some(ref allocated) = allocation {
            if let Err(error) = controller.disconnect_established_cis(
                allocated, CIS_DISCONNECT_TIMEOUT,
            ) {
                cleanup_errors.push(format!("CIS disconnect UNCONFIRMED: {error}"));
            }
        }
    }
    for sink in prepared[..attempted].iter().rev() {
        if let Err(error) = disable_if_streaming_and_release(controller, sink) {
            cleanup_errors.push(format!(
                "{} ASE Disable/Release/Idle UNCONFIRMED: {error}",
                sink.sink.side.as_str()
            ));
        }
    }
    if cig_may_exist {
        if let Err(error) = controller.remove_unestablished_cig(FIRST_SOUND_CIG_ID) {
            cleanup_errors.push(format!("Remove CIG UNCONFIRMED: {error}"));
        }
    }
    if let Err(error) = stage {
        return Err(if cleanup_errors.is_empty() {
            format!("R5.2-B transaction failed, rollback confirmed: {error}")
        } else {
            format!(
                "R5.2-B transaction failed: {error}; cleanup UNCONFIRMED: {}",
                cleanup_errors.join("; ")
            )
        });
    }
    if !cleanup_errors.is_empty() {
        return Err(format!(
            "R5.2-B established CIS, but cleanup UNCONFIRMED: {}",
            cleanup_errors.join("; ")
        ));
    }
    let allocation = allocation.ok_or("successful R5.2-B missing CIG allocation")?;
    if qos_members.len() != 2 || enabled.len() != 2 || established.len() != 2 {
        return Err("R5.2-B successful stage unexpectedly omitted one ear".into());
    }
    Ok(DualCisProbeSnapshot {
        qos_members,
        enabled,
        allocation,
        established,
        all_ases_idle: true,
        all_cis_disconnected: true,
        cig_removed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_context_encodes_upstream_streaming_audio_metadata_ltv() {
        let value = media_metadata(0x0004).unwrap().to_bytes().unwrap();
        assert_eq!(value, vec![0x03, 0x02, 0x04, 0x00]);
        assert!(media_metadata(0).is_err());
    }

    fn enabling_status(cig_id: u8, cis_id: u8, context: u16) -> AseStatus {
        AseStatus {
            ase_id: 1,
            state: AseState::ENABLING,
            details: AseStatusDetails::Metadata {
                cig_id, cis_id, metadata: media_metadata(context).unwrap(),
            },
        }
    }

    #[test]
    fn enable_gate_requires_matching_state_ase_cig_cis_and_media() {
        assert!(verify_enabled_status(
            StereoSide::Left, 1, 1, 1, 0x0004, enabling_status(1, 1, 0x0004),
        ).is_ok());
        assert!(verify_enabled_status(
            StereoSide::Left, 1, 1, 1, 0x0004, enabling_status(1, 2, 0x0004),
        ).is_err());
        assert!(verify_enabled_status(
            StereoSide::Left, 1, 1, 1, 0x0004, enabling_status(1, 1, 0x0002),
        ).is_err());
        let mut wrong = enabling_status(1, 1, 0x0004);
        wrong.state = AseState::QOS_CONFIGURED;
        assert!(verify_enabled_status(StereoSide::Left, 1, 1, 1, 0x0004, wrong).is_err());
    }
}
