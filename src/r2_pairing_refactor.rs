use crate::{
    error::{AppError, Result},
    usb,
};
use std::time::Duration;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ACL_LINGER: Duration = Duration::from_secs(2);

pub fn parse_seconds(value: &str) -> Result<u16> {
    let seconds: u16 = value.parse().map_err(|_| {
        AppError::Protocol("pairing-refactor scan duration must be an integer (1..=30)".into())
    })?;
    if !(1..=30).contains(&seconds) {
        return Err(AppError::Protocol(
            "pairing-refactor scan duration must be 1..=30 seconds".into(),
        ));
    }
    Ok(seconds)
}

/// Phase-2 HIL for the replacement pairing architecture.
///
/// This command intentionally performs only:
///
/// stored set context -> passive scan -> RSI candidate selection -> generic
/// Filter Accept List direct connection -> linger -> disconnect.
///
/// It sends no SMP traffic and performs no BondStore mutation.
pub fn set_member_connect_probe(seconds: u16, wanted_name: &str) -> Result<()> {
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage(
            "--name requires a nonempty device name".into(),
        ));
    }

    println!("R2 pairing-refactor Phase 2: generic Set Member connection probe");
    println!("Target name: {wanted_name:?}");
    println!("Policy: RSI discovers one ExpectedSetMember; LeConnectionManager owns HCI initiating policy.");
    println!("Safety: no SMP, no GATT, no bond creation, no key-store or set-metadata mutation.");
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
    let resolving_keys = attached
        .install_bond_store(&store)
        .map_err(|error| AppError::Protocol(format!("Bond store install failed: {error}")))?;

    let set = store.set_summary(wanted_name);
    let sirk = store
        .set_sirk(wanted_name)
        .map_err(|error| AppError::Protocol(format!("Stored SIRK invalid: {error}")))?
        .ok_or_else(|| AppError::Protocol(format!(
            "No stored CSIS SIRK exists for {wanted_name:?}; complete primary bonding/CSIS discovery first"
        )))?;

    if set.members.is_empty() {
        return Err(AppError::Protocol(format!(
            "Stored SIRK exists for {wanted_name:?}, but no committed primary member is recorded"
        )));
    }

    println!(
        "Set context: committed_members={} set_size={} IRK_resolver_keys={}",
        set.members.len(),
        set.set_size
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into()),
        resolving_keys
    );
    for (index, member) in set.members.iter().enumerate() {
        println!(
            "  member[{index}] identity={} rank={} csis_verified={}",
            member.identity_address,
            member
                .rank
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into()),
            member.csis_verified
        );
    }

    println!("Scanning for ExpectedSetMember ({seconds}s)...");
    let scan = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;
    println!(
        "Scan: {} advertisements, {} retained advertiser(s)",
        scan.reports_seen,
        scan.observations.len()
    );

    let mut candidates = Vec::<bumble_adapter::ScanObservation>::new();

    for observation in scan
        .observations
        .iter()
        .filter(|observation| observation.is_connectable)
        .filter(|observation| observation.name.as_deref() == Some(wanted_name))
    {
        let resolved = store
            .resolve_observation_identity(observation)
            .map_err(|error| AppError::Protocol(format!("RPA resolution failed: {error}")))?;
        let rsi_result = match observation.rsi.as_ref() {
            Some(rsi) => bumble_adapter::rsi_matches_sirk(&sirk, rsi)
                .map_err(|error| AppError::Protocol(format!(
                    "CSIS RSI validation failed: {error}"
                )))?,
            None => false,
        };

        println!(
            "  observed addr={} type={} RSSI={}dBm RSI={} result={} resolved_identity={}",
            observation.address,
            observation.address_type,
            observation.rssi,
            observation
                .rsi
                .as_ref()
                .map(|value| hex_bytes(value))
                .unwrap_or_else(|| "absent".into()),
            if rsi_result { "MATCH" } else { "NO-MATCH" },
            resolved.as_deref().unwrap_or("unresolved")
        );

        if !rsi_result {
            continue;
        }

        if resolved.as_ref().is_some_and(|identity| {
            store.is_member_of(wanted_name, identity)
        }) {
            println!(
                "    skip: advertisement resolves to an already committed set member"
            );
            continue;
        }

        candidates.push(observation.clone());
    }

    if candidates.is_empty() {
        return Err(AppError::Protocol(format!(
            "No uncommitted connectable advertiser for {wanted_name:?} had an RSI matching the stored SIRK"
        )));
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.rssi));
    println!("ExpectedSetMember candidate(s):");
    for (index, candidate) in candidates.iter().enumerate() {
        println!(
            "  [{index}] addr={} type={} RSSI={}dBm SID={} RSI={}",
            candidate.address,
            candidate.address_type,
            candidate.rssi,
            candidate.sid,
            candidate
                .rsi
                .as_ref()
                .map(|value| hex_bytes(value))
                .unwrap_or_else(|| "absent".into())
        );
    }

    let selected = &candidates[0];
    println!(
        "Selecting strongest ExpectedSetMember: {} type={} RSSI={}dBm",
        selected.address, selected.address_type, selected.rssi
    );
    println!("Discovery phase complete; generic LeConnectionManager takes ownership.");

    let report = {
        let mut manager = attached.le_connection_manager();
        manager
            .direct_connect_filter_accept_list(selected, CONNECT_TIMEOUT)
            .map_err(|error| AppError::Protocol(format!(
                "Generic Set Member direct connection failed: {error}"
            )))?
    };

    println!("Generic direct connection: PASS");
    println!(
        "  handle=0x{:04X} role={} controller_peer={} interval={} ({} ms) latency={} supervision={} ({} ms)",
        report.connection_handle,
        if report.role == 0 { "central" } else { "peripheral" },
        report.peer_address,
        report.connection_interval,
        f64::from(report.connection_interval) * 1.25,
        report.peripheral_latency,
        report.supervision_timeout,
        u32::from(report.supervision_timeout) * 10
    );

    println!(
        "Holding ACL for {}s without SMP/GATT...",
        ACL_LINGER.as_secs()
    );
    attached
        .linger_le_connection(report.connection_handle, ACL_LINGER)
        .map_err(|error| AppError::Protocol(format!(
            "Generic Set Member ACL linger failed: {error}"
        )))?;
    println!("ACL remained established: PASS");

    let reason = attached
        .disconnect_le_connection(
            report.connection_handle,
            0x13,
            DISCONNECT_TIMEOUT,
        )
        .map_err(|error| AppError::Protocol(format!(
            "Generic Set Member disconnect failed: {error}"
        )))?;
    println!("Disconnect: PASS reason=0x{reason:02X}");
    println!("R2 Phase-2 RESULT: RSI candidate -> generic Filter Accept List direct connection succeeded; no pairing state was modified.");
    Ok(())
}

fn hex_bytes(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}
