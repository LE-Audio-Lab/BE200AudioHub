//! R1.6b: bounded BLE passive scanning with LE Extended Advertising Reports.
//! Never initiates a connection, transmits an advertisement, pairs, or resets.
//! HCI controller-native duration is an independent auto-stop safeguard.
use crate::{error::{AppError, Result}, hci_capabilities, hci_wire::HciSession};
use std::{collections::HashMap, time::{Duration, Instant}};

const HCI_SET_EVENT_MASK: u16 = 0x0c01;
const LE_SET_EVENT_MASK: u16 = 0x2001;
const LE_SET_EXT_SCAN_PARAMS: u16 = 0x2041;
const LE_SET_EXT_SCAN_ENABLE: u16 = 0x2042;
const OUTPUT_LIMIT: usize = 100;

fn protocol(message: impl Into<String>) -> AppError { AppError::Protocol(message.into()) }

#[derive(Debug, PartialEq, Eq)]
struct Advertisement {
    addr_type: u8,
    address: [u8; 6],
    event_type: u16,
    rssi: i8,
    primary_phy: u8,
    sid: u8,
    data: Vec<u8>,
}

/// One command-line duration in 1..=30 seconds.
pub fn parse_seconds(value: &str) -> Result<u16> {
    let seconds: u16 = value.parse()
        .map_err(|_| protocol("Scan duration must be an integer (1..=30 seconds)"))?;
    if !(1..=30).contains(&seconds) {
        return Err(protocol("Scan duration must be 1..=30 seconds"));
    }
    Ok(seconds)
}

fn parse_reports(event: &[u8]) -> Result<Vec<Advertisement>> {
    if event.len() < 2 || event.len() != event[1] as usize + 2 {
        return Err(protocol("Invalid HCI advertising-event framing"));
    }
    if event[0] != 0x3e { return Ok(Vec::new()); }
    if event.len() < 4 { return Err(protocol("Truncated LE Meta Event")); }
    let subevent = event[2];
    if subevent != 0x0d && subevent != 0x02 { return Ok(Vec::new()); }
    let count = event[3] as usize;
    let mut offset = 4;
    let mut reports = Vec::with_capacity(count.min(10));
    for _ in 0..count {
        let available = event.len().saturating_sub(offset);
        match subevent {
            0x0d => {
                // LE Extended Advertising Report: 24-byte fixed header, then data.
                if available < 24 { return Err(protocol("Truncated Extended Advertising Report")); }
                let event_type = u16::from_le_bytes([event[offset],event[offset+1]]);
                let addr_type = event[offset+2];
                let mut address = [0u8;6];
                address.copy_from_slice(&event[offset+3..offset+9]);
                let primary_phy = event[offset+9];
                let sid = event[offset+11];
                let rssi = event[offset+13] as i8;
                let length = event[offset+23] as usize;
                if available < 24 + length { return Err(protocol("Extended Advertising data overrun")); }
                reports.push(Advertisement {
                    addr_type,address,event_type,rssi,primary_phy,sid,
                    data:event[offset+24..offset+24+length].to_vec(),
                });
                offset += 24 + length;
            }
            0x02 => {
                // Legacy LE Advertising Report: event(1), addr type(1),
                // address(6), data length(1), data(N), RSSI(1).
                if available < 10 { return Err(protocol("Truncated Legacy Advertising Report")); }
                let length = event[offset+8] as usize;
                if available < 10 + length { return Err(protocol("Legacy Advertising data overrun")); }
                let mut address = [0u8;6];
                address.copy_from_slice(&event[offset+2..offset+8]);
                reports.push(Advertisement {
                    event_type: event[offset] as u16, addr_type:event[offset+1],
                    address,rssi:event[offset+9+length] as i8,primary_phy:1,sid:0xff,
                    data:event[offset+9..offset+9+length].to_vec(),
                });
                offset += 10 + length;
            }
            _ => unreachable!(),
        }
    }
    if offset != event.len() { return Err(protocol("Trailing bytes after Advertising Reports")); }
    Ok(reports)
}

/// AD-structure name field; never emit embedded terminal control characters.
fn display_name(data: &[u8]) -> Option<String> {
    let mut offset = 0;
    let mut short = None;
    while offset < data.len() {
        let count = data[offset] as usize;
        if count == 0 { break; }
        if offset + count + 1 > data.len() { break; }
        let kind = data[offset + 1];
        if (kind == 0x08 || kind == 0x09) && count > 1 {
            let safe: String = String::from_utf8_lossy(&data[offset+2..offset+count+1])
                .chars().filter(|c| !c.is_control()).take(48).collect();
            if !safe.is_empty() {
                if kind == 0x09 { return Some(safe); }
                short = Some(safe);
            }
        }
        offset += count + 1;
    }
    short
}

fn address_text(address: &[u8;6]) -> String {
    address.iter().rev().map(|octet| format!("{octet:02X}"))
        .collect::<Vec<_>>().join(":")
}

fn scan_params_1m_passive() -> [u8; 8] {
    // Own Address Public, accept all, PHY=LE 1M, passive (no SCAN_REQ),
    // interval 0x0060 = 60ms, window 0x0030 = 30ms.
    [0, 0, 1, 0, 0x60, 0, 0x30, 0]
}

fn native_enable(seconds: u16) -> [u8; 6] {
    // HCI duration unit is 10ms; add 500ms so host explicitly disables
    // first under normal completion. Native timeout handles abrupt exit.
    let duration_units = seconds * 100 + 50;
    let mut out = [1, 1, 0, 0, 0, 0]; // enable, filter duplicates, duration, period=0
    out[2..4].copy_from_slice(&duration_units.to_le_bytes());
    out
}

pub fn scan(seconds: u16, show_addresses: bool) -> Result<()> {
    if !(1..=30).contains(&seconds) { return Err(protocol("Scan duration must be 1..=30 seconds")); }
    let mut hci = HciSession::open()?;
    let mut possibly_enabled = false;
    let outcome = (|| -> Result<()> {
        hci_capabilities::ensure_operational(&mut hci)?;
        let supported = hci_capabilities::read_supported_commands(&mut hci)?;
        if !hci_capabilities::extended_scan_supported(&supported)? {
            return Err(protocol("Controller did not advertise Extended Scan commands; no scan started"));
        }

        // HCI Event Mask bit61=LE Meta Event; also include Command Complete,
        // Command Status and Hardware Error event bits. LE Event Mask bit12
        // enables Extended Advertising Reports, bit16 scan timeout, bit1 legacy.
        let event_mask = ((1u64 << 61) | (1u64 << 15) | (1u64 << 5) | (1u64 << 4)).to_le_bytes();
        let le_mask = ((1u64 << 12) | (1u64 << 16) | (1u64 << 1)).to_le_bytes();
        hci.command_complete(HCI_SET_EVENT_MASK, &event_mask)?;
        hci.command_complete(LE_SET_EVENT_MASK, &le_mask)?;
        hci.command_complete(LE_SET_EXT_SCAN_PARAMS, &scan_params_1m_passive())?;

        println!("R1.6b — Passive BLE scan (LE 1M; {seconds}s)");
        println!("No connection, pairing, scan requests, advertising or audio traffic.");
        if show_addresses {
            println!("Privacy warning: --show-addresses prints nearby Bluetooth addresses.");
        } else {
            println!("Addresses are hidden by default; #IDs are local to this scan.");
        }

        // Mark before sending Enable: even if its ack is lost, do best-effort
        // disable. Controller native Duration is a second protection.
        possibly_enabled = true;
        hci.send_command(LE_SET_EXT_SCAN_ENABLE, &native_enable(seconds))?;
        hci.wait_complete(LE_SET_EXT_SCAN_ENABLE)?;

        let until = Instant::now() + Duration::from_secs(seconds as u64);
        let mut total = 0usize;
        let mut malformed = 0usize;
        let mut seen: HashMap<(u8,[u8;6]),usize> = HashMap::new();
        let mut timed_out_natively = false;
        while Instant::now() < until {
            let timeout = until.saturating_duration_since(Instant::now()).min(Duration::from_millis(300));
            let Some(event) = hci.poll_event(timeout)? else { continue; };
            if event.len() >= 3 && event[0] == 0x3e && event[2] == 0x11 {
                timed_out_natively = true;
                break; // HCI LE Extended Scan Timeout, scan is already disabled
            }
            let reports = match parse_reports(&event) {
                Ok(reports) => reports,
                Err(error) => {
                    malformed += 1;
                    log::warn!(target:"be200hub::ble", "Ignoring malformed LE report: {error}");
                    continue;
                }
            };
            for report in reports {
                total += 1;
                let next = seen.len() + 1;
                let id = *seen.entry((report.addr_type,report.address)).or_insert(next);
                if total <= OUTPUT_LIMIT {
                    let name = display_name(&report.data).unwrap_or_else(|| "(unnamed)".into());
                    let sid = if report.sid == 0xff { "-".into() } else { report.sid.to_string() };
                    let address = if show_addresses {
                        format!(" addr={}",address_text(&report.address))
                    } else { String::new() };
                    println!("  #{id:03} RSSI={}dBm PHY={} SID={sid} type=0x{:04X} data={}B name={}{}",
                        report.rssi,report.primary_phy,report.event_type,report.data.len(),name,address);
                } else if total == OUTPUT_LIMIT + 1 {
                    println!("  ... report output limited to {OUTPUT_LIMIT}; continuing scan and counting");
                }
            }
        }
        println!("Scan summary: {total} reports, {} unique advertisers, {malformed} malformed events",
            seen.len());
        if timed_out_natively {
            println!("Controller reported LE Extended Scan Timeout.");
        }
        log::info!(target:"be200hub::ble", "scan completed: {} reports, {} unique, {} malformed",
            total, seen.len(), malformed);
        Ok(())
    })();

    // Always attempt to disable if Enable may have reached the controller.
    let cleanup = if possibly_enabled {
        hci.command_complete(LE_SET_EXT_SCAN_ENABLE, &[0,0,0,0,0,0]).map(|_| ())
    } else { Ok(()) };
    let result = match (outcome, cleanup) {
        (Ok(()), Ok(())) => {
            if possibly_enabled { println!("LE Extended Scan disabled: OK"); }
            Ok(())
        }
        (Ok(()), Err(error)) => Err(protocol(format!(
            "Scan finished, but failed to confirm scan disable: {error}"
        ))),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(disable)) => {
            eprintln!("WARNING: scan disable could not be confirmed: {disable}");
            Err(error)
        }
    };
    hci.finish(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_and_scan_parameter_guards() {
        assert_eq!(parse_seconds("10").unwrap(),10);
        assert!(parse_seconds("0").is_err());
        assert!(parse_seconds("31").is_err());
        assert!(parse_seconds("abc").is_err());
        assert_eq!(scan_params_1m_passive(), [0,0,1,0,0x60,0,0x30,0]);
        assert_eq!(u16::from_le_bytes([native_enable(10)[2],native_enable(10)[3]]),1050);
    }

    #[test]
    fn parses_ext_adv_and_sanitizes_name() {
        let mut event = vec![0x3e,0,0x0d,1];
        event.extend_from_slice(&[
            0x13,0, // legacy advertising type in extended report
            1, // address type
            1,2,3,4,5,6, // address
            1,0,0xff,0x7f,0xc5, // PHY 1M, secondary none, SID invalid, tx, RSSI=-59
            0,0, // periodic interval
            0xff,0,0,0,0,0,0, // direct address type + address
            3, // advertising data length
            2,9,b'X', // AD name X
        ]);
        event[1] = (event.len()-2) as u8;
        let reports = parse_reports(&event).unwrap();
        assert_eq!(reports.len(),1);
        assert_eq!(reports[0].rssi,-59);
        assert_eq!(reports[0].data,vec![2,9,b'X']);
        assert_eq!(display_name(&reports[0].data).as_deref(),Some("X"));
        assert_eq!(address_text(&reports[0].address),"06:05:04:03:02:01");
    }

    #[test]
    fn parses_legacy_and_rejects_overrun() {
        let event = [0x3e,13,0x02,1,0,1,1,2,3,4,5,6,1,0,0xd8];
        let reports = parse_reports(&event).unwrap();
        assert_eq!(reports[0].rssi,-40);
        let mut malformed = event;
        malformed[12] = 20;
        assert!(parse_reports(&malformed).is_err());
        assert!(parse_reports(&[0x3e,2,0x0d,1]).is_err());
    }

    #[test]
    fn ad_name_strips_terminal_control_chars() {
        assert_eq!(display_name(&[3,9,0x1b,b'A']),Some("A".into()));
        assert_eq!(display_name(&[5,9,b'X']),None);
    }
}
