use crate::{
    error::{AppError, Result},
    usb,
};
use bap_client::{
    configure_and_release, configure_qos_and_release, establish_dual_cis_and_release,
    plan_first_sound, run_dual_iso_probe_and_release,
    plan_first_sound_qos, AudioSetSnapshot, CodecConfigScope, CodecConfigurationPlan,
    CodecConfiguredSetSnapshot, ConnectedAudioMember, DualCisProbeSnapshot,
    QosConfiguredSetSnapshot, StereoQosPlan,
};
use bumble_adapter::{
    BondMemberSummary, BondedPeerIdentity, LeAclReport, ScanObservation,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ENCRYPTION_TIMEOUT: Duration = Duration::from_secs(10);
const GATT_TIMEOUT: Duration = Duration::from_secs(12);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreFingerprint {
    pairing: String,
    set: String,
    local: String,
}

#[derive(Clone, Debug)]
struct ResolvedMember {
    member: BondMemberSummary,
    observation: ScanObservation,
}

#[derive(Clone, Debug)]
struct ActiveMember {
    member: BondMemberSummary,
    observation: ScanObservation,
    acl: LeAclReport,
}

pub fn parse_seconds(value: &str) -> Result<u16> {
    let seconds: u16 = value.parse().map_err(|_| {
        AppError::Protocol("R3 audio-set scan duration must be an integer (1..=30)".into())
    })?;
    if !(1..=30).contains(&seconds) {
        return Err(AppError::Protocol(
            "R3 audio-set scan duration must be 1..=30 seconds".into(),
        ));
    }
    Ok(seconds)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioSetAction {
    Inspect,
    CodecPlan,
    ConfigCodec(CodecConfigScope),
    QosPlan,
    ConfigQos,
    EstablishCis,
    IsoPaths,
    FirstSound,
}

impl AudioSetAction {
    fn creates_cis(self) -> bool {
        matches!(self, Self::EstablishCis | Self::IsoPaths | Self::FirstSound)
    }
}

pub fn iso_paths(seconds: u16, wanted_name: &str) -> Result<()> {
    inspect_impl(seconds, wanted_name, AudioSetAction::IsoPaths)
}

pub fn first_sound(seconds: u16, wanted_name: &str, tone: &audio_bridge::first_sound::PreparedStereoTone) -> Result<()> {
    inspect_with_audio(seconds, wanted_name, AudioSetAction::FirstSound, Some(tone))
}

pub fn inspect(seconds: u16, wanted_name: &str) -> Result<()> {
    inspect_impl(seconds, wanted_name, AudioSetAction::Inspect)
}

/// R4.1 read-only hardware bridge: reuse R3 discovery, then plan offline.
pub fn codec_plan(seconds: u16, wanted_name: &str) -> Result<()> {
    inspect_impl(seconds, wanted_name, AudioSetAction::CodecPlan)
}

pub fn parse_config_scope(value: &str) -> Result<CodecConfigScope> {
    match value {
        "left" => Ok(CodecConfigScope::Left),
        "right" => Ok(CodecConfigScope::Right),
        "both" => Ok(CodecConfigScope::Both),
        _ => Err(AppError::Usage("--side must be left, right or both".into())),
    }
}

/// R4.2 intentional ASCS mutation. All configured ASEs are released to Idle.
pub fn config_codec(seconds: u16, wanted_name: &str, scope: CodecConfigScope) -> Result<()> {
    inspect_impl(seconds, wanted_name, AudioSetAction::ConfigCodec(scope))
}

/// R5.1: reuse verified R4.2 ASCS Config Codec/Release, plan QoS offline.
/// No HCI Set CIG, ASCS Config QoS, Create CIS or ISO writes.
pub fn qos_plan(seconds: u16, wanted_name: &str) -> Result<()> {
    inspect_impl(seconds, wanted_name, AudioSetAction::QosPlan)
}

/// R5.2-A: real HCI Set CIG + dual ASCS Config QoS; no CIS establishment.
/// Every ASE returns to Idle and the unestablished CIG is removed.
pub fn config_qos(seconds: u16, wanted_name: &str) -> Result<()> {
    inspect_impl(seconds, wanted_name, AudioSetAction::ConfigQos)
}

/// R5.2-B: transient Enable + two LE CIS Established events with teardown.
pub fn establish_cis(seconds: u16, wanted_name: &str) -> Result<()> {
    inspect_impl(seconds, wanted_name, AudioSetAction::EstablishCis)
}

fn inspect_impl(seconds: u16, wanted_name: &str, action: AudioSetAction) -> Result<()> {
    inspect_with_audio(seconds, wanted_name, action, None)
}

fn inspect_with_audio(
    seconds: u16, wanted_name: &str, action: AudioSetAction,
    tone: Option<&audio_bridge::first_sound::PreparedStereoTone>,
) -> Result<()> {
    if matches!(action, AudioSetAction::FirstSound) && tone.is_none() {
        return Err(AppError::Protocol("First Sound requires offline-prepared LC3 frames".into()));
    }
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage(
            "--name requires a nonempty device name".into(),
        ));
    }

    println!(
        "{}",
        match action {
            AudioSetAction::Inspect => "R3 bonded audio-set inspection",
            AudioSetAction::CodecPlan => "R4.1 read-only codec planning",
            AudioSetAction::ConfigCodec(_) => "R4.2 ASCS Config Codec + Release transaction",
            AudioSetAction::QosPlan => "R5.1 QoS/CIG planner (R4.2 codec-write probe + dry run)",
            AudioSetAction::ConfigQos => "R5.2-A HCI CIG allocation + dual ASCS Config QoS transaction",
            AudioSetAction::EstablishCis => "R5.2-B ASCS Enable + Dual CIS Establishment + teardown",
            AudioSetAction::IsoPaths => "R5.3-A dual ISO input-path setup/remove probe (no audio)",
            AudioSetAction::FirstSound => "R5.3 A/B/C bounded dual-mono LC3 First Sound probe",
        }
    );
    println!("Target name: {wanted_name:?}");
    println!("Path: one scan -> resolve all members -> identity/FAL dual bonded ACL -> PACS/ASCS/CAS discovery.");
    if matches!(action, AudioSetAction::IsoPaths | AudioSetAction::FirstSound) {
        println!("Safety: explicit transient ASCS/CIG/CIS/ISO writes; no SMP, no bond mutation, no Source ASE or Receiver Start Ready.");
        println!("Cleanup: ISO drain/remove -> CIS disconnect -> ASE Disable/Release/Idle -> Remove CIG -> ACL/privacy cleanup.");
        if matches!(action, AudioSetAction::FirstSound) {
            println!("Audio: left 440Hz/right 880Hz low-level test tones; HCI completion does NOT prove acoustic output.");
        } else { println!("No LC3 or ISO audio packets will be sent."); }
    } else if matches!(action, AudioSetAction::EstablishCis) {
        println!("Safety: ASCS Codec/QoS/Enable/Disable/Release and HCI CIG/Create CIS/Disconnect writes ENABLED.");
        println!("No SMP, bond mutation, Receiver Start Ready, ISO Data Path or audio packets. CIS disconnect, ASE Idle and CIG removal required.");
    } else if matches!(action, AudioSetAction::ConfigQos) {
        println!("Safety: HCI Set/Remove CIG + ASCS Config Codec/Config QoS/Release writes are explicitly enabled.");
        println!("No SMP, bond mutation, ASCS Enable, HCI Create CIS or ISO traffic. Both ASEs return Idle and CIG is removed.");
    } else if matches!(action, AudioSetAction::ConfigCodec(_) | AudioSetAction::QosPlan) {
        println!("Safety: ASCS Config Codec/Release writes are ENABLED by explicit CLI confirmation.");
        println!("No SMP, Bond mutation, HCI Set CIG, ASCS Config QoS, CIS or ISO activity; every ASE returns Idle.");
    } else {
        println!("Safety: no SMP, no ASCS Control Point write, no CIG/CIS/ISO, no persistent-state mutation.");
    }
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    println!("Attach: PASS");
    // Cached HCI Local Features + Supported Commands + LE Buffer Size V2.
    // This does not submit any new HCI commands.
    let controller_iso = attached.iso_controller_capabilities();
    if matches!(action, AudioSetAction::QosPlan | AudioSetAction::ConfigQos | AudioSetAction::EstablishCis | AudioSetAction::IsoPaths | AudioSetAction::FirstSound) {
        println!(
            "[R5/ISO] controller capability snapshot: CIS_Central={} 2M={} SetCig={} CreateCis={} RemoveCig={} SetupIsoPath={} ISO={}B x {}",
            controller_iso.cis_central, controller_iso.le_2m_phy,
            controller_iso.set_cig_parameters, controller_iso.create_cis,
            controller_iso.remove_cig, controller_iso.setup_iso_data_path,
            controller_iso.iso_data_packet_length, controller_iso.iso_packet_count
        );
    }

    let controller_identity = attached
        .read_controller_identity(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!(
            "Read controller identity failed: {error}"
        )))?;
    println!("Controller identity: {}", controller_identity.public_address);

    let store = bumble_adapter::BondStore::open_for_controller(&controller_identity)
        .map_err(|error| AppError::Protocol(format!("Bond store open failed: {error}")))?;
    if !store.has_persistent_local_identity() {
        return Err(AppError::Protocol(
            "R3 requires a persistent local identity and completed R2 pairing".into(),
        ));
    }
    let resolver_count = attached
        .install_bond_store(&store)
        .map_err(|error| AppError::Protocol(format!("Bond store install failed: {error}")))?;

    let set = store.set_summary(wanted_name);
    let expected_size = set
        .set_size
        .ok_or_else(|| AppError::Protocol("Stored coordinated set has no Set Size".into()))?;
    if set.members.len() != usize::from(expected_size) {
        return Err(AppError::Protocol(format!(
            "Stored set is incomplete: {} committed member(s), Set Size={expected_size}",
            set.members.len()
        )));
    }
    if set
        .members
        .iter()
        .any(|member| !member.csis_verified || member.rank.is_none())
    {
        return Err(AppError::Protocol(
            "R3 requires every stored member to be CSIS-verified with a rank".into(),
        ));
    }

    let before = fingerprint_store(&store)?;
    println!(
        "Set context: committed={}/{} resolver_keys={} persistent fingerprint captured",
        set.members.len(), expected_size, resolver_count
    );

    // Core 5.2+ requires CIS Host Support (feature bit32) set *before*
    // establishing any LE ACL. Only R5.2-B and later ISO probes request
    // this controller-local Host capability; R3/R4/R5.2-A are unchanged.
    if action.creates_cis() {
        let report = attached.ensure_cis_host_feature_pre_acl()
            .map_err(|error| AppError::Protocol(format!(
                "R5.2-B controller Host Feature preflight failed: {error}"
            )))?;
        println!(
            "[R5.2/HOST] READY: bit32_enabled={} wrote_host_feature={}",
            report.enabled_after, report.set_command_sent
        );
        // Previous R5.2-B run reached 2/2 CIS Established but sent an
        // invalid Disconnect reason=0x16, leaving CIG 1 removal unconfirmed.
        // The prior ACL disconnect implicitly terminated both CIS. Before
        // another bonded connection, prove this CIG is absent or remove its
        // inactive leftover allocation; fail closed if still active (0x0C).
        attached.reconcile_test_cig_pre_acl(bap_client::FIRST_SOUND_CIG_ID)
            .map_err(|error| AppError::Protocol(format!(
                "R5.2-B pre-ACL residual CIG reconciliation failed: {error}"
            )))?;
    }

    println!("Discovery scan: resolving all current member addresses in one {seconds}s window...");
    let scan = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("R3 member scan failed: {error}")))?;
    let resolved = resolve_all_members(&store, &set.members, &scan.observations)?;

    let privacy_peers = attached
        .configure_bonded_identity_resolution(&store, &set.members)
        .map_err(|error| AppError::Protocol(format!(
            "R3 controller identity-resolution setup failed: {error}"
        )))?;

    let mut active = Vec::<ActiveMember>::new();
    let operation = (|| -> Result<(AudioSetSnapshot, Option<CodecConfigurationPlan>, Option<CodecConfiguredSetSnapshot>, Option<StereoQosPlan>, Option<QosConfiguredSetSnapshot>, Option<DualCisProbeSnapshot>)> {
        for resolved_member in resolved {
            let rank = resolved_member.member.rank.expect("rank checked");
            let privacy_peer = find_privacy_peer(&privacy_peers, &resolved_member.member)?;
            println!();
            println!(
                "Member rank={rank}: identity={} current={} type={} RSSI={}dBm",
                resolved_member.member.identity_address,
                resolved_member.observation.address,
                resolved_member.observation.address_type,
                resolved_member.observation.rssi
            );
            println!(
                "  connect policy: controller resolves current RPA; FAL target identity={} type={}",
                privacy_peer.identity_address, privacy_peer.address_type
            );

            let mut identity_target = resolved_member.observation.clone();
            identity_target.address = privacy_peer.identity_address.clone();
            identity_target.address_type = privacy_peer.address_type;

            let acl = {
                let mut manager = attached.le_connection_manager();
                manager
                    .direct_connect_filter_accept_list(
                        &identity_target,
                        CONNECT_TIMEOUT,
                    )
                    .map_err(|error| AppError::Protocol(format!(
                        "R3 ACL connection failed for {}: {error}",
                        resolved_member.member.identity_address
                    )))?
            };
            println!("  ACL: PASS handle=0x{:04X}", acl.connection_handle);

            let connection_handle = acl.connection_handle;
            let identity_address = resolved_member.member.identity_address.clone();
            // Register the ACL immediately after it is established. A failed or
            // timed-out stored-LTK recovery must not leak a connected handle.
            active.push(ActiveMember {
                member: resolved_member.member,
                observation: resolved_member.observation,
                acl,
            });

            attached
                .resume_bonded_encryption(
                    connection_handle,
                    &identity_address,
                    &store,
                    ENCRYPTION_TIMEOUT,
                )
                .map_err(|error| AppError::Protocol(format!(
                    "R3 stored-LTK encryption failed for {}: {error}",
                    identity_address
                )))?;
            println!("  stored-LTK encryption: PASS");

            let state = attached.le_acl_state(connection_handle);
            if !state.connected || !state.encrypted {
                return Err(AppError::Protocol(format!(
                    "R3 member {} did not remain connected+encrypted after security restore",
                    identity_address
                )));
            }

        }

        assert_distinct_live_handles(&attached, &active)?;
        println!();
        println!(
            "Dual bonded ACL gate: PASS - {} connected and encrypted member(s)",
            active.len()
        );

        let mut member_snapshots = Vec::with_capacity(active.len());
        for member in &active {
            let rank = member.member.rank.expect("rank checked");
            println!();
            println!(
                "Profile discovery: rank={} identity={} handle=0x{:04X}",
                rank, member.member.identity_address, member.acl.connection_handle
            );
            let connected = ConnectedAudioMember {
                identity_address: member.member.identity_address.clone(),
                rank,
                current_address: member.observation.address.clone(),
                current_address_type: member.observation.address_type,
                connection_handle: member.acl.connection_handle,
            };
            let snapshot = bap_client::discover_audio_member(
                &mut attached,
                &connected,
                GATT_TIMEOUT,
            )
            .map_err(|error| AppError::Protocol(format!(
                "R3 audio profile discovery failed for {}: {error}",
                member.member.identity_address
            )))?;
            println!(
                "  discovery: PASS sink_pac_records={} source_pac_records={} sink_ases={} source_ases={} CAS={}",
                snapshot.pacs.sink_pac_records.len(),
                snapshot.pacs.source_pac_records.len(),
                snapshot.ascs.sink_ases.len(),
                snapshot.ascs.source_ases.len(),
                snapshot.cap.is_some()
            );
            member_snapshots.push(snapshot);
        }

        let snapshot = AudioSetSnapshot {
            target_name: wanted_name.to_owned(),
            set_size: expected_size,
            members: member_snapshots,
        };
        print_snapshot(&snapshot);
        snapshot
            .validate_sink_discovery()
            .map_err(AppError::Protocol)?;
        validate_stereo_locations(&snapshot)?;
        // R3 reads only; R4.1 only plans; R4.2 mutates ASCS then releases
        // every attempted ASE while the two encrypted ACLs are still live.
        let plan = if matches!(action, AudioSetAction::Inspect) {
            None
        } else {
            Some(plan_first_sound(&snapshot).map_err(|error| {
                AppError::Protocol(format!("R4.1 codec planning failed: {error}"))
            })?)
        };
        if matches!(action, AudioSetAction::QosPlan | AudioSetAction::ConfigQos | AudioSetAction::EstablishCis | AudioSetAction::IsoPaths | AudioSetAction::FirstSound) {
            let max_sdu = plan.as_ref().expect("non-inspect action has codec plan")
                .stream.octets_per_codec_frame;
            // Refuse unsupported controllers BEFORE writing Config Codec.
            controller_iso.validate_for_unfragmented_cis(max_sdu)
                .map_err(|error| AppError::Protocol(format!("R5.1 ISO capability gate failed: {error}")))?;
        }
        let configured = match (action, plan.as_ref()) {
            (AudioSetAction::ConfigCodec(scope), Some(plan)) => {
                Some(configure_and_release(&mut attached, plan, scope).map_err(|error| {
                    AppError::Protocol(format!("R4.2 ASCS transaction failed: {error}"))
                })?)
            }
            (AudioSetAction::QosPlan, Some(plan)) => {
                Some(configure_and_release(&mut attached, plan, CodecConfigScope::Both)
                    .map_err(|error| AppError::Protocol(format!("R5.1 codec probe failed: {error}")))?)
            }
            _ => None,
        };
        // R4.2 has already released both ASEs to Idle. Planning does not
        // send an HCI/GATT QoS command or keep any ACL session alive.
        let qos = match (action, plan.as_ref(), configured.as_ref()) {
            (AudioSetAction::QosPlan, Some(codec), Some(confirmed)) => {
                Some(plan_first_sound_qos(codec, confirmed, controller_iso)
                    .map_err(|error| AppError::Protocol(format!("R5.1 QoS planning failed: {error}")))?)
            }
            _ => None,
        };
        let qos_configured = match (action, plan.as_ref()) {
            (AudioSetAction::ConfigQos, Some(codec)) => {
                Some(configure_qos_and_release(&mut attached, codec, controller_iso)
                    .map_err(|error| AppError::Protocol(format!("R5.2-A QoS transaction failed: {error}")))?)
            }
            _ => None,
        };
        let cis_established = match (action, plan.as_ref()) {
            (AudioSetAction::IsoPaths | AudioSetAction::FirstSound, Some(codec)) => {
                Some(run_dual_iso_probe_and_release(&mut attached, codec, controller_iso, tone)
                    .map_err(|error| AppError::Protocol(format!("R5.3 ISO transaction failed: {error}")))?)
            }
            (AudioSetAction::EstablishCis, Some(codec)) => {
                Some(establish_dual_cis_and_release(&mut attached, codec, controller_iso)
                    .map_err(|error| AppError::Protocol(format!("R5.2-B CIS transaction failed: {error}")))?)
            }
            _ => None,
        };
        Ok((snapshot, plan, configured, qos, qos_configured, cis_established))
    })();

    let disconnect_cleanup = disconnect_all(&mut attached, &active);
    let privacy_cleanup = attached
        .clear_bonded_identity_resolution()
        .map_err(|error| AppError::Protocol(format!(
            "R3 controller identity-resolution cleanup failed: {error}"
        )));
    let cleanup = combine_cleanup(disconnect_cleanup, privacy_cleanup);

    let (snapshot, plan, configured, qos, qos_configured, cis_established) = match operation {
        Ok(value) => value,
        Err(error) => {
            if let Err(cleanup_error) = cleanup {
                eprintln!("R3/R4 ACL/privacy cleanup also failed: {cleanup_error}");
            }
            // R4.2 is allowed to mutate transient ASE state, never bond
            // material. Audit that invariant on the failure path as well.
            match fingerprint_store(&store) {
                Ok(after) if after == before => {
                    eprintln!("Persistent-state fingerprint: UNCHANGED (failure path)");
                }
                Ok(_) => {
                    return Err(AppError::Protocol(format!(
                        "{error}; WARNING: persistent pairing/set/identity fingerprint CHANGED"
                    )));
                }
                Err(fingerprint_error) => {
                    return Err(AppError::Protocol(format!(
                        "{error}; persistent-state verification failed: {fingerprint_error}"
                    )));
                }
            }
            return Err(error);
        }
    };
    cleanup?;

    let after = fingerprint_store(&store)?;
    if before != after {
        return Err(AppError::Protocol(
            "R3 inspection changed persistent pairing/set/local-identity state".into(),
        ));
    }

    println!();
    println!("Persistent-state fingerprint: UNCHANGED");
    if let Some(plan) = plan {
        print_codec_plan(&plan);
        if let Some(cis) = cis_established {
            print_cis_probe(&cis);
            if let Some(iso) = &cis.iso {
                println!("[R5.3/RESULT] input_paths={}/2 removed={}/2 both_ases_streaming={}",
                    iso.input_paths_configured, iso.input_paths_removed, iso.both_ases_streaming);
                if let Some(stream) = &iso.stream {
                    println!("[R5.3/RESULT] transport probe PASS: pairs={} completed={:?} peak_pending={} elapsed={}ms max_late={}us",
                        stream.frame_pairs,stream.tx.completed,stream.tx.peak_pending,stream.elapsed_ms,stream.max_lateness_us);
                    println!("FIRST SOUND LISTENING RESULT: USER VERIFICATION REQUIRED (left 440Hz, right 880Hz). HCI completions are not audibility or RF delivery proof.");
                } else { println!("R5.3-A ISO DATA PATH PASS: 2/2 Setup and 2/2 Remove verified; zero audio packets requested."); }
                println!("R5.3 CLEANUP PASS: ASEs Idle, CIS/ACL disconnected, CIG removed, persistent bond fingerprint unchanged.");
            } else {
                println!("R5.2-B DUAL CIS PASS: 2/2 LE CIS Established events, both ASEs Idle, both CIS disconnected, CIG removed, ACLs disconnected, bond unchanged; no ISO Data Path/audio.");
            }
        } else if let Some(qos_configured) = qos_configured {
            print_qos_configured(&qos_configured);
            println!("R5.2-A CONFIG QOS PASS: both ASEs CodecConfigured -> QoSConfigured with CP+notify+readback; both released Idle, CIG removed, ACLs disconnected, no CIS established, bond unchanged.");
        } else if let Some(configured) = configured {
            print_codec_configured(&configured);
            if let Some(qos) = qos {
                print_qos_plan(&qos)?;
                println!("R5.1 QOS PLAN PASS: 2 CIS parameter sets, ASCS Config QoS and HCI Set CIG serializations validated; no QoS/CIG/CIS/ISO command sent; all ASEs Idle and bonds unchanged.");
            } else {
                println!("R4.2 CONFIG CODEC PASS: {} ASE(s) configured, response+notification+readback verified, all released to Idle; no CIG/CIS/ISO or bond mutation.",
                    configured.members.len());
            }
        } else {
            println!("R4.1 dry-run: ACL handles shown above have been disconnected; no ASE state changed.");
            println!("R4.1 CODEC PLAN PASS: dual-mono LC3 configuration validated; no ASCS control writes, CIG/CIS/ISO, or bond mutation.");
        }
    } else {
        println!(
            "R3 AUDIO-SET INSPECT PASS: {}/{} bonded member(s), PACS+ASCS discovered, stereo locations resolved, no control writes.",
            snapshot.members.len(), snapshot.set_size
        );
    }
    Ok(())
}

fn find_privacy_peer<'a>(
    peers: &'a [BondedPeerIdentity],
    member: &BondMemberSummary,
) -> Result<&'a BondedPeerIdentity> {
    peers
        .iter()
        .find(|peer| {
            peer.identity_address
                .eq_ignore_ascii_case(&member.identity_address)
        })
        .ok_or_else(|| AppError::Protocol(format!(
            "controller resolving-list setup omitted bonded identity {}",
            member.identity_address
        )))
}

fn combine_cleanup(disconnect: Result<()>, privacy: Result<()>) -> Result<()> {
    match (disconnect, privacy) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(disconnect), Err(privacy)) => Err(AppError::Protocol(format!(
            "{disconnect}; additionally {privacy}"
        ))),
    }
}

fn resolve_all_members(
    store: &bumble_adapter::BondStore,
    members: &[BondMemberSummary],
    observations: &[ScanObservation],
) -> Result<Vec<ResolvedMember>> {
    let mut strongest = BTreeMap::<String, ScanObservation>::new();
    for observation in observations.iter().filter(|value| value.is_connectable) {
        let resolved = store
            .resolve_observation_identity(observation)
            .map_err(|error| AppError::Protocol(format!(
                "RPA resolution failed for {}: {error}",
                observation.address
            )))?;
        let Some(identity) = resolved else {
            continue;
        };
        if !members
            .iter()
            .any(|member| member.identity_address.eq_ignore_ascii_case(&identity))
        {
            continue;
        }
        println!(
            "  resolved addr={} type={} RSSI={}dBm -> {}",
            observation.address, observation.address_type, observation.rssi, identity
        );
        strongest
            .entry(identity)
            .and_modify(|current| {
                if observation.rssi > current.rssi {
                    *current = observation.clone();
                }
            })
            .or_insert_with(|| observation.clone());
    }

    let mut ordered = members.to_vec();
    ordered.sort_by_key(|member| member.rank.unwrap_or(u8::MAX));
    let mut selected_addresses = BTreeSet::new();
    let mut result = Vec::with_capacity(ordered.len());
    for member in ordered {
        let key = strongest
            .keys()
            .find(|identity| identity.eq_ignore_ascii_case(&member.identity_address))
            .cloned()
            .ok_or_else(|| AppError::Protocol(format!(
                "Stored member {} was not visible/resolvable in the shared scan",
                member.identity_address
            )))?;
        let observation = strongest.remove(&key).expect("selected key exists");
        let address_key = format!(
            "{}|{}",
            observation.address.to_ascii_uppercase(),
            observation.address_type
        );
        if !selected_addresses.insert(address_key) {
            return Err(AppError::Protocol(
                "shared scan selected the same transport address for multiple set members".into(),
            ));
        }
        result.push(ResolvedMember { member, observation });
    }
    Ok(result)
}

fn assert_distinct_live_handles(
    attached: &bumble_adapter::AttachedController,
    active: &[ActiveMember],
) -> Result<()> {
    let mut handles = BTreeSet::new();
    for member in active {
        if !handles.insert(member.acl.connection_handle) {
            return Err(AppError::Protocol(format!(
                "controller reused active handle 0x{:04X} for multiple members",
                member.acl.connection_handle
            )));
        }
        let state = attached.le_acl_state(member.acl.connection_handle);
        if !state.connected || !state.encrypted {
            return Err(AppError::Protocol(format!(
                "ACL 0x{:04X} for {} is not simultaneously connected+encrypted",
                member.acl.connection_handle, member.member.identity_address
            )));
        }
    }
    Ok(())
}

fn disconnect_all(
    attached: &mut bumble_adapter::AttachedController,
    active: &[ActiveMember],
) -> Result<()> {
    let mut failures = Vec::new();
    for member in active.iter().rev() {
        let state = attached.le_acl_state(member.acl.connection_handle);
        if !state.connected {
            continue;
        }
        match attached.disconnect_le_connection(
            member.acl.connection_handle,
            0x13,
            DISCONNECT_TIMEOUT,
        ) {
            Ok(reason) => println!(
                "Disconnect rank={} handle=0x{:04X}: PASS reason=0x{reason:02X}",
                member.member.rank.unwrap_or(0),
                member.acl.connection_handle
            ),
            Err(error) => failures.push(format!(
                "{} handle=0x{:04X}: {error}",
                member.member.identity_address, member.acl.connection_handle
            )),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Protocol(format!(
            "R3 disconnect cleanup failed: {}",
            failures.join("; ")
        )))
    }
}

fn print_snapshot(snapshot: &AudioSetSnapshot) {
    println!();
    println!(
        "AudioSetSnapshot: target={:?} members={}/{}",
        snapshot.target_name,
        snapshot.members.len(),
        snapshot.set_size
    );
    for member in &snapshot.members {
        let sink_location = member
            .pacs
            .sink_audio_locations
            .map(|value| format!("0x{:08X}({})", value.0, location_side(value.0)))
            .unwrap_or_else(|| "absent".into());
        let source_location = member
            .pacs
            .source_audio_locations
            .map(|value| format!("0x{:08X}", value.0))
            .unwrap_or_else(|| "absent".into());
        println!(
            "  member rank={} identity={} current={} type={} handle=0x{:04X}",
            member.rank,
            member.identity_address,
            member.current_address,
            member.current_address_type,
            member.connection_handle
        );
        println!(
            "    PACS service=0x{:04X}..0x{:04X} sink_records={} source_records={} sink_location={} source_location={}",
            member.pacs.service.start_handle,
            member.pacs.service.end_handle,
            member.pacs.sink_pac_records.len(),
            member.pacs.source_pac_records.len(),
            sink_location,
            source_location
        );
        println!(
            "    contexts supported(sink=0x{:04X},source=0x{:04X}) available(sink=0x{:04X},source=0x{:04X})",
            member.pacs.supported_contexts.sink.0,
            member.pacs.supported_contexts.source.0,
            member.pacs.available_contexts.sink.0,
            member.pacs.available_contexts.source.0
        );
        for (index, record) in member.pacs.sink_pac_records.iter().enumerate() {
            println!("    sink_pac[{index}] {record:?}");
        }
        println!(
            "    ASCS service=0x{:04X}..0x{:04X} control_point=0x{:04X} sink_ases={} source_ases={}",
            member.ascs.service.start_handle,
            member.ascs.service.end_handle,
            member.ascs.control_point_handle,
            member.ascs.sink_ases.len(),
            member.ascs.source_ases.len()
        );
        for ase in &member.ascs.sink_ases {
            println!(
                "    sink_ase id={} value_handle=0x{:04X} state=0x{:02X} details={:?}",
                ase.status.ase_id,
                ase.value_handle,
                ase.status.state.0,
                ase.status.details
            );
        }
        match &member.cap {
            Some(cap) => println!(
                "    CAS service=0x{:04X}..0x{:04X} included_csis={}",
                cap.service.start_handle,
                cap.service.end_handle,
                cap.included_csis
                    .as_ref()
                    .map(|service| format!(
                        "0x{:04X}..0x{:04X}",
                        service.start_handle, service.end_handle
                    ))
                    .unwrap_or_else(|| "absent".into())
            ),
            None => println!("    CAS: absent"),
        }
    }
}

fn print_cis_probe(snapshot: &DualCisProbeSnapshot) {
    println!();
    println!(
        "DualCisProbeSnapshot: CIG={} enabled={} QoSConfigured={} Established={}/2 disconnected={} idle={} cig_removed={}",
        snapshot.allocation.cig_id, snapshot.enabled.len(),
        snapshot.qos_members.len(), snapshot.established.len(),
        snapshot.all_cis_disconnected, snapshot.all_ases_idle, snapshot.cig_removed
    );
    for entry in &snapshot.established {
        let link = &entry.link;
        println!(
            "  CIS ID={} handle=0x{:04X} ACL=0x{:04X} PHY(C/P)={}/{} lat_CtoP={}us NSE={} BN={} FT={} MaxPDU={} ISO_interval_units={}",
            entry.cis_id, link.connection_handle, entry.acl_handle,
            link.phy_c_to_p, link.phy_p_to_c,
            link.transport_latency_c_to_p, link.nse,
            link.bn_c_to_p, link.ft_c_to_p, link.max_pdu_c_to_p,
            link.iso_interval
        );
    }
    println!("  CIS and ACL handles are no longer valid after teardown.");
    if snapshot.iso.as_ref().and_then(|iso| iso.stream.as_ref()).is_none() {
        println!("  No ISO audio payload transmitted.");
    }
}

fn print_qos_configured(snapshot: &QosConfiguredSetSnapshot) {
    println!();
    println!(
        "QosConfiguredSetSnapshot: members={} CIG={} removed={} released_idle={}",
        snapshot.members.len(), snapshot.allocation.cig_id,
        snapshot.cig_removed, snapshot.released_to_idle
    );
    for member in &snapshot.members {
        let qos = &member.actual;
        let handle = snapshot.allocation.cis.iter()
            .find(|cis| cis.cis_id == qos.cis_id)
            .map(|cis| format!("0x{:04X}", cis.connection_handle))
            .unwrap_or_else(|| "unmapped".to_owned());
        println!(
            "  {} ASE={} CIG={} CIS={} allocated_handle={} interval={}us PHY={} SDU={} RTN={} latency={}ms presentation={}us",
            member.side.as_str(), member.ase_id, qos.cig_id, qos.cis_id,
            handle, qos.sdu_interval, qos.phy, qos.max_sdu,
            qos.retransmission_number, qos.max_transport_latency,
            qos.presentation_delay
        );
    }
    println!("  Allocated handles are invalid after successful Remove CIG; no CIS was established.");
}

fn print_qos_plan(plan: &StereoQosPlan) -> Result<()> {
    println!();
    println!(
        "StereoQosPlan: CIG={} CIS={} interval={}us PHY_C_to_P={} framing={} max_SDU={} transport_latency={}ms presentation_delay={}us",
        plan.cig.cig_id, plan.cig.cis_parameters.len(), plan.sdu_interval_us,
        plan.phy_c_to_p, plan.cig.framing, plan.left.ascs.max_sdu,
        plan.max_transport_latency_ms, plan.presentation_delay_us
    );
    for member in [&plan.left, &plan.right] {
        println!(
            "  {} CIS={} ASE={} identity={} ACL=0x{:04X} SDU={} rtn={} lat={}ms pd={}us",
            member.side.as_str(), member.cis_id, member.ascs.ase_id,
            member.identity_address, member.acl_handle, member.ascs.max_sdu,
            member.ascs.retransmission_number, member.ascs.max_transport_latency,
            member.ascs.presentation_delay
        );
        let ascs_bytes = plan.ascs_config_qos(member.side).to_bytes()
            .map_err(|error| AppError::Protocol(format!("R5.1 ASCS serialization failed: {error}")))?;
        println!("    ASCS Config QoS wire: {ascs_bytes:02X?}");
    }
    for cis in &plan.cig.cis_parameters {
        println!(
            "  CIG CIS={} C->P max_sdu={} phy={} rtn={} ; P->C max_sdu={} phy={} rtn={}",
            cis.cis_id, cis.max_sdu_c_to_p, cis.phy_c_to_p, cis.rtn_c_to_p,
            cis.max_sdu_p_to_c, cis.phy_p_to_c, cis.rtn_p_to_c
        );
    }
    let hci_bytes = plan.hci_set_cig_command().to_bytes();
    println!("  HCI LE Set CIG Parameters wire (NOT SENT): {hci_bytes:02X?}");
    println!("  R5.1 dry-run: all ACL handles shown above have been disconnected.");
    Ok(())
}

fn print_codec_configured(snapshot: &CodecConfiguredSetSnapshot) {
    println!();
    println!("CodecConfiguredSetSnapshot: configured={} released_to_idle={}",
        snapshot.members.len(), snapshot.all_released_to_idle);
    for member in &snapshot.members {
        println!(
            "  {} identity={} ASE={} framing={} PHY={} retransmissions={} max_transport_latency={}ms",
            member.side.as_str(),
            member.identity_address,
            member.ase_id,
            member.preferred_framing,
            member.preferred_phy,
            member.preferred_retransmission_number,
            member.preferred_max_transport_latency
        );
        println!(
            "    supported_presentation_delay={}..{}us preferred={}..{}us codec_ltv={:02X?}",
            member.supported_presentation_delay_min,
            member.supported_presentation_delay_max,
            member.preferred_presentation_delay_min,
            member.preferred_presentation_delay_max,
            member.codec_specific_configuration
        );
    }
}

fn print_codec_plan(plan: &CodecConfigurationPlan) {
    println!();
    println!(
        "CodecConfigurationPlan: LC3 {}Hz {}us {}octets/frame 1frame/SDU context=0x{:04X}",
        plan.stream.sample_rate_hz,
        plan.stream.frame_duration_us,
        plan.stream.octets_per_codec_frame,
        plan.context.0
    );
    for sink in [&plan.left, &plan.right] {
        println!(
            "  {} rank={} identity={} ACL=0x{:04X} ASE={} value=0x{:04X} CP=0x{:04X} location=0x{:08X}",
            sink.side.as_str(),
            sink.rank,
            sink.identity_address,
            sink.acl_handle,
            sink.ase_id,
            sink.ase_value_handle,
            sink.ascs_control_point_handle,
            sink.audio_location.0
        );
        let ltv = sink.codec_specific_configuration.to_bytes();
        println!("    codec_id={:?} codec_specific_configuration={:02X?}", sink.codec_id, ltv);
    }
}

fn validate_stereo_locations(snapshot: &AudioSetSnapshot) -> Result<()> {
    let sides = snapshot
        .members
        .iter()
        .map(|member| {
            member
                .pacs
                .sink_audio_locations
                .map(|value| location_side(value.0))
                .unwrap_or("absent")
        })
        .collect::<Vec<_>>();
    let left = sides.iter().filter(|side| **side == "left").count();
    let right = sides.iter().filter(|side| **side == "right").count();
    if left != 1 || right != 1 {
        return Err(AppError::Protocol(format!(
            "R3 expected one left and one right Sink Audio Location; observed {:?}",
            sides
        )));
    }
    Ok(())
}

fn location_side(bits: u32) -> &'static str {
    const LEFT: u32 = 0x0000_0001
        | 0x0000_0010
        | 0x0000_0040
        | 0x0000_0400
        | 0x0000_1000
        | 0x0001_0000
        | 0x0004_0000
        | 0x0040_0000
        | 0x0100_0000
        | 0x0400_0000;
    const RIGHT: u32 = 0x0000_0002
        | 0x0000_0020
        | 0x0000_0080
        | 0x0000_0800
        | 0x0000_2000
        | 0x0002_0000
        | 0x0008_0000
        | 0x0080_0000
        | 0x0200_0000
        | 0x0800_0000;
    match (bits & LEFT != 0, bits & RIGHT != 0) {
        (true, false) => "left",
        (false, true) => "right",
        (true, true) => "mixed",
        (false, false) => "unknown",
    }
}

fn fingerprint_store(store: &bumble_adapter::BondStore) -> Result<StoreFingerprint> {
    Ok(StoreFingerprint {
        pairing: fingerprint(&store.pairing_keys_path())?,
        set: fingerprint(&store.set_db_path())?,
        local: fingerprint(&store.local_identity_path())?,
    })
}

fn fingerprint(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|error| AppError::Protocol(format!(
        "Failed to fingerprint {}: {error}",
        path.display()
    )))?;
    Ok(format!("{}B:{:x}", bytes.len(), Sha256::digest(bytes)))
}
