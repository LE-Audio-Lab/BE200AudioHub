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


pub fn scan_forensics(seconds: u16) -> Result<()> {
    println!("R2 forensic passive advertising capture");
    println!("Target: Intel BE200 8087:0036; LE 1M; {seconds}s");
    println!("Policy: passive scan only; no connection, pairing, SCAN_REQ, GATT, CIG/CIS, or ISO.");
    println!("Purpose: fingerprint TWS members / RPA / identity-looking advertisers / case / vendor beacons.");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    println!("Attach: PASS");

    let summary = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Bumble forensic scan failed: {error}")))?;

    println!(
        "Forensic scan: PASS — {} reports, {} retained advertiser(s)",
        summary.reports_seen,
        summary.observations.len()
    );
    println!("NOTE: address/RPA classification below is transport evidence, not proof of physical identity.");

    for (index, observation) in summary.observations.iter().enumerate() {
        let name = observation.name.as_deref().unwrap_or("(unnamed)");
        let sid = if observation.sid == 0xFF {
            "-".into()
        } else {
            observation.sid.to_string()
        };
        println!();
        println!(
            "[ADV #{:03}] addr={} type={} ({}) RSSI={}dBm PHY={}/{} SID={} legacy={} connectable={} scannable={} name={}",
            index + 1,
            observation.address,
            observation.address_type,
            address_type_label(&observation.address, observation.address_type),
            observation.rssi,
            observation.primary_phy,
            observation.secondary_phy,
            sid,
            observation.is_legacy,
            observation.is_connectable,
            observation.is_scannable,
            name
        );
        println!(
            "  RSI={} raw={}B {}",
            observation
                .rsi
                .as_ref()
                .map(|value| hex_bytes(value))
                .unwrap_or_else(|| "absent".into()),
            observation.raw_data.len(),
            hex_bytes(&observation.raw_data)
        );

        print_ad_structures(&observation.raw_data);

        let uuids = advertised_16bit_uuids(&observation.raw_data);
        if !uuids.is_empty() {
            let rendered = uuids
                .iter()
                .map(|uuid| match known_uuid16(*uuid) {
                    Some(label) => format!("0x{uuid:04X}({label})"),
                    None => format!("0x{uuid:04X}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!("  derived 16-bit UUIDs: {rendered}");
        }

        let audio_hints = uuids
            .iter()
            .copied()
            .filter(|uuid| matches!(*uuid, 0x1846 | 0x184E | 0x1850 | 0x1851 | 0x1852 | 0x1853 | 0x1854 | 0x1855))
            .collect::<Vec<_>>();
        println!(
            "  derived hints: random_kind={} RSI={} LE-Audio-service-hint={}",
            random_address_kind(&observation.address, observation.address_type)
                .unwrap_or("n/a"),
            if observation.rsi.is_some() { "yes" } else { "no" },
            if audio_hints.is_empty() {
                "none-in-advertising".into()
            } else {
                audio_hints
                    .iter()
                    .map(|uuid| format!("0x{uuid:04X}"))
                    .collect::<Vec<_>>()
                    .join(",")
            }
        );
    }

    println!();
    println!("LE Extended Scan disabled: OK");
    println!("R2 forensic capture complete. No advertiser was connected or paired.");
    Ok(())
}

fn print_ad_structures(data: &[u8]) {
    if data.is_empty() {
        println!("  AD structures: (none)");
        return;
    }

    println!("  AD structures:");
    let mut offset = 0usize;
    let mut index = 0usize;
    while offset < data.len() {
        let count = usize::from(data[offset]);
        if count == 0 {
            println!("    [{index:02}] terminator at offset {offset}");
            break;
        }
        let Some(end) = offset.checked_add(count + 1) else {
            println!("    [{index:02}] malformed length overflow at offset {offset}");
            break;
        };
        if count < 1 || end > data.len() {
            println!(
                "    [{index:02}] malformed/truncated structure: offset={} declared={} remaining={}",
                offset,
                count,
                data.len().saturating_sub(offset + 1)
            );
            break;
        }

        let kind = data[offset + 1];
        let value = &data[offset + 2..end];
        let decoded = decode_ad_structure(kind, value);
        println!(
            "    [{index:02}] type=0x{kind:02X} {:<30} len={} value={}{}",
            ad_type_label(kind),
            value.len(),
            hex_bytes(value),
            if decoded.is_empty() {
                String::new()
            } else {
                format!("  => {decoded}")
            }
        );

        offset = end;
        index += 1;
    }
}

fn decode_ad_structure(kind: u8, value: &[u8]) -> String {
    match kind {
        0x01 if !value.is_empty() => format!("flags=0x{:02X}", value[0]),
        0x02 | 0x03 => decode_uuid16_list(value),
        0x08 | 0x09 => format!("name={:?}", String::from_utf8_lossy(value)),
        0x0A if !value.is_empty() => format!("tx_power={}dBm", value[0] as i8),
        0x16 if value.len() >= 2 => {
            let uuid = u16::from_le_bytes([value[0], value[1]]);
            let label = known_uuid16(uuid)
                .map(|label| format!(" ({label})"))
                .unwrap_or_default();
            format!(
                "service_uuid=0x{uuid:04X}{label} payload={}",
                hex_bytes(&value[2..])
            )
        }
        0x19 if value.len() >= 2 => {
            let appearance = u16::from_le_bytes([value[0], value[1]]);
            format!("appearance=0x{appearance:04X}")
        }
        0x2E if value.len() == 6 => format!("RSI={}", hex_bytes(value)),
        0xFF if value.len() >= 2 => {
            let company = u16::from_le_bytes([value[0], value[1]]);
            let label = match company {
                0x0006 => "Microsoft",
                0x0075 => "Samsung Electronics",
                _ => "unknown-company",
            };
            format!(
                "company=0x{company:04X} ({label}) payload={}",
                hex_bytes(&value[2..])
            )
        }
        _ => String::new(),
    }
}

fn advertised_16bit_uuids(data: &[u8]) -> Vec<u16> {
    let mut result = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let count = usize::from(data[offset]);
        if count == 0 {
            break;
        }
        let Some(end) = offset.checked_add(count + 1) else {
            break;
        };
        if count < 1 || end > data.len() {
            break;
        }
        let kind = data[offset + 1];
        let value = &data[offset + 2..end];
        if matches!(kind, 0x02 | 0x03) {
            for chunk in value.chunks_exact(2) {
                result.push(u16::from_le_bytes([chunk[0], chunk[1]]));
            }
        } else if kind == 0x16 && value.len() >= 2 {
            result.push(u16::from_le_bytes([value[0], value[1]]));
        }
        offset = end;
    }
    result.sort_unstable();
    result.dedup();
    result
}

fn decode_uuid16_list(value: &[u8]) -> String {
    let mut uuids = Vec::new();
    for chunk in value.chunks_exact(2) {
        let uuid = u16::from_le_bytes([chunk[0], chunk[1]]);
        uuids.push(match known_uuid16(uuid) {
            Some(label) => format!("0x{uuid:04X}({label})"),
            None => format!("0x{uuid:04X}"),
        });
    }
    if value.len() % 2 != 0 {
        uuids.push(format!("trailing-byte=0x{:02X}", value[value.len() - 1]));
    }
    uuids.join(", ")
}

fn known_uuid16(uuid: u16) -> Option<&'static str> {
    match uuid {
        0x1844 => Some("Volume Control Service"),
        0x1846 => Some("Coordinated Set Identification Service"),
        0x184E => Some("Audio Stream Control Service"),
        0x184F => Some("Broadcast Audio Scan Service"),
        0x1850 => Some("Published Audio Capabilities Service"),
        0x1851 => Some("Basic Audio Announcement Service"),
        0x1852 => Some("Broadcast Audio Announcement Service"),
        0x1853 => Some("Common Audio Service"),
        0x1854 => Some("Hearing Access Service"),
        0x1855 => Some("Telephony and Media Audio Service"),
        0xFE2C => Some("Google Fast Pair Service"),
        _ => None,
    }
}

fn ad_type_label(kind: u8) -> &'static str {
    match kind {
        0x01 => "Flags",
        0x02 => "Incomplete 16-bit UUIDs",
        0x03 => "Complete 16-bit UUIDs",
        0x06 => "Incomplete 128-bit UUIDs",
        0x07 => "Complete 128-bit UUIDs",
        0x08 => "Shortened Local Name",
        0x09 => "Complete Local Name",
        0x0A => "TX Power",
        0x16 => "Service Data - 16-bit UUID",
        0x19 => "Appearance",
        0x20 => "Service Data - 32-bit UUID",
        0x21 => "Service Data - 128-bit UUID",
        0x24 => "URI",
        0x2E => "Resolvable Set Identifier",
        0xFF => "Manufacturer Specific Data",
        _ => "AD type",
    }
}

fn address_type_label(address: &str, address_type: u8) -> String {
    match address_type {
        0 => "public device address".into(),
        1 => format!(
            "random device address / {}",
            random_address_kind(address, address_type).unwrap_or("unknown-random")
        ),
        2 => "public identity address".into(),
        3 => "random identity address".into(),
        0xFF => "anonymous".into(),
        other => format!("unknown type {other}"),
    }
}

fn random_address_kind(address: &str, address_type: u8) -> Option<&'static str> {
    if address_type != 1 {
        return None;
    }
    let canonical = address.split('/').next().unwrap_or(address);
    let first = canonical.split(':').next()?;
    let octet = u8::from_str_radix(first, 16).ok()?;
    Some(match octet >> 6 {
        0b00 => "non-resolvable-private",
        0b01 => "resolvable-private(RPA)",
        0b11 => "static-random",
        _ => "reserved-random",
    })
}

fn hex_bytes(data: &[u8]) -> String {
    if data.is_empty() {
        return "-".into();
    }
    data.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}


pub fn swift_pair_probe(seconds: u16, wanted_name: &str) -> Result<()> {
    if wanted_name.trim().is_empty() {
        return Err(AppError::Usage("--name requires a nonempty device name".into()));
    }

    println!("R2 Swift Pair ACL-only probe");
    println!("Target name: {wanted_name:?}");
    println!("Policy: detect Microsoft Swift Pair beacon -> LE ACL -> linger -> disconnect; NO SMP/GATT/bonding.");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    println!("Attach: PASS");

    println!("Scanning for Swift Pair beacon ({seconds}s)...");
    let summary = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;

    let mut candidates = summary
        .observations
        .iter()
        .filter(|observation| observation.is_connectable)
        .filter_map(|observation| {
            swift_pair_payload(&observation.raw_data)
                .filter(|payload| {
                    payload.beacon_id == 0x03
                        && payload.sub_scenario == 0x02
                        && payload.display_name.as_deref() == Some(wanted_name)
                })
                .map(|payload| (observation.clone(), payload))
        })
        .collect::<Vec<_>>();

    candidates.sort_by_key(|(observation, _)| std::cmp::Reverse(observation.rssi));

    if candidates.is_empty() {
        return Err(AppError::Protocol(format!(
            "No connectable Microsoft Swift Pair LE+BR/EDR Secure Connections beacon matched {wanted_name:?}"
        )));
    }

    println!("Matching Swift Pair candidate(s):");
    for (index, (observation, payload)) in candidates.iter().enumerate() {
        println!(
            "  [{index}] addr={} type={} RSSI={}dBm beacon=0x{:02X} subscenario=0x{:02X} reserved_rssi=0x{:02X} display_name={:?}",
            observation.address,
            observation.address_type,
            observation.rssi,
            payload.beacon_id,
            payload.sub_scenario,
            payload.reserved_rssi,
            payload.display_name
        );
    }

    let (selected, payload) = &candidates[0];
    println!(
        "Selecting strongest Swift Pair beacon: {} type={} RSSI={}dBm",
        selected.address, selected.address_type, selected.rssi
    );
    println!(
        "Swift Pair decode: beacon=0x{:02X}, subscenario=0x{:02X} (LE + BR/EDR Secure Connections), reserved_rssi=0x{:02X}",
        payload.beacon_id, payload.sub_scenario, payload.reserved_rssi
    );

    let acl = attached
        .connect_scanned_observation(selected, CONNECT_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Swift Pair ACL connect failed: {error}")))?;

    println!(
        "Swift Pair ACL: PASS handle=0x{:04X} peer={}",
        acl.connection_handle, acl.peer_address
    );
    println!("Holding ACL for {}s without SMP/GATT...", ACL_LINGER.as_secs());
    attached
        .linger_le_connection(acl.connection_handle, ACL_LINGER)
        .map_err(|error| AppError::Protocol(format!("Swift Pair ACL linger failed: {error}")))?;
    println!("ACL remained established: PASS");

    let reason = attached
        .disconnect_le_connection(acl.connection_handle, 0x13, DISCONNECT_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Swift Pair disconnect failed: {error}")))?;
    println!("Disconnect: PASS reason=0x{reason:02X}");
    println!("R2 RESULT: Swift Pair transport endpoint is ACL-connectable; no pairing was attempted.");
    Ok(())
}

#[derive(Clone, Debug)]
struct SwiftPairPayload {
    beacon_id: u8,
    sub_scenario: u8,
    reserved_rssi: u8,
    display_name: Option<String>,
}

fn swift_pair_payload(data: &[u8]) -> Option<SwiftPairPayload> {
    let mut offset = 0usize;
    while offset < data.len() {
        let count = usize::from(*data.get(offset)?);
        if count == 0 {
            break;
        }
        let end = offset.checked_add(count + 1)?;
        if count < 1 || end > data.len() {
            break;
        }
        let kind = data[offset + 1];
        let value = &data[offset + 2..end];

        if kind == 0xFF && value.len() >= 5 {
            let company = u16::from_le_bytes([value[0], value[1]]);
            if company == 0x0006 {
                let payload = &value[2..];
                let beacon_id = payload[0];
                let sub_scenario = payload[1];
                let reserved_rssi = payload[2];

                // Swift Pair dual-mode payloads may include three bytes of
                // Class-of-Device before the fallback display name.
                let name_start = if payload.len() >= 6 { 6 } else { 3 };
                let display_name = if payload.len() > name_start {
                    let rendered = String::from_utf8_lossy(&payload[name_start..])
                        .trim_matches(char::from(0))
                        .to_string();
                    (!rendered.is_empty()).then_some(rendered)
                } else {
                    None
                };

                return Some(SwiftPairPayload {
                    beacon_id,
                    sub_scenario,
                    reserved_rssi,
                    display_name,
                });
            }
        }
        offset = end;
    }
    None
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
    println!("Invariant: an existing bond is never deleted or re-paired automatically.");
    println!("Goal: one durable SMP bond per physical member, with CSIS/RSI set validation.");
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
        let set_sirk = store
            .set_sirk(wanted_name)
            .map_err(|error| AppError::Protocol(format!("Stored SIRK invalid: {error}")))?;

        println!(
            "Persistent set state: {}/{} member(s), SIRK={}, set_size={}",
            current.members.len(),
            TARGET_SET_MEMBERS,
            if set_sirk.is_some() { "known" } else { "unknown" },
            current
                .set_size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into())
        );
        for (index, member) in current.members.iter().enumerate() {
            println!(
                "  member[{index}] identity={} type={} rank={} csis_verified={}",
                member.identity_address,
                member.address_type,
                member
                    .rank
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "?".into()),
                member.csis_verified
            );
        }

        if current.members.len() >= TARGET_SET_MEMBERS
            && set_sirk.is_some()
            && current.members.iter().all(|member| member.csis_verified)
        {
            println!(
                "R2.4 onboarding COMPLETE: {} CSIS-verified members are persisted; no pairing was attempted.",
                current.members.len()
            );
            return Ok(());
        }

        println!("Scanning for {seconds}s...");
        let scan = attached
            .scan_passive_extended(seconds)
            .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;
        println!(
            "Scan: {} advertisements, {} unique advertisers",
            scan.reports_seen,
            scan.observations.len()
        );

        let mut known_recovery = Vec::<(bumble_adapter::ScanObservation, String)>::new();
        let mut orphan_bonds = Vec::<(bumble_adapter::ScanObservation, String)>::new();
        let mut fresh = Vec::<bumble_adapter::ScanObservation>::new();

        for observation in scan
            .observations
            .iter()
            .filter(|observation| observation.is_connectable)
        {
            let resolved = store
                .resolve_observation_identity(observation)
                .map_err(|error| AppError::Protocol(format!("RPA resolution failed: {error}")))?;

            match resolved {
                Some(identity_address) if store.is_member_of(wanted_name, &identity_address) => {
                    let needs_csis = current
                        .members
                        .iter()
                        .find(|member| {
                            member
                                .identity_address
                                .eq_ignore_ascii_case(&identity_address)
                        })
                        .is_some_and(|member| !member.csis_verified)
                        || set_sirk.is_none();

                    if needs_csis {
                        known_recovery.push((observation.clone(), identity_address));
                    } else {
                        println!(
                            "  skip known verified member {} via current RPA {}",
                            identity_address, observation.address
                        );
                    }
                }
                Some(identity_address) => {
                    let same_set = candidate_matches_set(observation, set_sirk.as_ref())?;
                    let name_ok = observation.name.as_deref() == Some(wanted_name);
                    if (set_sirk.is_some() && same_set) || (set_sirk.is_none() && name_ok) {
                        orphan_bonds.push((observation.clone(), identity_address));
                    }
                }
                None => {
                    if observation.name.as_deref() != Some(wanted_name) {
                        continue;
                    }
                    if candidate_matches_set(observation, set_sirk.as_ref())? {
                        fresh.push(observation.clone());
                    } else if set_sirk.is_some() {
                        println!(
                            "  reject same-name candidate {}: missing/mismatched CSIS RSI",
                            observation.address
                        );
                    }
                }
            }
        }

        if let Some((observation, identity_address)) = known_recovery
            .into_iter()
            .max_by_key(|(observation, _)| observation.rssi)
        {
            println!(
                "Recovering CSIS metadata for existing bond {} via current RPA {} (no SMP)...",
                identity_address, observation.address
            );
            let acl = attached
                .connect_scanned_observation(&observation, CONNECT_TIMEOUT)
                .map_err(|error| AppError::Protocol(format!(
                    "Known-bond ACL connect failed: {error}"
                )))?;

            let recovery = (|| -> Result<()> {
                attached
                    .resume_bonded_encryption(
                        acl.connection_handle,
                        &identity_address,
                        &store,
                        RESUME_TIMEOUT,
                    )
                    .map_err(|error| AppError::Protocol(format!(
                        "Known-bond encryption failed: {error}"
                    )))?;

                let csis = attached
                    .read_csis_member(
                        acl.connection_handle,
                        &identity_address,
                        &store,
                        RESUME_TIMEOUT,
                    )
                    .map_err(|error| AppError::Protocol(format!(
                        "Known-bond CSIS recovery failed: {error}"
                    )))?;

                validate_csis(&store, wanted_name, &csis)?;
                store
                    .update_csis_member(
                        wanted_name,
                        &identity_address,
                        csis.set_size,
                        csis.sirk,
                        csis.rank,
                    )
                    .map_err(|error| AppError::Protocol(format!(
                        "Failed to persist recovered CSIS metadata: {error}"
                    )))?;

                println!(
                    "  CSIS recovered: set_size={} rank={}",
                    csis.set_size
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "?".into()),
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
            if let Err(error) = recovery {
                let _ = disconnect;
                return Err(error);
            }
            disconnect.map_err(|error| AppError::Protocol(format!(
                "CSIS-recovery disconnect failed: {error}"
            )))?;
            continue;
        }

        if !orphan_bonds.is_empty() {
            println!("Existing untracked bond candidate(s) found; SMP will NOT be repeated:");
            for (index, (observation, identity_address)) in orphan_bonds.iter().enumerate() {
                println!(
                    "  [{index}] identity={} current_rpa={} RSSI={}dBm RSI={}",
                    identity_address,
                    observation.address,
                    observation.rssi,
                    if observation.rsi.is_some() { "present" } else { "absent" }
                );
            }
            let selected_index = if orphan_bonds.len() == 1 {
                0
            } else {
                choose_candidate(orphan_bonds.len())?
            };
            let (observation, identity_address) = &orphan_bonds[selected_index];

            let acl = attached
                .connect_scanned_observation(observation, CONNECT_TIMEOUT)
                .map_err(|error| AppError::Protocol(format!(
                    "Orphan-bond ACL connect failed: {error}"
                )))?;

            let recovery = (|| -> Result<()> {
                attached
                    .resume_bonded_encryption(
                        acl.connection_handle,
                        identity_address,
                        &store,
                        RESUME_TIMEOUT,
                    )
                    .map_err(|error| AppError::Protocol(format!(
                        "Orphan-bond encryption failed: {error}"
                    )))?;

                let csis = attached
                    .read_csis_member(
                        acl.connection_handle,
                        identity_address,
                        &store,
                        RESUME_TIMEOUT,
                    )
                    .map_err(|error| AppError::Protocol(format!(
                        "Orphan-bond CSIS verification failed: {error}"
                    )))?;

                validate_csis(&store, wanted_name, &csis)?;

                if !store
                    .adopt_existing_bond(wanted_name, identity_address)
                    .map_err(|error| AppError::Protocol(format!(
                        "Failed to adopt verified existing bond: {error}"
                    )))?
                {
                    return Err(AppError::Protocol(format!(
                        "bond {} disappeared from the persistent key store during recovery",
                        identity_address
                    )));
                }

                store
                    .update_csis_member(
                        wanted_name,
                        identity_address,
                        csis.set_size,
                        csis.sirk,
                        csis.rank,
                    )
                    .map_err(|error| AppError::Protocol(format!(
                        "Failed to persist adopted CSIS metadata: {error}"
                    )))?;

                println!(
                    "  orphan bond adopted after encrypted CSIS verification: identity={} set_size={} rank={}",
                    identity_address,
                    csis.set_size
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "?".into()),
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
            if let Err(error) = recovery {
                let _ = disconnect;
                return Err(error);
            }
            disconnect.map_err(|error| AppError::Protocol(format!(
                "Orphan-recovery disconnect failed: {error}"
            )))?;
            continue;
        }

        let current = store.set_summary(wanted_name);
        let set_sirk = store
            .set_sirk(wanted_name)
            .map_err(|error| AppError::Protocol(format!("Stored SIRK invalid: {error}")))?;

        if !current.members.is_empty() && set_sirk.is_none() {
            return Err(AppError::Protocol(format!(
                "{} bond(s) are safely persisted, but CSIS/SIRK has not yet been recovered. Do not pair another member. Make a known bonded earbud visible and rerun the same command.",
                current.members.len()
            )));
        }

        if fresh.is_empty() {
            let detail = if set_sirk.is_some() {
                "No unbonded same-set candidate with a valid CSIS RSI was visible."
            } else {
                "No unbonded connectable target was visible."
            };
            return Err(AppError::Protocol(format!(
                "{detail} {} member(s) are already safely persisted; do not delete or re-pair them. Re-enter earbud pairing mode and rerun this same command.",
                current.members.len()
            )));
        }

        println!("Unbonded validated candidate(s):");
        for (index, candidate) in fresh.iter().enumerate() {
            println!(
                "  [{index}] {} type={} RSSI={}dBm PHY={}/{} SID={} RSI={}",
                candidate.address,
                candidate.address_type,
                candidate.rssi,
                candidate.primary_phy,
                candidate.secondary_phy,
                candidate.sid,
                if candidate.rsi.is_some() { "present" } else { "absent" }
            );
        }

        let selected_index = if fresh.len() == 1 {
            println!("Only one candidate; selecting [0].");
            0
        } else {
            choose_candidate(fresh.len())?
        };
        let selected = &fresh[selected_index];

        println!(
            "Connecting candidate [{selected_index}] {}...",
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
        let paired = match attached.pair_and_persist(
            acl.connection_handle,
            &identity.public_address,
            wanted_name,
            &mut store,
            PAIRING_TIMEOUT,
        ) {
            Ok(paired) => paired,
            Err(error) => {
                let _ = attached.disconnect_le_connection(
                    acl.connection_handle,
                    0x13,
                    DISCONNECT_TIMEOUT,
                );
                return Err(AppError::Protocol(format!(
                    "SMP/bond failed: {error}. Existing persisted keys, if any, were NOT deleted; rerun the same onboarding command rather than forgetting the device."
                )));
            }
        };

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

        let csis = match attached.read_csis_member(
            acl.connection_handle,
            &paired.identity_address,
            &store,
            RESUME_TIMEOUT,
        ) {
            Ok(csis) => csis,
            Err(error) => {
                let _ = attached.disconnect_le_connection(
                    acl.connection_handle,
                    0x13,
                    DISCONNECT_TIMEOUT,
                );
                return Err(AppError::Protocol(format!(
                    "Bond {} is safely persisted, but encrypted CSIS discovery failed: {error}. Do NOT re-pair it; rerun onboarding and the stored bond will be resumed.",
                    paired.identity_address
                )));
            }
        };

        if let Err(error) = validate_csis(&store, wanted_name, &csis) {
            let _ = attached.disconnect_le_connection(
                acl.connection_handle,
                0x13,
                DISCONNECT_TIMEOUT,
            );
            return Err(error);
        }

        if let Err(error) = store.update_csis_member(
            wanted_name,
            &paired.identity_address,
            csis.set_size,
            csis.sirk,
            csis.rank,
        ) {
            let _ = attached.disconnect_le_connection(
                acl.connection_handle,
                0x13,
                DISCONNECT_TIMEOUT,
            );
            return Err(AppError::Protocol(format!(
                "Bond {} is persisted, but CSIS metadata commit failed: {error}. Do not re-pair; rerun onboarding to recover.",
                paired.identity_address
            )));
        }

        println!(
            "CSIS verified: SIRK acquired, set_size={} rank={}",
            csis.set_size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into()),
            csis.rank
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into())
        );

        let reason = attached
            .disconnect_le_connection(acl.connection_handle, 0x13, DISCONNECT_TIMEOUT)
            .map_err(|error| AppError::Protocol(format!(
                "Post-bond disconnect failed: {error}"
            )))?;
        println!("Disconnect: PASS reason=0x{reason:02X}");

        let current = store.set_summary(wanted_name);
        if current.members.len() < TARGET_SET_MEMBERS {
            println!(
                "Member persisted + CSIS verified. Continuing directly to member {}/{} without re-pairing existing members.",
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

    let expected_sirk = store
        .set_sirk(wanted_name)
        .map_err(|error| AppError::Protocol(format!("Stored SIRK invalid: {error}")))?
        .ok_or_else(|| AppError::Protocol(
            "Persistent bonds exist but CSIS/SIRK metadata is incomplete; run bond-onboard to recover it without re-pairing".into()
        ))?;

    if set.members.iter().any(|member| !member.csis_verified) {
        return Err(AppError::Protocol(
            "Persistent bond set contains an unverified CSIS member; run bond-onboard to recover metadata without re-pairing".into()
        ));
    }

    println!("Verifying {} persisted member(s)...", set.members.len());

    for (index, member) in set.members.iter().enumerate() {
        println!(
            "Member {}/{} identity={} rank={}",
            index + 1,
            set.members.len(),
            member.identity_address,
            member
                .rank
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into())
        );
        println!("  scanning for current RPA ({seconds}s)...");

        let scan = attached
            .scan_passive_extended(seconds)
            .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;

        let selected = scan
            .observations
            .iter()
            .filter(|observation| observation.is_connectable)
            .filter_map(|observation| {
                store
                    .resolve_observation_identity(observation)
                    .ok()
                    .flatten()
                    .filter(|resolved| {
                        resolved.eq_ignore_ascii_case(&member.identity_address)
                    })
                    .map(|_| observation.clone())
            })
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

        let verification = (|| -> Result<()> {
            attached
                .resume_bonded_encryption(
                    acl.connection_handle,
                    &member.identity_address,
                    &store,
                    RESUME_TIMEOUT,
                )
                .map_err(|error| AppError::Protocol(format!(
                    "Stored-bond encryption failed: {error}"
                )))?;
            println!("  encrypted from stored LTK: PASS");

            let csis = attached
                .read_csis_member(
                    acl.connection_handle,
                    &member.identity_address,
                    &store,
                    RESUME_TIMEOUT,
                )
                .map_err(|error| AppError::Protocol(format!(
                    "Stored-bond CSIS verification failed: {error}"
                )))?;

            if csis.sirk != expected_sirk {
                return Err(AppError::Protocol(format!(
                    "CSIS SIRK changed for persistent member {}",
                    member.identity_address
                )));
            }
            if let (Some(stored), Some(actual)) = (set.set_size, csis.set_size) {
                if stored != actual {
                    return Err(AppError::Protocol(format!(
                        "CSIS Set Size changed for {}: stored {}, peer {}",
                        member.identity_address, stored, actual
                    )));
                }
            }
            if let (Some(stored), Some(actual)) = (member.rank, csis.rank) {
                if stored != actual {
                    return Err(AppError::Protocol(format!(
                        "CSIS rank changed for {}: stored {}, peer {}",
                        member.identity_address, stored, actual
                    )));
                }
            }
            println!(
                "  CSIS: PASS set_size={} rank={}",
                csis.set_size
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "?".into()),
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
            "Stored-bond disconnect failed: {error}"
        )))?;
        println!("  disconnect: PASS reason=0x{reason:02X}");
    }

    println!("R2.4 RESULT: all recorded members reconnected, encrypted from stored LTK, and matched stored CSIS without SMP pairing.");
    Ok(())
}

fn candidate_matches_set(
    observation: &bumble_adapter::ScanObservation,
    sirk: Option<&[u8; 16]>,
) -> Result<bool> {
    let Some(sirk) = sirk else {
        return Ok(true);
    };
    let Some(rsi) = observation.rsi.as_ref() else {
        return Ok(false);
    };
    bumble_adapter::rsi_matches_sirk(sirk, rsi)
        .map_err(|error| AppError::Protocol(format!("CSIS RSI validation failed: {error}")))
}

fn validate_csis(
    store: &bumble_adapter::BondStore,
    wanted_name: &str,
    report: &bumble_adapter::CsisMemberReport,
) -> Result<()> {
    if let Some(expected) = store
        .set_sirk(wanted_name)
        .map_err(|error| AppError::Protocol(format!("Stored SIRK invalid: {error}")))?
    {
        if report.sirk != expected {
            return Err(AppError::Protocol(
                "CSIS SIRK mismatch: peer does not belong to the already-persisted coordinated set".into()
            ));
        }
    }

    let current = store.set_summary(wanted_name);
    if let (Some(stored), Some(actual)) = (current.set_size, report.set_size) {
        if stored != actual {
            return Err(AppError::Protocol(format!(
                "CSIS Set Size mismatch: stored {stored}, peer {actual}"
            )));
        }
    }
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