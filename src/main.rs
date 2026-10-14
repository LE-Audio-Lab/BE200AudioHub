mod architecture;
mod runtime;
mod platform;
mod transport;

#[cfg(any(feature = "hci-wip", test))]
mod hci_host;

fn main() {
    #[cfg(all(feature = "hci-wip", windows))]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("review2")
            && args.get(1).map(String::as_str) == Some("hci-smoke")
        {
            let config = match hci_host::smoke::parse_smoke_args(&args[2..]) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("Review-2A hci-smoke argument error: {error}");
                    std::process::exit(2);
                }
            };

            println!(
                "Review-2A HCI smoke: resetless attach -> capabilities -> passive scan"
            );
            println!("Scan duration: {} s", config.scan_duration.as_secs());
            if let Some(name) = &config.target_name {
                println!("Target name: {name}");
            }

            match hci_host::smoke::run_be200_smoke(config) {
                Ok(result) => {
                    println!("Capabilities: PASS");
                    println!(
                        "Scan: PASS reports={} observations={} truncated={}",
                        result.scan.reports_seen,
                        result.scan.observations.len(),
                        result.scan.truncated
                    );
                    for observation in &result.scan.observations {
                        println!(
                            "  addr={} type={} rssi={} name={}",
                            observation.address,
                            observation.address_type,
                            observation.rssi,
                            observation.name.as_deref().unwrap_or("<none>")
                        );
                    }
                    println!("Target seen: {}", result.target_seen);
                    println!(
                        "Shutdown: hci={:?} transport={:?}",
                        result.exit.hci, result.exit.transport
                    );

                    if !result.target_seen {
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
