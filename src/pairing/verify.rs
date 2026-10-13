use crate::{
    error::{AppError, Result},
    usb,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ENCRYPTION_TIMEOUT: Duration = Duration::from_secs(10);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreSnapshot {
    pairing_keys: FileFingerprint,
    set_metadata: FileFingerprint,
    local_identity: FileFingerprint,
    key_count: usize,
}

pub fn parse_seconds(value: &str) -> Result<u16> {
    let seconds: u16 = value.parse().map_err(|_| {
        AppError::Protocol("pairing-v2 verify scan duration must be an integer (1..=30)".into())
    })?;
    if !(1..=30).contains(&seconds) {
        return Err(AppError::Protocol(
            "pairing-v2 verify scan duration must be 1..=30 seconds".into(),
        ));
    }
    Ok(seconds)
}

/// Read-only verification of every committed coordinated-set member.
///
/// For each stored identity this command performs:
/// current-address discovery -> IRK resolution -> generic FAL connection ->
/// stored-LTK encryption -> encrypted CSIS read -> exact SIRK/size/rank check.
///
/// No SMP session is created and no persistent store API is called. File
/// fingerprints are compared before and after the full run as an additional
/// non-mutation assertion.
pub fn verify(seconds: u16, wanted_name: &str) -> Result<()> {
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage(
            "--name requires a nonempty device name".into(),
        ));
    }

    println!("R2 pairing-v2: stored coordinated-set verification");
    println!("Target name: {wanted_name:?}");
    println!("Path: IRK resolve -> generic FAL ACL -> stored-LTK encryption -> encrypted CSIS verification.");
    println!("Safety: no SMP Pairing Request, no new bond, no key/set/local-identity mutation.");
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
            "pairing-v2 verification requires a persistent local identity".into(),
        ));
    }

    let resolver_count = attached
        .install_bond_store(&store)
        .map_err(|error| AppError::Protocol(format!("Bond store install failed: {error}")))?;

    let set = store.set_summary(wanted_name);
    let expected_sirk = store
        .set_sirk(wanted_name)
        .map_err(|error| AppError::Protocol(format!("Stored SIRK invalid: {error}")))?
        .ok_or_else(|| AppError::Protocol(format!(
            "No stored CSIS SIRK exists for {wanted_name:?}"
        )))?;
    let expected_size = set.set_size.ok_or_else(|| {
        AppError::Protocol("Stored coordinated set has no Set Size".into())
    })?;

    if set.members.len() != usize::from(expected_size) {
        return Err(AppError::Protocol(format!(
            "Stored set is incomplete: {} committed member(s), Set Size={expected_size}",
            set.members.len()
        )));
    }
    if set.members.iter().any(|member| !member.csis_verified) {
        return Err(AppError::Protocol(
            "Stored set contains a member that is not CSIS-verified".into(),
        ));
    }

    let before = snapshot_store(&store)?;
    println!(
        "Store before: keys={} resolver_keys={} pairing={} set={} local={}",
        before.key_count,
        resolver_count,
        before.pairing_keys.state,
        before.set_metadata.state,
        before.local_identity.state
    );
    println!(
        "Set context: committed={}/{} SIRK=known",
        set.members.len(), expected_size
    );

    let mut members = set.members.clone();
    members.sort_by_key(|member| member.rank.unwrap_or(u8::MAX));

    for (index, member) in members.iter().enumerate() {
        println!();
        println!(
            "Member {}/{}: identity={} rank={} type={} — scanning current address ({}s)...",
            index + 1,
            members.len(),
            member.identity_address,
            member
                .rank
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into()),
            member.address_type,
            seconds
        );

        let scan = attached
            .scan_passive_extended(seconds)
            .map_err(|error| AppError::Protocol(format!(
                "Stored-member scan failed for {}: {error}",
                member.identity_address
            )))?;

        let mut candidates = Vec::new();
        for observation in scan
            .observations
            .iter()
            .filter(|observation| observation.is_connectable)
        {
            let resolved = store
                .resolve_observation_identity(observation)
                .map_err(|error| AppError::Protocol(format!(
                    "RPA resolution failed for {}: {error}",
                    observation.address
                )))?;
            if resolved.as_deref().is_some_and(|identity| {
                identity.eq_ignore_ascii_case(&member.identity_address)
            }) {
                println!(
                    "  resolved addr={} type={} RSSI={}dBm -> {}",
                    observation.address,
                    observation.address_type,
                    observation.rssi,
                    member.identity_address
                );
                candidates.push(observation.clone());
            }
        }

        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.rssi));
        let selected = candidates.first().ok_or_else(|| {
            AppError::Protocol(format!(
                "Stored member {} was not visible/resolvable in this scan",
                member.identity_address
            ))
        })?;

        println!(
            "  selected current transport addr={} type={} RSSI={}dBm",
            selected.address, selected.address_type, selected.rssi
        );

        let acl = {
            let mut manager = attached.le_connection_manager();
            manager
                .direct_connect_filter_accept_list(selected, CONNECT_TIMEOUT)
                .map_err(|error| AppError::Protocol(format!(
                    "Stored-member generic direct connection failed for {}: {error}",
                    member.identity_address
                )))?
        };
        println!("  ACL: PASS handle=0x{:04X}", acl.connection_handle);

        let verification = (|| -> Result<()> {
            attached
                .resume_bonded_encryption(
                    acl.connection_handle,
                    &member.identity_address,
                    &store,
                    ENCRYPTION_TIMEOUT,
                )
                .map_err(|error| AppError::Protocol(format!(
                    "Stored-LTK encryption failed for {}: {error}",
                    member.identity_address
                )))?;
            println!("  stored-LTK encryption: PASS");

            let csis = attached
                .read_csis_member(
                    acl.connection_handle,
                    &member.identity_address,
                    &store,
                    VERIFY_TIMEOUT,
                )
                .map_err(|error| AppError::Protocol(format!(
                    "Encrypted CSIS verification failed for {}: {error}",
                    member.identity_address
                )))?;

            if csis.sirk != expected_sirk {
                return Err(AppError::Protocol(format!(
                    "CSIS SIRK mismatch for {}",
                    member.identity_address
                )));
            }
            if csis.set_size != Some(expected_size) {
                return Err(AppError::Protocol(format!(
                    "CSIS Set Size mismatch for {}: expected {}, peer {:?}",
                    member.identity_address, expected_size, csis.set_size
                )));
            }
            if csis.rank != member.rank {
                return Err(AppError::Protocol(format!(
                    "CSIS rank mismatch for {}: stored {:?}, peer {:?}",
                    member.identity_address, member.rank, csis.rank
                )));
            }

            println!(
                "  CSIS: PASS SIRK=MATCH set_size={} rank={}",
                expected_size,
                csis.rank
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "?".into())
            );
            Ok(())
        })();

        let disconnect = attached.disconnect_le_connection(
            acl.connection_handle,
            0x13,
            DISCONNECT_TIMEOUT,
        );

        if let Err(error) = verification {
            let _ = disconnect;
            return Err(error);
        }

        let reason = disconnect.map_err(|error| AppError::Protocol(format!(
            "Stored-member disconnect failed for {}: {error}",
            member.identity_address
        )))?;
        println!("  disconnect: PASS reason=0x{reason:02X}");
    }

    let after = snapshot_store(&store)?;
    println!();
    println!(
        "Store after:  keys={} pairing={} set={} local={}",
        after.key_count,
        after.pairing_keys.state,
        after.set_metadata.state,
        after.local_identity.state
    );
    if before != after {
        return Err(AppError::Protocol(
            "pairing-v2 verification changed persistent state; refusing PASS".into(),
        ));
    }

    println!("Persistent-state fingerprint: UNCHANGED");
    println!(
        "R2 pairing-v2 VERIFY PASS: {}/{} member(s) resolved, connected through FAL, encrypted from stored LTK, and matched stored SIRK/Set Size/Rank without SMP.",
        members.len(), expected_size
    );
    Ok(())
}

fn snapshot_store(store: &bumble_adapter::BondStore) -> Result<StoreSnapshot> {
    Ok(StoreSnapshot {
        pairing_keys: fingerprint(&store.pairing_keys_path())?,
        set_metadata: fingerprint(&store.set_db_path())?,
        local_identity: fingerprint(&store.local_identity_path())?,
        key_count: store
            .pairing_key_count()
            .map_err(|error| AppError::Protocol(format!(
                "Bond key count failed: {error}"
            )))?,
    })
}

fn fingerprint(path: &Path) -> Result<FileFingerprint> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let digest = Sha256::digest(&bytes);
            Ok(FileFingerprint {
                state: format!("{}B:{}", bytes.len(), hex_lower(&digest)),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileFingerprint {
            state: "missing".into(),
        }),
        Err(error) => Err(AppError::Protocol(format!(
            "Failed to fingerprint {}: {error}",
            path.display()
        ))),
    }
}

fn hex_lower(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
