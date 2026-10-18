mod architecture;
mod runtime;
mod platform;
mod plane;
mod iso_tx_bridge;
mod control;

mod review;

use review::{
    review2::{
        attach as review2,
        bond_smoke as review2_bond_smoke,
        gatt_smoke as review2_gatt_smoke,
        iso_smoke as review2_iso_smoke,
    },
    round7::first_sound as round7_first_sound,
};

fn main() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("round7")
            && args.get(1).map(String::as_str) == Some("first-sound")
        {
            let config =
                match round7_first_sound::parse_first_sound_args(
                    &args[2..],
                ) {
                    Ok(config) => config,
                    Err(error) => {
                        eprintln!(
                            "Round7 First Sound argument error: {error}"
                        );
                        std::process::exit(2);
                    }
                };

            println!(
                "Round7 First Sound HIL: sealed Review-2D setup -> route-acknowledged logical stream -> valid LC3 -> absolute StreamClock grid -> IsoTxBridge -> logical HCI ISO lane -> drain -> reverse teardown"
            );
            println!("Target set: {}", config.bond.target_name);
            println!(
                "Frames: {} × 10 ms; LC3 48 kHz mono/ear, 100 B/ear",
                config.frames
            );
            println!(
                "Test signal: Left=400 Hz, Right=600 Hz, approx -50 dBFS digital peak, 100 ms fades."
            );
            println!(
                "SAFETY: dBFS is not calibrated acoustic SPL. First run with both earbuds OUT of your ears."
            );
            println!(
                "Scope: validates the new Media/Clock/Bridge/logical-HCI data plane; setup/teardown still reuse the sealed Review-2D HIL oracle, so this is not yet full Core/Lifecycle runtime acceptance."
            );

            match round7_first_sound::execute_first_sound(config) {
                Ok(result) => {
                    let submitted = result
                        .audit_after
                        .total_submitted
                        .saturating_sub(
                            result.audit_before.total_submitted
                        );
                    let completed = result
                        .audit_after
                        .total_completed
                        .saturating_sub(
                            result.audit_before.total_completed
                        );

                    println!(
                        "Firmware bypass: {:?}",
                        result.bond.firmware
                    );
                    println!(
                        "Controller identity guard: PASS ({})",
                        result.bond.controller_identity
                    );
                    println!(
                        "Product data plane: valid LC3 -> StreamClock -> IsoTxBridge -> logical HCI route: PASS"
                    );
                    println!(
                        "ISO Host audit: frames={} submitted={} completed={} in_flight_after={}",
                        result.frames,
                        submitted,
                        completed,
                        result.audit_after.in_flight
                    );
                    println!(
                        "Controller ISO packet window: {:?}",
                        result.audit_before.limit
                    );
                    println!(
                        "Logical route unbind + ISO drain + reverse teardown: PASS"
                    );
                    for (connection, reason) in
                        &result.bond.cleanup.disconnected
                    {
                        println!(
                            "Cleanup ACL disconnect: handle=0x{:04X}@gen{} reason=0x{:02X}",
                            connection.connection_handle,
                            connection.generation,
                            reason,
                        );
                    }
                    println!(
                        "Privacy cleanup: {}",
                        result.bond.cleanup.privacy_cleared
                    );
                    println!(
                        "Bond store unchanged: {}",
                        result.bond.bond_unchanged
                    );
                    println!(
                        "Shutdown: hci={:?} transport={:?}",
                        result.bond.exit.hci,
                        result.bond.exit.transport
                    );
                    return;
                }
                Err(review2_iso_smoke::IsoSmokeExecuteError::Bond(
                    error,
                )) => {
                    eprintln!(
                        "Round7 First Sound session FAILED: {error:?}"
                    );
                    std::process::exit(1);
                }
                Err(review2_iso_smoke::IsoSmokeExecuteError::Probe {
                    detail,
                    bond,
                }) => {
                    eprintln!(
                        "Round7 First Sound PROBE FAILED: {detail}"
                    );
                    for error in &bond.cleanup.errors {
                        eprintln!("Cleanup issue: {error}");
                    }
                    eprintln!(
                        "Privacy cleanup: {}; Bond store unchanged: {}",
                        bond.cleanup.privacy_cleared,
                        bond.bond_unchanged
                    );
                    eprintln!(
                        "Shutdown: hci={:?} transport={:?}",
                        bond.exit.hci, bond.exit.transport
                    );
                    std::process::exit(1);
                }
                Err(
                    review2_iso_smoke::IsoSmokeExecuteError::ProbeAndCleanup {
                        detail,
                        bond,
                    },
                ) => {
                    eprintln!(
                        "Round7 First Sound PROBE FAILED: {detail}"
                    );
                    eprintln!(
                        "Round7 First Sound cleanup also FAILED: {bond:?}"
                    );
                    std::process::exit(1);
                }
            }
        }

        if args.first().map(String::as_str) == Some("review2")
            && args.get(1).map(String::as_str) == Some("iso-smoke")
        {
            let config = match review2_iso_smoke::parse_iso_smoke_args(
                &args[2..],
            ) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!(
                        "Review-2D iso-smoke argument error: {error}"
                    );
                    std::process::exit(2);
                }
            };

            println!(
                "Review-2D ISO HIL: existing bond -> pre-ACL CIS Host bit32 -> dual encrypted ACL -> ASCS Codec/QoS/Enable -> dual CIS -> ISO paths -> bounded HCI ISO TX -> drain -> teardown"
            );
            println!("Target set: {}", config.bond.target_name);
            println!(
                "Connect timeout: {} s; transport intervals: {} × 10 ms",
                config.bond.connect_timeout.as_secs(),
                config.frames
            );
            println!(
                "Policy: existing bond only; no SMP/bond writes. ISO payload is codec-opaque 100-byte transport data; this smoke does NOT claim LC3/acoustic correctness."
            );

            match review2_iso_smoke::execute_iso_smoke(config) {
                Ok(result) => {
                    let submitted = result
                        .audit_after
                        .total_submitted
                        .saturating_sub(
                            result.audit_before.total_submitted
                        );
                    let completed = result
                        .audit_after
                        .total_completed
                        .saturating_sub(
                            result.audit_before.total_completed
                        );
                    println!(
                        "Firmware bypass: {:?}",
                        result.bond.firmware
                    );
                    println!(
                        "Controller identity guard: PASS ({})",
                        result.bond.controller_identity
                    );
                    println!(
                        "Left:  identity={} CIS=0x{:04X}",
                        result.left_identity_address,
                        result.left_cis_handle
                    );
                    println!(
                        "Right: identity={} CIS=0x{:04X}",
                        result.right_identity_address,
                        result.right_cis_handle
                    );
                    println!(
                        "Controller ISO packet window: {:?}",
                        result.audit_before.limit
                    );
                    println!(
                        "ISO Host audit: frames={} submitted={} completed={} in_flight_after={}",
                        result.frames,
                        submitted,
                        completed,
                        result.audit_after.in_flight
                    );
                    println!("ISO drain + reverse teardown: PASS");
                    for (connection, reason) in
                        &result.bond.cleanup.disconnected
                    {
                        println!(
                            "Cleanup ACL disconnect: handle=0x{:04X}@gen{} reason=0x{:02X}",
                            connection.connection_handle,
                            connection.generation,
                            reason,
                        );
                    }
                    println!(
                        "Privacy cleanup: {}",
                        result.bond.cleanup.privacy_cleared
                    );
                    println!(
                        "Bond store unchanged: {}",
                        result.bond.bond_unchanged
                    );
                    println!(
                        "Shutdown: hci={:?} transport={:?}",
                        result.bond.exit.hci,
                        result.bond.exit.transport
                    );
                    return;
                }
                Err(review2_iso_smoke::IsoSmokeExecuteError::Bond(
                    error,
                )) => {
                    eprintln!(
                        "Review-2D iso-smoke session FAILED: {error:?}"
                    );
                    std::process::exit(1);
                }
                Err(review2_iso_smoke::IsoSmokeExecuteError::Probe {
                    detail,
                    bond,
                }) => {
                    eprintln!(
                        "Review-2D iso-smoke PROBE FAILED: {detail}"
                    );
                    for error in &bond.cleanup.errors {
                        eprintln!("Cleanup issue: {error}");
                    }
                    eprintln!(
                        "Privacy cleanup: {}; Bond store unchanged: {}",
                        bond.cleanup.privacy_cleared,
                        bond.bond_unchanged
                    );
                    eprintln!(
                        "Shutdown: hci={:?} transport={:?}",
                        bond.exit.hci, bond.exit.transport
                    );
                    std::process::exit(1);
                }
                Err(
                    review2_iso_smoke::IsoSmokeExecuteError::ProbeAndCleanup {
                        detail,
                        bond,
                    },
                ) => {
                    eprintln!(
                        "Review-2D iso-smoke PROBE FAILED: {detail}"
                    );
                    eprintln!(
                        "Review-2D session cleanup also FAILED: {bond:?}"
                    );
                    std::process::exit(1);
                }
            }
        }

        if args.first().map(String::as_str) == Some("review2")
            && args.get(1).map(String::as_str) == Some("gatt-smoke")
        {
            let config = match review2_gatt_smoke::parse_gatt_smoke_args(
                &args[2..],
            ) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!(
                        "Review-2C gatt-smoke argument error: {error}"
                    );
                    std::process::exit(2);
                }
            };

            println!(
                "Review-2C read-only GATT HIL: existing bond -> dual encrypted ACL -> MTU -> services -> PACS read -> cleanup"
            );
            println!("Target set: {}", config.bond.target_name);
            println!(
                "Connect timeout: {} s; GATT timeout: {} s",
                config.bond.connect_timeout.as_secs(),
                config.gatt_timeout.as_secs()
            );
            println!(
                "Policy: read-only GATT; no SMP, no bond writes, no ASCS/CCCD writes"
            );

            match review2_gatt_smoke::execute_gatt_smoke(config) {
                Ok(result) => {
                    println!(
                        "Firmware bypass: {:?}",
                        result.bond.firmware
                    );
                    println!(
                        "Controller identity guard: PASS ({})",
                        result.bond.controller_identity
                    );
                    for member in &result.members {
                        println!(
                            "Member: identity={} handle=0x{:04X}@gen{} mtu={} services={} PACS=0x{:04X}..0x{:04X} ASCS=0x{:04X}..0x{:04X} CAS=0x{:04X}..0x{:04X}",
                            member.identity_address,
                            member.connection.connection_handle,
                            member.connection.generation,
                            member.mtu,
                            member.service_count,
                            member.pacs.start_handle,
                            member.pacs.end_handle,
                            member.ascs.start_handle,
                            member.ascs.end_handle,
                            member.cas.start_handle,
                            member.cas.end_handle,
                        );
                        println!(
                            "  GATT: PACS chars={} ASCS chars={} CAS includes={} CSIS={} descriptors={} supported-contexts-handle=0x{:04X} value-bytes={}",
                            member.pacs_characteristic_count,
                            member.ascs_characteristic_count,
                            member.cas_included_service_count,
                            member.cas_includes_csis,
                            member.descriptor_probe_count,
                            member.supported_contexts_handle,
                            member.supported_contexts_value.len(),
                        );
                    }
                    println!("Encrypted GATT service access: PASS");
                    for (connection, reason) in
                        &result.bond.cleanup.disconnected
                    {
                        println!(
                            "Cleanup disconnect: handle=0x{:04X}@gen{} reason=0x{:02X}",
                            connection.connection_handle,
                            connection.generation,
                            reason,
                        );
                    }
                    println!(
                        "Privacy cleanup: {}",
                        result.bond.cleanup.privacy_cleared
                    );
                    println!(
                        "Bond store unchanged: {}",
                        result.bond.bond_unchanged
                    );
                    println!(
                        "Shutdown: hci={:?} transport={:?}",
                        result.bond.exit.hci,
                        result.bond.exit.transport
                    );
                    return;
                }
                Err(review2_gatt_smoke::GattSmokeExecuteError::Bond(
                    error,
                )) => {
                    eprintln!(
                        "Review-2C gatt-smoke bond/session FAILED: {error:?}"
                    );
                    std::process::exit(1);
                }
                Err(review2_gatt_smoke::GattSmokeExecuteError::Probe {
                    detail,
                    cleanup,
                    bond_unchanged,
                    exit,
                }) => {
                    eprintln!(
                        "Review-2C gatt-smoke PROBE FAILED: {detail}"
                    );
                    for error in cleanup.errors {
                        eprintln!("Cleanup issue: {error}");
                    }
                    eprintln!(
                        "Privacy cleanup: {}; Bond store unchanged: {}",
                        cleanup.privacy_cleared,
                        bond_unchanged
                    );
                    eprintln!(
                        "Shutdown: hci={:?} transport={:?}",
                        exit.hci, exit.transport
                    );
                    std::process::exit(1);
                }
                Err(
                    review2_gatt_smoke::GattSmokeExecuteError::ProbeAndCleanup {
                        detail,
                        cleanup,
                    },
                ) => {
                    eprintln!(
                        "Review-2C gatt-smoke PROBE FAILED: {detail}"
                    );
                    eprintln!(
                        "Review-2C cleanup also FAILED: {cleanup:?}"
                    );
                    std::process::exit(1);
                }
            }
        }

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
