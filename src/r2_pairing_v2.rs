use crate::{
    error::{AppError, Result},
    usb,
};
use std::time::Duration;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PAIRING_TIMEOUT: Duration = Duration::from_secs(30);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

pub fn parse_seconds(value: &str) -> Result<u16> {
    let seconds: u16 = value.parse().map_err(|_| {
        AppError::Protocol("pairing-v2 scan duration must be an integer (1..=30)".into())
    })?;
    if !(1..=30).contains(&seconds) {
        return Err(AppError::Protocol(
            "pairing-v2 scan duration must be 1..=30 seconds".into(),
        ));
    }
    Ok(seconds)
}

pub fn archive_reset(confirm: bool) -> Result<()> {
    if !confirm {
        return Err(AppError::Usage(
            "r2 pairing-v2-reset requires --confirm".into(),
        ));
    }

    println!("R2 pairing-v2 archive/reset");
    println!("Action: archive BE200AudioHub-managed peer keys and set metadata, then create a fresh persistent local identity.");
    println!("Windows Bluetooth pairing state is not modified.");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    println!("Attach: PASS");

    let identity = attached
        .read_controller_identity(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!(
            "Read controller identity failed: {error}"
        )))?;
    let (store, archive) = bumble_adapter::BondStore::reseed_for_controller(&identity)
        .map_err(|error| AppError::Protocol(format!(
            "pairing-v2 reset failed: {error}"
        )))?;

    println!("Controller identity: {}", identity.public_address);
    println!("Archive: {}", archive.display());
    println!("Fresh local identity: {}", store.local_identity_path().display());
    println!("pairing-v2 reset COMPLETE");
    Ok(())
}

/// Replacement end-to-end pairing runner.
///
/// Primary and set-member bonds both remain provisional until encrypted CSIS
/// metadata has been read. Later members additionally require exact SIRK and
/// Set Size verification before any persistent key write.
pub fn onboard(seconds: u16, wanted_name: &str) -> Result<()> {
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage(
            "--name requires a nonempty device name".into(),
        ));
    }

    println!("R2 pairing-v2: transactional coordinated-set onboarding");
    println!("Target name: {wanted_name:?}");
    println!("Architecture: Swift Pair primary discovery -> generic FAL connection -> provisional SMP -> encrypted CSIS verification -> commit.");
    println!("Invariant: no peer key is persistent before the required CSIS gate passes.");
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

    let mut store = bumble_adapter::BondStore::open_for_controller(&controller_identity)
        .map_err(|error| AppError::Protocol(format!("Bond store open failed: {error}")))?;
    if !store.has_persistent_local_identity() {
        return Err(AppError::Protocol(
            "Legacy bond state has no persistent local identity. Run `be200hub r2 pairing-v2-reset --confirm` once, put the Buds3 Pro into formal pairing mode, then rerun pairing-v2-onboard.".into()
        ));
    }

    let resolver_count = attached
        .install_bond_store(&store)
        .map_err(|error| AppError::Protocol(format!("Bond store install failed: {error}")))?;
    println!(
        "Store: keys={} resolver_keys={} local_identity=loaded",
        store
            .pairing_key_count()
            .map_err(|error| AppError::Protocol(format!("Bond store inspect failed: {error}")))?,
        resolver_count
    );

    if store.set_summary(wanted_name).members.is_empty() {
        pair_primary(
            &mut attached,
            &mut store,
            &controller_identity.public_address,
            seconds,
            wanted_name,
        )?;
    }

    loop {
        let summary = store.set_summary(wanted_name);
        let expected_sirk = store
            .set_sirk(wanted_name)
            .map_err(|error| AppError::Protocol(format!("Stored SIRK invalid: {error}")))?
            .ok_or_else(|| AppError::Protocol(
                "primary is committed but expected SIRK is missing".into()
            ))?;
        let expected_size = summary.set_size.ok_or_else(|| {
            AppError::Protocol("primary CSIS report did not include Set Size".into())
        })?;

        println!(
            "Set state: committed={}/{} SIRK=known",
            summary.members.len(),
            expected_size
        );
        for (index, member) in summary.members.iter().enumerate() {
            println!(
                "  member[{index}] identity={} rank={} verified={}",
                member.identity_address,
                member
                    .rank
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "?".into()),
                member.csis_verified
            );
        }

        if summary.members.len() >= usize::from(expected_size)
            && summary.members.iter().all(|member| member.csis_verified)
        {
            println!(
                "R2 pairing-v2 COMPLETE: {} CSIS-verified member(s) committed",
                summary.members.len()
            );
            return Ok(());
        }

        pair_next_set_member(
            &mut attached,
            &mut store,
            &controller_identity.public_address,
            seconds,
            wanted_name,
            expected_sirk,
            Some(expected_size),
        )?;
    }
}

fn pair_primary(
    attached: &mut bumble_adapter::AttachedController,
    store: &mut bumble_adapter::BondStore,
    controller_public_address: &str,
    seconds: u16,
    wanted_name: &str,
) -> Result<()> {
    println!("Primary stage: scanning for pairing-only Microsoft beacon ({seconds}s)...");
    let scan = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Primary scan failed: {error}")))?;

    let mut candidates = scan
        .observations
        .iter()
        .filter(|observation| observation.is_connectable)
        .filter(|observation| swift_pair_matches(observation, wanted_name))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.rssi));
    let selected = candidates.first().ok_or_else(|| {
        AppError::Protocol(format!(
            "No pairing-only Microsoft beacon matched {wanted_name:?}; enter formal Buds3 Pro pairing mode"
        ))
    })?;

    println!(
        "Primary ExpectedDevice: addr={} type={} RSSI={}dBm",
        selected.address, selected.address_type, selected.rssi
    );

    let acl = {
        let mut manager = attached.le_connection_manager();
        manager
            .direct_connect_filter_accept_list(selected, CONNECT_TIMEOUT)
            .map_err(|error| AppError::Protocol(format!(
                "Primary generic direct connection failed: {error}"
            )))?
    };
    println!("Primary ACL: PASS handle=0x{:04X}", acl.connection_handle);

    let provisional = {
        let mut manager = attached.le_bond_manager();
        manager
            .pair_provisional(
                acl.connection_handle,
                controller_public_address,
                0,
                None,
                store,
                PAIRING_TIMEOUT,
            )
            .map_err(|error| AppError::Protocol(format!(
                "Primary provisional bond failed: {error}"
            )))?
    };

    let csis = attached
        .read_csis_member_with_ltk(
            acl.connection_handle,
            provisional.ltk(),
            VERIFY_TIMEOUT,
        )
        .map_err(|error| AppError::Protocol(format!(
            "Primary encrypted CSIS discovery failed: {error}"
        )))?;
    println!(
        "Primary CSIS: set_size={} rank={}",
        csis.set_size
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into()),
        csis.rank
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into())
    );

    let committed = bumble_adapter::commit_primary_bond(
        store,
        wanted_name,
        provisional,
        &csis,
    )
    .map_err(|error| AppError::Protocol(format!(
        "Primary transaction commit failed: {error}"
    )))?;
    attached
        .install_bond_store(store)
        .map_err(|error| AppError::Protocol(format!(
            "Address resolver refresh after primary commit failed: {error}"
        )))?;

    let reason = attached
        .disconnect_le_connection(acl.connection_handle, 0x13, DISCONNECT_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!(
            "Primary disconnect failed: {error}"
        )))?;
    println!(
        "Primary committed: identity={} rank={} disconnect=0x{reason:02X}",
        committed.identity_address,
        committed
            .rank
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into())
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn pair_next_set_member(
    attached: &mut bumble_adapter::AttachedController,
    store: &mut bumble_adapter::BondStore,
    controller_public_address: &str,
    seconds: u16,
    wanted_name: &str,
    expected_sirk: [u8; 16],
    expected_set_size: Option<u8>,
) -> Result<()> {
    println!("Set-member stage: scanning RSI candidates ({seconds}s)...");
    let scan = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Set-member scan failed: {error}")))?;

    let mut candidates = Vec::new();
    for observation in scan
        .observations
        .iter()
        .filter(|observation| observation.is_connectable)
        .filter(|observation| observation.name.as_deref() == Some(wanted_name))
    {
        let Some(rsi) = observation.rsi.as_ref() else {
            continue;
        };
        if !bumble_adapter::rsi_matches_sirk(&expected_sirk, rsi)
            .map_err(|error| AppError::Protocol(format!(
                "RSI validation failed: {error}"
            )))?
        {
            continue;
        }

        let resolved = store
            .resolve_observation_identity(observation)
            .map_err(|error| AppError::Protocol(format!(
                "RPA resolution failed: {error}"
            )))?;
        if resolved
            .as_ref()
            .is_some_and(|identity| store.is_member_of(wanted_name, identity))
        {
            println!(
                "  skip committed member via addr={} identity={}",
                observation.address,
                resolved.as_deref().unwrap_or("?")
            );
            continue;
        }

        println!(
            "  ExpectedSetMember addr={} type={} RSSI={}dBm RSI={}",
            observation.address,
            observation.address_type,
            observation.rssi,
            hex_bytes(rsi)
        );
        candidates.push(observation.clone());
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.rssi));
    let selected = candidates.first().ok_or_else(|| {
        AppError::Protocol(
            "No uncommitted RSI-matching set member was visible; re-enter formal pairing mode and retry"
                .into(),
        )
    })?;

    let acl = {
        let mut manager = attached.le_connection_manager();
        manager
            .direct_connect_filter_accept_list(selected, CONNECT_TIMEOUT)
            .map_err(|error| AppError::Protocol(format!(
                "Set-member generic direct connection failed: {error}"
            )))?
    };
    println!("Set-member ACL: PASS handle=0x{:04X}", acl.connection_handle);

    let expected_identity = (selected.address_type == 0).then(|| {
        selected
            .address
            .split('/')
            .next()
            .unwrap_or(&selected.address)
            .to_string()
    });

    let provisional = {
        let mut manager = attached.le_bond_manager();
        manager
            .pair_provisional(
                acl.connection_handle,
                controller_public_address,
                0,
                expected_identity.as_deref(),
                store,
                PAIRING_TIMEOUT,
            )
            .map_err(|error| AppError::Protocol(format!(
                "Set-member provisional bond failed: {error}"
            )))?
    };

    let verification = attached
        .verify_provisional_set_member(
            acl.connection_handle,
            &provisional,
            expected_sirk,
            expected_set_size,
            VERIFY_TIMEOUT,
        )
        .map_err(|error| AppError::Protocol(format!(
            "Set-member CSIS verification transport failed: {error}"
        )))?;

    let verified = match verification {
        bumble_adapter::SetMemberVerification::Verified(report) => report,
        bumble_adapter::SetMemberVerification::Rejected { reason, .. } => {
            let _ = attached.disconnect_le_connection(
                acl.connection_handle,
                0x13,
                DISCONNECT_TIMEOUT,
            );
            return Err(AppError::Protocol(format!(
                "Set-member rejected before commit: {reason}; provisional keys were discarded"
            )));
        }
    };

    let committed = bumble_adapter::commit_verified_set_member(
        store,
        wanted_name,
        provisional,
        &verified,
    )
    .map_err(|error| AppError::Protocol(format!(
        "Set-member transaction commit failed: {error}"
    )))?;
    attached
        .install_bond_store(store)
        .map_err(|error| AppError::Protocol(format!(
            "Address resolver refresh after member commit failed: {error}"
        )))?;

    let reason = attached
        .disconnect_le_connection(acl.connection_handle, 0x13, DISCONNECT_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!(
            "Set-member disconnect failed: {error}"
        )))?;
    println!(
        "Set member committed: identity={} rank={} disconnect=0x{reason:02X}",
        committed.identity_address,
        committed
            .rank
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into())
    );
    Ok(())
}

fn swift_pair_matches(
    observation: &bumble_adapter::ScanObservation,
    wanted_name: &str,
) -> bool {
    let mut offset = 0usize;
    let data = &observation.raw_data;
    while offset < data.len() {
        let count = usize::from(data[offset]);
        if count == 0 {
            break;
        }
        let Some(end) = offset.checked_add(count + 1) else {
            return false;
        };
        if count < 1 || end > data.len() {
            return false;
        }
        let kind = data[offset + 1];
        let value = &data[offset + 2..end];
        if kind == 0xFF && value.len() >= 5 {
            let company = u16::from_le_bytes([value[0], value[1]]);
            if company == 0x0006 {
                let payload = &value[2..];
                if payload[0] != 0x03 || payload[1] != 0x02 {
                    return false;
                }
                let name_start = if payload.len() >= 6 { 6 } else { 3 };
                if payload.len() <= name_start {
                    return false;
                }
                let name = String::from_utf8_lossy(&payload[name_start..])
                    .trim_matches(char::from(0))
                    .to_string();
                return name == wanted_name;
            }
        }
        offset = end;
    }
    false
}

fn hex_bytes(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}
