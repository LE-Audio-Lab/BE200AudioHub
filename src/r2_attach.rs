use crate::{
    driver,
    error::{AppError, Result},
};
use std::time::Duration;

const ATTACH_TIMEOUT: Duration = Duration::from_secs(5);

pub fn probe() -> Result<()> {
    driver::verify()?;

    println!("R2.1 attach probe");
    println!("Target: Intel BE200 8087:0036 via Bumble-RS USB transport");
    println!("Policy: preserve existing Operational firmware; HCI Reset is forbidden");
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
    driver::verify()?;

    println!("R2.2 Bumble passive BLE scan");
    println!("Target: Intel BE200 8087:0036; LE 1M; {seconds}s");
    println!("Policy: no HCI Reset, no firmware reload, no connection, no pairing, no SCAN_REQ");
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
            format!(" addr={}", observation.address)
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
