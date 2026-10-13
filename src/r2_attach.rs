use crate::{
    error::{AppError, Result},
    usb,
};
use std::time::Duration;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);

pub fn probe() -> Result<()> {
    println!("R2 attach probe");
    println!("Policy: preserve Operational firmware; HCI Reset is forbidden");
    usb::r2_fast_preflight()?;

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
    Ok(())
}

pub fn parse_scan_seconds(value: &str) -> Result<u16> {
    let seconds: u16 = value.parse().map_err(|_| {
        AppError::Protocol("R2 scan duration must be an integer (1..=30 seconds)".into())
    })?;
    if !(1..=30).contains(&seconds) {
        return Err(AppError::Protocol(
            "R2 scan duration must be 1..=30 seconds".into(),
        ));
    }
    Ok(seconds)
}

pub fn scan(seconds: u16, show_addresses: bool) -> Result<()> {
    println!("R2 passive BLE scan");
    println!("Policy: no reset, connection, pairing, or SCAN_REQ");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    let summary = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Bumble scan failed: {error}")))?;

    println!(
        "Scan: PASS - {} advertisement(s), {} retained advertiser(s)",
        summary.reports_seen,
        summary.observations.len()
    );
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
    Ok(())
}

pub fn scan_forensics(seconds: u16) -> Result<()> {
    println!("R2 passive advertising forensics");
    println!("Policy: raw HCI trace only; no connection, pairing, GATT, or ISO");
    usb::r2_fast_preflight()?;

    let mut attached = bumble_adapter::attach_be200_existing_operational_raw_trace(ATTACH_TIMEOUT)
        .map_err(|error| AppError::Protocol(format!("Bumble attach failed: {error}")))?;
    let summary = attached
        .scan_passive_extended(seconds)
        .map_err(|error| AppError::Protocol(format!("Bumble forensic scan failed: {error}")))?;

    println!(
        "Forensic scan: PASS - {} report(s), {} retained advertiser(s)",
        summary.reports_seen,
        summary.observations.len()
    );
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
            println!(
                "  derived 16-bit UUIDs: {}",
                uuids
                    .iter()
                    .map(|uuid| match known_uuid16(*uuid) {
                        Some(label) => format!("0x{uuid:04X}({label})"),
                        None => format!("0x{uuid:04X}"),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
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
                "    [{index:02}] malformed/truncated: offset={} declared={} remaining={}",
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
                format!(" => {decoded}")
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
    let mut uuids = value
        .chunks_exact(2)
        .map(|chunk| {
            let uuid = u16::from_le_bytes([chunk[0], chunk[1]]);
            match known_uuid16(uuid) {
                Some(label) => format!("0x{uuid:04X}({label})"),
                None => format!("0x{uuid:04X}"),
            }
        })
        .collect::<Vec<_>>();
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
