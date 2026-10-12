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
