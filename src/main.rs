mod architecture;
mod runtime;
mod platform;
mod transport;

mod hci_host;
mod review2;

fn main() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("review2")
            && args.get(1).map(String::as_str) == Some("hci-smoke")
        {
            let config = match review2::parse_hci_smoke_args(&args[2..]) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("Review-2A hci-smoke argument error: {error}");
                    std::process::exit(2);
                }
            };

            println!("Review-2A firmware bypass: probe -> optional one-shot load -> release");
            println!(
                "Review-2A HCI smoke: resetless attach -> capabilities -> passive scan"
            );
            println!("Scan duration: {} s", config.smoke.scan_duration.as_secs());
            if let Some(name) = &config.smoke.target_name {
                println!("Target name: {name}");
            }

            match review2::execute_hci_smoke(config) {
                Ok(result) => {
                    println!("Firmware bypass: {:?}", result.firmware);
                    println!("Capabilities: PASS");
                    println!(
                        "Scan: PASS reports={} observations={} truncated={}",
                        result.hci.scan.reports_seen,
                        result.hci.scan.observations.len(),
                        result.hci.scan.truncated
                    );
                    for observation in &result.hci.scan.observations {
                        println!(
                            "  addr={} type={} rssi={} name={}",
                            observation.address,
                            observation.address_type,
                            observation.rssi,
                            observation.name.as_deref().unwrap_or("<none>")
                        );
                    }
                    println!("Target seen: {}", result.hci.target_seen);
                    println!(
                        "Shutdown: hci={:?} transport={:?}",
                        result.hci.exit.hci, result.hci.exit.transport
                    );

                    if !result.hci.target_seen {
                        std::process::exit(3);
                    }
                    return;
                }
                Err(error) => {
                    eprintln!("Review-2A HCI smoke FAILED: {error:?}");
                    std::process::exit(1);
                }
            }
        }
    }

    println!("BE200AudioHub Review/Refactor architecture scaffold");
}
