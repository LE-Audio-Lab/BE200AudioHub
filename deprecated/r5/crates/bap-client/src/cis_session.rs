//! Shared guarded two-ear CIS lifecycle for R5.2-B and the opt-in R5.3 probe.
//!
//! R5.2-B still installs no ISO paths and transmits no audio. R5.3 additionally
//! verifies input paths and optionally sends pre-encoded dual-mono LC3, then
//! removes the paths before the existing CIS/ASE/CIG rollback. Neither path
//! sends Receiver Start Ready: both selected ASEs are Audio Sink.

use std::time::{Duration, Instant};

use bumble_adapter::{AttachedController, CigAllocation, EstablishedCis, IsoControllerCapabilities, IsoStreamReport, ProbeGuard};
use audio_bridge::first_sound::PreparedStereoTone;
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
    /// Present only for the opt-in R5.3 path. R5.2-B remains control-only.
    pub iso: Option<IsoProbeSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoProbeSnapshot {
    pub input_paths_configured: usize,
    pub input_paths_removed: usize,
    pub both_ases_streaming: bool,
    pub stream: Option<IsoStreamReport>,
}

#[derive(Clone, Copy)]
enum DataProbe<'a> { None, PathsOnly, Tone(&'a PreparedStereoTone) }
impl DataProbe<'_> {
    fn enabled(self) -> bool { !matches!(self, Self::None) }
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

fn wait_for_streaming(
    controller: &mut AttachedController, prepared: &PreparedSink<'_>, cig_id: u8, cis_id: u8,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        ProbeGuard::check_cancelled()?;
        let status = read_sink_status(controller, prepared)?;
        if status.ase_id != prepared.sink.ase_id {
            return Err("R5.3 remote ASE identity changed".into());
        }
        match &status.details {
            AseStatusDetails::Metadata { cig_id: actual_cig, cis_id: actual_cis, .. }
                if *actual_cig == cig_id && *actual_cis == cis_id => {},
            other => return Err(format!("R5.3 remote ASE metadata mismatch: {other:?}")),
        }
        if status.state == AseState::STREAMING {
            println!("[R5.3/ASE] {} ASE={} Streaming readback PASS", prepared.sink.side.as_str(), status.ase_id);
            return Ok(());
        }
        if status.state != AseState::ENABLING || Instant::now() >= deadline {
            return Err(format!("R5.3 {} ASE={} not Streaming: state=0x{:02X}; no Receiver Start Ready is sent to Sink ASEs",
                prepared.sink.side.as_str(),status.ase_id,status.state.0));
        }
        std::thread::sleep(Duration::from_millis(50));
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
    run_dual_cis_transaction(controller, codec, capabilities, DataProbe::None)
}

/// R5.3-A when tone is None; the complete A/B/C probe when tone is prepared.
/// Shares the proven R5.2-B transaction and its rollback, not a second lifecycle.
pub fn run_dual_iso_probe_and_release(
    controller: &mut AttachedController,
    codec: &CodecConfigurationPlan,
    capabilities: IsoControllerCapabilities,
    tone: Option<&PreparedStereoTone>,
) -> Result<DualCisProbeSnapshot, String> {
    run_dual_cis_transaction(controller, codec, capabilities,
        tone.map_or(DataProbe::PathsOnly, DataProbe::Tone))
}

fn run_dual_cis_transaction(
    controller: &mut AttachedController,
    codec: &CodecConfigurationPlan,
    capabilities: IsoControllerCapabilities,
    probe: DataProbe<'_>,
) -> Result<DualCisProbeSnapshot, String> {
    if codec.left.acl_handle == codec.right.acl_handle {
        return Err("R5.2-B requires distinct encrypted left/right ACLs".into());
    }
    capabilities.validate_for_unfragmented_cis(codec.stream.octets_per_codec_frame)?;
    if probe.enabled() {
        if codec.stream.sample_rate_hz != 48_000 || codec.stream.frame_duration_us != 10_000
            || codec.stream.octets_per_codec_frame != 100
            || codec.stream.topology != device_policy::StreamTopology::StereoDualMonoAse
        { return Err("R5.3 requires the verified 48kHz/10ms/100B dual-mono codec plan".into()); }
        controller.require_iso_remove_support()?;
        ProbeGuard::check_cancelled()?;
    }
    let prepared = vec![
        prepare_sink(controller, &codec.left)?,
        prepare_sink(controller, &codec.right)?,
    ];
    println!("[CIS/TXN] Codec -> CIG -> QoS -> Enable -> Create CIS -> optional ISO probe -> Disconnect -> Disable -> Release -> Remove CIG");
    if probe.enabled() {
        println!("[R5.3/TXN] ISO: Setup Input Paths -> optional bounded LC3 -> Drain -> Remove Input Paths (before CIS teardown)");
    }

    let mut attempted = 0usize;
    let mut cig_may_exist = false;
    let mut cis_create_attempted = false;
    let mut allocation = None::<CigAllocation>;
    let mut codec_members = Vec::with_capacity(2);
    let mut qos_members = Vec::with_capacity(2);
    let mut enabled = Vec::with_capacity(2);
    let mut established = Vec::with_capacity(2);
    let mut paths_may_exist = Vec::<u16>::new();
    let mut paths_configured = 0usize;
    let mut paths_removed = 0usize;
    let mut stream_attempted = false;
    let mut stream_report = None;
    let mut both_streaming = false;
    let mut stereo_handles = None;

    let stage = (|| -> Result<(), String> {
        for sink in &prepared {
            if probe.enabled() { ProbeGuard::check_cancelled()?; }
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
        if probe.enabled() {
            let handle_for = |id| allocated.cis.iter().find(|cis| cis.cis_id == id)
                .map(|cis| cis.connection_handle).ok_or_else(|| format!("allocated CIG omitted CIS {id}"));
            let handles = [handle_for(LEFT_CIS_ID)?, handle_for(RIGHT_CIS_ID)?];
            stereo_handles = Some(handles);
            for handle in handles {
                ProbeGuard::check_cancelled()?;
                match controller.setup_iso_input_path(handle) {
                    Ok(()) => { paths_may_exist.push(handle); paths_configured += 1; }
                    Err(error) => {
                        if error.may_have_configured { paths_may_exist.push(handle); }
                        return Err(format!("R5.3-A ISO Setup failed: {error}"));
                    }
                }
            }
            if let DataProbe::Tone(tone) = probe {
                wait_for_streaming(controller, &prepared[0], allocated.cig_id, LEFT_CIS_ID)?;
                wait_for_streaming(controller, &prepared[1], allocated.cig_id, RIGHT_CIS_ID)?;
                both_streaming = true;
                let frames = tone.frames().iter().map(|f| (&f.left[..], &f.right[..])).collect::<Vec<_>>();
                println!("[R5.3-C/START] {}s / {} pairs; left 440Hz right 880Hz, low-level tone; no Windows audio capture", tone.play_seconds(), frames.len());
                stream_attempted = true;
                stream_report = Some(controller.transmit_stereo_iso(handles,
                    [codec.left.acl_handle, codec.right.acl_handle], &frames)?);
                // A live CIS alone is not proof the remote ASE kept Streaming.
                wait_for_streaming(controller, &prepared[0], allocated.cig_id, LEFT_CIS_ID)?;
                wait_for_streaming(controller, &prepared[1], allocated.cig_id, RIGHT_CIS_ID)?;
            }
        }
        Ok(())
    })();

    let mut cleanup_errors = Vec::new();
    if stream_attempted {
        if let Some(handles) = stereo_handles {
            if let Err(error) = controller.drain_iso_output(handles) {
                cleanup_errors.push(format!("ISO drain before teardown UNCONFIRMED: {error}"));
            }
        }
    }
    for &handle in paths_may_exist.iter().rev() {
        match controller.remove_iso_input_path(handle) {
            Ok(()) => paths_removed += 1,
            Err(error) => cleanup_errors.push(format!("ISO input path Remove UNCONFIRMED: {error}")),
        }
    }
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
            format!("CIS/ISO transaction failed, rollback confirmed: {error}")
        } else {
            format!(
                "CIS/ISO transaction failed: {error}; cleanup UNCONFIRMED: {}",
                cleanup_errors.join("; ")
            )
        });
    }
    if !cleanup_errors.is_empty() {
        return Err(format!(
            "CIS/ISO stage completed, but cleanup UNCONFIRMED: {}",
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
        iso: probe.enabled().then_some(IsoProbeSnapshot {
            input_paths_configured: paths_configured,
            input_paths_removed: paths_removed,
            both_ases_streaming: both_streaming,
            stream: stream_report,
        }),
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
