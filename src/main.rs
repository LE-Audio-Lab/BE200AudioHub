mod architecture;
mod runtime;
mod platform;
mod transport;

mod hci_host;
mod review2;
mod review2_bond;
mod review2_bond_smoke;

fn main() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("review2")
            && args.get(1).map(String::as_str) == Some("bond-smoke")
        {
            let config = match review2_bond_smoke::parse_bond_smoke_args(
                &args[2..],
            ) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!(
                        "Review-2B bond-smoke argument error: {error}"
                    );
                    std::process::exit(2);
                }
            };

            println!(
                "Review-2B stored-bond HIL: identity guard -> privacy -> dual ACL -> stored-LTK -> cleanup"
            );
            println!("Target set: {}", config.target_name);
            println!(
                "Connect timeout: {} s",
                config.connect_timeout.as_secs()
            );
            println!("Policy: existing bond only; no pairing/SMP start; no bond writes");

            match review2_bond_smoke::execute_bond_smoke(config) {
                Ok(result) => {
                    println!("Firmware bypass: {:?}", result.firmware);
                    println!(
                        "Controller identity guard: PASS ({})",
                        result.controller_identity
                    );
                    for (index, member) in result.members.iter().enumerate() {
                        println!(
                            "Member {}: identity={} handle=0x{:04X}@gen{} encrypted=true key_size={}",
                            index + 1,
                            member.identity_address,
                            member.connection.connection_handle,
                            member.connection.generation,
                            member.encryption_key_size,
                        );
                    }
                    println!("Dual encrypted ACL: PASS");
                    for (connection, reason) in &result.cleanup.disconnected {
                        println!(
                            "Cleanup disconnect: handle=0x{:04X}@gen{} reason=0x{:02X}",
                            connection.connection_handle,
                            connection.generation,
                            reason,
                        );
                    }
                    println!(
                        "Privacy cleanup: {}",
                        if result.cleanup.privacy_cleared {
                            "PASS"
                        } else {
                            "NOT REQUIRED"
                        }
                    );
                    println!(
                        "Bond store unchanged: {}",
                        result.bond_unchanged
                    );
                    println!(
                        "Shutdown: hci={:?} transport={:?}",
                        result.exit.hci, result.exit.transport
                    );
                    return;
                }
                Err(review2_bond_smoke::BondSmokeExecuteError::Preflight(
                    error,
                )) => {
                    eprintln!(
                        "Review-2B bond-smoke PRECHECK FAILED: {error}"
                    );
                    std::process::exit(1);
                }
                Err(review2_bond_smoke::BondSmokeExecuteError::Runtime {
                    primary,
                    cleanup,
                    bond_unchanged,
                    exit,
                }) => {
                    eprintln!(
                        "Review-2B bond-smoke FAILED: {primary:?}"
                    );
                    for (connection, reason) in cleanup.disconnected {
                        eprintln!(
                            "Cleanup disconnect: handle=0x{:04X}@gen{} reason=0x{:02X}",
                            connection.connection_handle,
                            connection.generation,
                            reason,
                        );
                    }
                    eprintln!(
                        "Privacy cleanup: {}",
                        cleanup.privacy_cleared
                    );
                    for error in cleanup.errors {
                        eprintln!("Cleanup issue: {error}");
                    }
                    eprintln!(
                        "Bond store unchanged/proven: {bond_unchanged}"
                    );
                    eprintln!(
                        "Shutdown: hci={:?} transport={:?}",
                        exit.hci, exit.transport
                    );
                    std::process::exit(1);
                }
            }
        }

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
