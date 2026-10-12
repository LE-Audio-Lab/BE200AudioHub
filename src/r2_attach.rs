use crate::{
    error::{AppError, Result},
    usb,
};
use std::io::{self, Write};
use std::time::Duration;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ACL_LINGER: Duration = Duration::from_secs(2);
const PAIRING_TIMEOUT: Duration = Duration::from_secs(30);
const RESUME_TIMEOUT: Duration = Duration::from_secs(10);
const TARGET_SET_MEMBERS: usize = 2;

pub fn probe() -> Result<()> {
    println!("R2.1 attach probe");
    println!("Target: Intel BE200 8087:0036 via Bumble-RS USB transport");
    println!("Policy: preserve existing Operational firmware; HCI Reset is forbidden");
    usb::r2_fast_preflight()?;
    println!("Opening Bumble-RS USB transport and initializing Host state...");

    let attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;

    let report = attached.report();
    println!("Attach: PASS");
    println!("Bumble upstream: {}", bumble_adapter::UPSTREAM_REVISION);
    println!("Host state: {:?}", attached.host_state());
    if let (Some(hci), Some(sub), Some(mfg)) = (
        report.hci_version,
        report.hci_subversion,
        report.manufacturer,
    ) {
        println!(
            "Controller version: HCI=0x{hci:02X} subversion=0x{sub:04X} manufacturer={mfg}"
        );
    }
    println!(
        "LE ACL buffer: {} B x {}",
        report.le_acl_data_packet_length,
        report.total_num_le_acl_data_packets
    );
    println!(
        "ISO buffer: {} B x {}",
        report.iso_data_packet_length,
        report.total_num_iso_data_packets
    );
    println!(
        "Max extended advertising data: {} B",
        report.maximum_advertising_data_length
    );
    println!("No HCI Reset or firmware reload was issued by this R2.1 path.");
    Ok(())
}

pub fn parse_scan_seconds(value: &str) -> Result<u16> {
    let seconds: u16 = value
        .parse()
        .map_err(|_| AppError::Protocol("R2 scan duration must be an integer (1..=30 seconds)".into()))?;
    if !(1..=30).contains(&seconds) {
        return Err(AppError::Protocol(
            "R2 scan duration must be 1..=30 seconds".into(),
        ));
    }
    Ok(seconds)
}

pub fn scan(seconds: u16, show_addresses: bool) -> Result<()> {
    println!("R2.2 Bumble passive BLE scan");
    println!("Target: Intel BE200 8087:0036; LE 1M; {seconds}s");
    println!("Policy: no HCI Reset, no firmware reload, no connection, no pairing, no SCAN_REQ");
    usb::r2_fast_preflight()?;
    println!("Attaching to existing Operational controller...");

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    println!("Attach: PASS");

    let summary = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;

    println!(
        "Scan: PASS — {} Bumble advertisements, {} unique advertisers",
        summary.reports_seen,
        summary.observations.len()
    );
    if !show_addresses {
        println!("Addresses hidden by default; use --show-addresses for HIL diagnostics.");
    }

    for (index, observation) in summary.observations.iter().enumerate() {
        let name = observation.name.as_deref().unwrap_or("(unnamed)");
        let sid = if observation.sid == 0xFF {
            "-".into()
        } else {
            observation.sid.to_string()
        };
        let address = if show_addresses {
            format!(" addr={} type={}", observation.address, observation.address_type)
        } else {
            String::new()
        };
        println!(
            "  #{:03} RSSI={}dBm PHY={}/{} SID={} legacy={} conn={} scan={} data={}B name={}{}",
            index + 1,
            observation.rssi,
            observation.primary_phy,
            observation.secondary_phy,
            sid,
            observation.is_legacy,
            observation.is_connectable,
            observation.is_scannable,
            observation.data_len,
            name,
            address
        );
    }
    println!("LE Extended Scan disabled: OK");
    Ok(())
}

pub fn connect_probe(seconds: u16, wanted_name: &str) -> Result<()> {
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage("--name requires a nonempty device name".into()));
    }

    println!("R2.3 LE ACL connect/disconnect probe");
    println!("Target name: {wanted_name:?}");
    println!("Policy: same-process scan -> explicit target -> LE Extended Create Connection");
    println!("No SMP, pairing, GATT, PACS, ASCS, CIG, CIS, or ISO traffic will be started.");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    println!("Attach: PASS");

    println!("Scanning for {seconds}s...");
    let summary = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;
    println!("Scan disabled: OK");

    let candidates: Vec<_> = summary
        .observations
        .iter()
        .filter(|observation| {
            observation.is_connectable
                && observation.name.as_deref() == Some(wanted_name)
        })
        .cloned()
        .collect();

    if candidates.is_empty() {
        return Err(AppError::Protocol(format!(
            "No connectable advertisement matched {wanted_name:?} in this scan"
        )));
    }

    println!("Matching connectable candidates:");
    for (index, candidate) in candidates.iter().enumerate() {
        println!(
            "  [{index}] {} type={} RSSI={}dBm PHY={}/{} SID={}",
            candidate.address,
            candidate.address_type,
            candidate.rssi,
            candidate.primary_phy,
            candidate.secondary_phy,
            candidate.sid
        );
    }

    let selected_index = if candidates.len() == 1 {
        println!("Only one candidate; selecting [0].");
        0
    } else {
        choose_candidate(candidates.len())?
    };
    let selected = &candidates[selected_index];

    println!(
        "Selected [{selected_index}] {} type={} RSSI={}dBm",
        selected.address, selected.address_type, selected.rssi
    );
    println!("Submitting LE Extended Create Connection (own address: public)...");

    let report = attached
        .connect_scanned_observation(selected, CONNECT_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("LE ACL connect failed: {error}")))?;

    println!("ACL: PASS");
    println!(
        "  handle=0x{:04X} role={} peer={}",
        report.connection_handle,
        if report.role == 0 { "central" } else { "peripheral" },
        report.peer_address
    );
    println!(
        "  interval={} ({} ms) latency={} supervision_timeout={} ({} ms)",
        report.connection_interval,
        f64::from(report.connection_interval) * 1.25,
        report.peripheral_latency,
        report.supervision_timeout,
        u32::from(report.supervision_timeout) * 10
    );

    println!("Holding ACL for {}s while pumping HCI...", ACL_LINGER.as_secs());
    attached
        .linger_le_connection(report.connection_handle, ACL_LINGER)
        .map_err(|error| AppError::Protocol(format!("LE ACL linger failed: {error}")))?;
    println!("ACL remained established: PASS");

    println!("Submitting controlled HCI Disconnect (reason 0x13)...");
    let reason = attached
        .disconnect_le_connection(
            report.connection_handle,
            0x13,
            DISCONNECT_TIMEOUT,
        )
        .map_err(|error| AppError::Protocol(format!("LE ACL disconnect failed: {error}")))?;

    println!(
        "Disconnect: PASS — handle=0x{:04X} reason=0x{reason:02X}",
        report.connection_handle
    );
    println!("R2.3 RESULT: same-process scan -> real LE ACL -> controlled disconnect succeeded.");
    Ok(())
}


pub fn bond_onboard(seconds: u16, wanted_name: &str) -> Result<()> {
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage("--name requires a nonempty device name".into()));
    }

    println!("R2.4 persistent dual-member bond onboarding");
    println!("Target name: {wanted_name:?}");
    println!("Invariant: existing bonds are never deleted or re-paired automatically.");
    println!("Goal: persist up to {TARGET_SET_MEMBERS} members, one SMP bond per physical member.");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    println!("Attach: PASS");

    let identity = attached
        .read_controller_identity(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Read controller identity failed: {error}")))?;
    println!("Controller identity: {}", identity.public_address);

    let mut store = bumble_adapter::BondStore::open_for_controller(&identity)
        .map_err(|error| AppError::Protocol(format!("Bond store open failed: {error}")))?;
    let resolving_keys = attached
        .install_bond_store(&store)
        .map_err(|error| AppError::Protocol(format!("Bond store install failed: {error}")))?;

    println!("Bond root: {}", store.root().display());
    println!("Pairing keys: {}", store.pairing_keys_path().display());
    println!("Set metadata: {}", store.set_db_path().display());
    println!("IRK resolver loaded: {resolving_keys} key(s)");

    loop {
        let current = store.set_summary(wanted_name);
        println!(
            "Persistent set state: {}/{} member(s)",
            current.members.len(),
            TARGET_SET_MEMBERS
        );
        for (index, member) in current.members.iter().enumerate() {
            println!(
                "  bonded[{index}] identity={} type={}",
                member.identity_address, member.address_type
            );
        }

        if current.members.len() >= TARGET_SET_MEMBERS {
            println!("R2.4 onboarding already complete; no pairing was attempted.");
            return Ok(());
        }

        println!("Scanning for {seconds}s to find an unbonded member...");
        let scan = attached
            .scan_passive_extended(seconds)
            .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;
        println!(
            "Scan: {} advertisements, {} unique advertisers",
            scan.reports_seen,
            scan.observations.len()
        );

        let mut fresh = Vec::new();
        let mut metadata_changed = false;

        for observation in scan.observations.iter().filter(|observation| {
            observation.is_connectable
                && observation.name.as_deref() == Some(wanted_name)
        }) {
            match store
                .resolve_observation_identity(observation)
                .map_err(|error| AppError::Protocol(format!("RPA resolution failed: {error}")))?
            {
                Some(identity_address) => {
                    if store.is_member_of(wanted_name, &identity_address) {
                        println!(
                            "  skip known member {} via current RPA {}",
                            identity_address, observation.address
                        );
                    } else if store
                        .adopt_existing_bond(wanted_name, &identity_address)
                        .map_err(|error| AppError::Protocol(format!(
                            "Failed to adopt existing bond: {error}"
                        )))?
                    {
                        println!(
                            "  recovered orphan bond {} from existing pairing keys; no SMP pairing needed",
                            identity_address
                        );
                        metadata_changed = true;
                    }
                }
                None => fresh.push(observation.clone()),
            }
        }

        if metadata_changed {
            let loaded = attached
                .install_bond_store(&store)
                .map_err(|error| AppError::Protocol(format!(
                    "Failed to refresh recovered bond resolver: {error}"
                )))?;
            println!("Bond metadata repaired; resolver now has {loaded} key(s).");
            continue;
        }

        let current = store.set_summary(wanted_name);
        if current.members.len() >= TARGET_SET_MEMBERS {
            println!("R2.4 onboarding complete from existing persistent bonds.");
            return Ok(());
        }

        if fresh.is_empty() {
            return Err(AppError::Protocol(format!(
                "No unbonded connectable {wanted_name:?} member was visible. {} member(s) are already safely persisted; do not delete them. Re-enter earbud pairing mode and rerun this same command.",
                current.members.len()
            )));
        }

        println!("Unbonded matching candidates:");
        for (index, candidate) in fresh.iter().enumerate() {
            println!(
                "  [{index}] {} type={} RSSI={}dBm PHY={}/{} SID={}",
                candidate.address,
                candidate.address_type,
                candidate.rssi,
                candidate.primary_phy,
                candidate.secondary_phy,
                candidate.sid
            );
        }

        let selected_index = if fresh.len() == 1 {
            println!("Only one unbonded candidate; selecting [0].");
            0
        } else {
            choose_candidate(fresh.len())?
        };
        let selected = &fresh[selected_index];

        println!(
            "Connecting unbonded candidate [{selected_index}] {}...",
            selected.address
        );
        let acl = attached
            .connect_scanned_observation(selected, CONNECT_TIMEOUT)
            .map_err(|error| AppError::Protocol(format!("LE ACL connect failed: {error}")))?;
        println!(
            "ACL: PASS handle=0x{:04X} peer={}",
            acl.connection_handle, acl.peer_address
        );

        println!("Starting one-time SMP Secure Connections bonding...");
        let paired = attached
            .pair_and_persist(
                acl.connection_handle,
                &identity.public_address,
                wanted_name,
                &mut store,
                PAIRING_TIMEOUT,
            )
            .map_err(|error| AppError::Protocol(format!("SMP/bond failed: {error}")))?;

        println!("Bond persisted: PASS");
        println!(
            "  identity={} type={} encrypted={} authenticated={} LTK={} IRK={}",
            paired.identity_address,
            paired.address_type,
            paired.encrypted,
            paired.authenticated,
            paired.has_ltk,
            paired.has_irk
        );

        println!("Disconnecting bonded member before searching for the next one...");
        let reason = attached
            .disconnect_le_connection(acl.connection_handle, 0x13, DISCONNECT_TIMEOUT)
            .map_err(|error| AppError::Protocol(format!(
                "Post-bond disconnect failed: {error}"
            )))?;
        println!("Disconnect: PASS reason=0x{reason:02X}");

        let current = store.set_summary(wanted_name);
        if current.members.len() < TARGET_SET_MEMBERS {
            println!(
                "Member persisted. Continuing directly to member {}/{} without re-pairing existing members.",
                current.members.len() + 1,
                TARGET_SET_MEMBERS
            );
        }
    }
}

pub fn bond_verify(seconds: u16, wanted_name: &str) -> Result<()> {
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage("--name requires a nonempty device name".into()));
    }

    println!("R2.4 stored-bond reconnect verification");
    println!("Target name: {wanted_name:?}");
    println!("Policy: no SMP pairing is allowed on this path.");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    let identity = attached
        .read_controller_identity(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Read controller identity failed: {error}")))?;
    let store = bumble_adapter::BondStore::open_for_controller(&identity)
        .map_err(|error| AppError::Protocol(format!("Bond store open failed: {error}")))?;
    attached
        .install_bond_store(&store)
        .map_err(|error| AppError::Protocol(format!("Bond store install failed: {error}")))?;

    let set = store.set_summary(wanted_name);
    if set.members.is_empty() {
        return Err(AppError::Protocol(format!(
            "No persistent members are recorded for {wanted_name:?}"
        )));
    }

    println!("Verifying {} persisted member(s)...", set.members.len());

    for (index, member) in set.members.iter().enumerate() {
        println!(
            "Member {}/{} identity={}",
            index + 1,
            set.members.len(),
            member.identity_address
        );
        println!("  scanning for current RPA ({seconds}s)...");

        let scan = attached
            .scan_passive_extended(seconds)
            .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;

        let mut matches = Vec::new();
        for observation in scan.observations.iter().filter(|observation| {
            observation.is_connectable
                && observation.name.as_deref() == Some(wanted_name)
        }) {
            let resolved = store
                .resolve_observation_identity(observation)
                .map_err(|error| AppError::Protocol(format!(
                    "RPA resolution failed: {error}"
                )))?;
            if resolved
                .as_deref()
                .is_some_and(|identity| identity.eq_ignore_ascii_case(&member.identity_address))
            {
                matches.push(observation.clone());
            }
        }

        let selected = matches
            .into_iter()
            .max_by_key(|observation| observation.rssi)
            .ok_or_else(|| AppError::Protocol(format!(
                "Persisted member {} was not visible/resolvable in this scan",
                member.identity_address
            )))?;

        println!(
            "  resolved current RPA {} -> {}",
            selected.address, member.identity_address
        );

        let acl = attached
            .connect_scanned_observation(&selected, CONNECT_TIMEOUT)
            .map_err(|error| AppError::Protocol(format!(
                "Stored-bond ACL connect failed: {error}"
            )))?;
        println!("  ACL: PASS handle=0x{:04X}", acl.connection_handle);

        let resumed = attached
            .resume_bonded_encryption(
                acl.connection_handle,
                &member.identity_address,
                &store,
                RESUME_TIMEOUT,
            )
            .map_err(|error| AppError::Protocol(format!(
                "Stored-bond encryption failed: {error}"
            )))?;
        println!(
            "  encrypted from stored LTK: PASS identity={} handle=0x{:04X}",
            resumed.identity_address, resumed.connection_handle
        );

        attached
            .linger_le_connection(acl.connection_handle, Duration::from_secs(1))
            .map_err(|error| AppError::Protocol(format!(
                "Stored-bond ACL linger failed: {error}"
            )))?;

        let reason = attached
            .disconnect_le_connection(acl.connection_handle, 0x13, DISCONNECT_TIMEOUT)
            .map_err(|error| AppError::Protocol(format!(
                "Stored-bond disconnect failed: {error}"
            )))?;
        println!("  disconnect: PASS reason=0x{reason:02X}");
    }

    println!("R2.4 RESULT: all recorded members reconnected and encrypted without SMP pairing.");
    Ok(())
}

fn choose_candidate(count: usize) -> Result<usize> {
    loop {
        print!("Select target [0..{}]: ", count - 1);
        io::stdout()
            .flush()
            .map_err(|error| AppError::Protocol(format!("failed to flush prompt: {error}")))?;

        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|error| AppError::Protocol(format!("failed to read target selection: {error}")))?;

        match line.trim().parse::<usize>() {
            Ok(index) if index < count => return Ok(index),
            _ => println!("Invalid selection."),
        }
    }
}