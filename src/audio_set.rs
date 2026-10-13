use crate::{
    error::{AppError, Result},
    usb,
};
use bap_client::{AudioSetSnapshot, ConnectedAudioMember};
use bumble_adapter::{BondMemberSummary, LeAclReport, ScanObservation};
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

pub fn inspect(seconds: u16, wanted_name: &str) -> Result<()> {
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage(
            "--name requires a nonempty device name".into(),
        ));
    }

    println!("R3 bonded audio-set inspection");
    println!("Target name: {wanted_name:?}");
    println!("Path: one scan -> resolve all members -> dual bonded ACL -> PACS/ASCS/CAS discovery.");
    println!("Safety: no SMP, no ASCS Control Point write, no CIG/CIS/ISO, no persistent-state mutation.");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    println!("Attach: PASS");

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

    println!("Discovery scan: resolving all current member addresses in one {seconds}s window...");
    let scan = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("R3 member scan failed: {error}")))?;
    let resolved = resolve_all_members(&store, &set.members, &scan.observations)?;

    let mut active = Vec::<ActiveMember>::new();
    let operation = (|| -> Result<AudioSetSnapshot> {
        for resolved_member in resolved {
            let rank = resolved_member.member.rank.expect("rank checked");
            println!();
            println!(
                "Member rank={rank}: identity={} current={} type={} RSSI={}dBm",
                resolved_member.member.identity_address,
                resolved_member.observation.address,
                resolved_member.observation.address_type,
                resolved_member.observation.rssi
            );

            let acl = {
                let mut manager = attached.le_connection_manager();
                manager
                    .direct_connect_filter_accept_list(
                        &resolved_member.observation,
                        CONNECT_TIMEOUT,
                    )
                    .map_err(|error| AppError::Protocol(format!(
                        "R3 ACL connection failed for {}: {error}",
                        resolved_member.member.identity_address
                    )))?
            };
            println!("  ACL: PASS handle=0x{:04X}", acl.connection_handle);

            attached
                .resume_bonded_encryption(
                    acl.connection_handle,
                    &resolved_member.member.identity_address,
                    &store,
                    ENCRYPTION_TIMEOUT,
                )
                .map_err(|error| AppError::Protocol(format!(
                    "R3 stored-LTK encryption failed for {}: {error}",
                    resolved_member.member.identity_address
                )))?;
            println!("  stored-LTK encryption: PASS");

            let state = attached.le_acl_state(acl.connection_handle);
            if !state.connected || !state.encrypted {
                return Err(AppError::Protocol(format!(
                    "R3 member {} did not remain connected+encrypted after security restore",
                    resolved_member.member.identity_address
                )));
            }

            active.push(ActiveMember {
                member: resolved_member.member,
                observation: resolved_member.observation,
                acl,
            });
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
        Ok(snapshot)
    })();

    let cleanup = disconnect_all(&mut attached, &active);
    let snapshot = match operation {
        Ok(snapshot) => snapshot,
        Err(error) => {
            if let Err(cleanup_error) = cleanup {
                eprintln!("R3 cleanup also failed: {cleanup_error}");
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
    println!(
        "R3 AUDIO-SET INSPECT PASS: {}/{} bonded member(s), PACS+ASCS discovered, stereo locations resolved, no control writes.",
        snapshot.members.len(), snapshot.set_size
    );
    Ok(())
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
