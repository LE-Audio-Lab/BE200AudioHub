use std::{
    thread,
    time::{Duration, Instant},
};

use bumble::Uuid;

use crate::{
    hci_host::mailbox::HciReport,
    plane::l1_hci::{
        acl::connection::ConnectionToken,
        gatt::transaction::{
            GattCharacteristic, GattOperation, GattReport, GattRequest,
            GattResult, GattService,
        },
    },
    review::review2::bond_smoke::{
        open_dual_encrypted_bond_session, BondSmokeCleanupSummary,
        BondSmokeConfig, BondSmokeExecuteError, BondSmokeResult,
        BondSmokeTracker,
    },
    runtime::mailbox::{RecvError, SendError},
};

pub const PACS_UUID: u16 = 0x1850;
pub const ASCS_UUID: u16 = 0x184E;
pub const CAS_UUID: u16 = 0x1853;
pub const CSIS_UUID: u16 = 0x1846;
pub const PACS_SUPPORTED_AUDIO_CONTEXTS_UUID: u16 = 0x2BCE;

const DEFAULT_GATT_TIMEOUT_SECONDS: u64 = 5;
const DEFAULT_CLIENT_MTU: u16 = 247;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattSmokeConfig {
    pub bond: BondSmokeConfig,
    pub gatt_timeout: Duration,
}

pub fn parse_gatt_smoke_args(
    args: &[String],
) -> Result<GattSmokeConfig, String> {
    let mut bond_args = Vec::new();
    let mut gatt_timeout =
        Duration::from_secs(DEFAULT_GATT_TIMEOUT_SECONDS);
    let mut confirmed = false;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--gatt-seconds" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| {
                        "missing value for --gatt-seconds".to_string()
                    })?;
                let seconds: u64 = value.parse().map_err(|_| {
                    "--gatt-seconds must be an integer".to_string()
                })?;
                if !(2..=20).contains(&seconds) {
                    return Err(
                        "--gatt-seconds must be in 2..=20".into()
                    );
                }
                gatt_timeout = Duration::from_secs(seconds);
                index += 2;
            }
            "--confirm-gatt-hil" => {
                confirmed = true;
                index += 1;
            }
            "--sfi" | "--ddc" | "--bond-root" | "--name"
            | "--connect-seconds" => {
                let flag = args[index].clone();
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {flag}"))?
                    .clone();
                bond_args.push(flag);
                bond_args.push(value);
                index += 2;
            }
            unknown => {
                return Err(format!(
                    "unknown review2 gatt-smoke argument: {unknown}"
                ))
            }
        }
    }

    if !confirmed {
        return Err("--confirm-gatt-hil is required".into());
    }

    bond_args.push("--confirm-stored-bond-hil".into());
    let bond =
        crate::review::review2::bond_smoke::parse_bond_smoke_args(&bond_args)?;

    Ok(GattSmokeConfig {
        bond,
        gatt_timeout,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattServiceRange {
    pub start_handle: u16,
    pub end_handle: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattSmokeMemberResult {
    pub identity_address: String,
    pub connection: ConnectionToken,
    pub mtu: u16,
    pub service_count: usize,
    pub pacs: GattServiceRange,
    pub ascs: GattServiceRange,
    pub cas: GattServiceRange,
    pub pacs_characteristic_count: usize,
    pub ascs_characteristic_count: usize,
    pub cas_included_service_count: usize,
    pub cas_includes_csis: bool,
    pub descriptor_probe_count: usize,
    pub supported_contexts_handle: u16,
    pub supported_contexts_value: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattSmokeResult {
    pub bond: BondSmokeResult,
    pub members: Vec<GattSmokeMemberResult>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GattSmokeExecuteError {
    Bond(BondSmokeExecuteError),
    Probe {
        detail: String,
        cleanup: BondSmokeCleanupSummary,
        bond_unchanged: bool,
        exit: crate::hci_host::stack::HciStackExit,
    },
    ProbeAndCleanup {
        detail: String,
        cleanup: Box<BondSmokeExecuteError>,
    },
}

fn send_error(stage: &str, error: SendError) -> String {
    format!("{stage}: HCI control lane rejected request: {error:?}")
}

fn wait_gatt_result(
    stack: &crate::hci_host::stack::HciStack,
    tracker: &mut BondSmokeTracker,
    connection: ConnectionToken,
    request_id: u64,
    timeout: Duration,
) -> Result<GattResult, String> {
    let deadline = Instant::now() + timeout;

    loop {
        match stack.reports().try_recv() {
            Ok(report) => {
                tracker.observe(&report);
                match report {
                    HciReport::Gatt(GattReport::Complete {
                        request_id: actual_id,
                        connection: actual_connection,
                        result,
                    }) if actual_id == request_id
                        && actual_connection == connection =>
                    {
                        return Ok(result)
                    }
                    HciReport::Gatt(GattReport::Failed(failure))
                        if failure.request_id == request_id
                            && failure.connection == connection =>
                    {
                        return Err(format!(
                            "GATT request {request_id} failed: {:?}",
                            failure.kind
                        ))
                    }
                    HciReport::Gatt(
                        GattReport::BearerPoisoned {
                            request_id: actual_id,
                            connection: actual_connection,
                        },
                    ) if actual_id == request_id
                        && actual_connection == connection =>
                    {
                        return Err(format!(
                            "GATT bearer poisoned after request {request_id}"
                        ))
                    }
                    _ => {}
                }
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    let cancel = stack.control().try_cancel_gatt(
                        connection,
                        request_id,
                    );
                    return Err(match cancel {
                        Ok(()) => format!(
                            "GATT request {request_id} timed out; cancel requested and bearer will remain poisoned until disconnect"
                        ),
                        Err(error) => format!(
                            "GATT request {request_id} timed out; cancel enqueue failed: {error:?}"
                        ),
                    });
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err("HCI report lane disconnected".into())
            }
        }
    }
}

pub(crate) fn execute_gatt(
    stack: &crate::hci_host::stack::HciStack,
    tracker: &mut BondSmokeTracker,
    connection: ConnectionToken,
    request_id: u64,
    operation: GattOperation,
    timeout: Duration,
) -> Result<GattResult, String> {
    stack
        .control()
        .try_begin_gatt(GattRequest {
            request_id,
            connection,
            operation,
        })
        .map_err(|error| send_error("GATT request", error))?;

    wait_gatt_result(
        stack,
        tracker,
        connection,
        request_id,
        timeout,
    )
}

fn service_with_uuid<'a>(
    services: &'a [GattService],
    uuid: u16,
) -> Option<&'a GattService> {
    let wanted = Uuid::from_16_bits(uuid);
    services
        .iter()
        .find(|service| service.uuid == wanted)
}

pub(crate) fn characteristic_with_uuid<'a>(
    characteristics: &'a [GattCharacteristic],
    uuid: u16,
) -> Option<&'a GattCharacteristic> {
    let wanted = Uuid::from_16_bits(uuid);
    characteristics
        .iter()
        .find(|characteristic| characteristic.uuid == wanted)
}

pub(crate) fn required_service(
    services: &[GattService],
    uuid: u16,
    label: &str,
) -> Result<GattServiceRange, String> {
    let service = service_with_uuid(services, uuid)
        .ok_or_else(|| {
            format!("peer does not expose required {label} service")
        })?;

    Ok(GattServiceRange {
        start_handle: service.handle,
        end_handle: service.end_group_handle,
    })
}

fn inspect_member(
    stack: &crate::hci_host::stack::HciStack,
    tracker: &mut BondSmokeTracker,
    identity_address: &str,
    connection: ConnectionToken,
    timeout: Duration,
    next_request_id: &mut u64,
) -> Result<GattSmokeMemberResult, String> {
    let mtu_request = *next_request_id;
    *next_request_id += 1;
    let mtu = match execute_gatt(
        stack,
        tracker,
        connection,
        mtu_request,
        GattOperation::ExchangeMtu {
            client_rx_mtu: DEFAULT_CLIENT_MTU,
        },
        timeout,
    )? {
        GattResult::Mtu { mtu } => mtu,
        other => {
            return Err(format!(
                "unexpected Exchange MTU result: {other:?}"
            ))
        }
    };

    let services_request = *next_request_id;
    *next_request_id += 1;
    let services = match execute_gatt(
        stack,
        tracker,
        connection,
        services_request,
        GattOperation::DiscoverServices,
        timeout,
    )? {
        GattResult::Services(services) => services,
        other => {
            return Err(format!(
                "unexpected service discovery result: {other:?}"
            ))
        }
    };

    let pacs = required_service(&services, PACS_UUID, "PACS")?;
    let ascs = required_service(&services, ASCS_UUID, "ASCS")?;
    let cas = required_service(&services, CAS_UUID, "CAS")?;

    let chars_request = *next_request_id;
    *next_request_id += 1;
    let characteristics = match execute_gatt(
        stack,
        tracker,
        connection,
        chars_request,
        GattOperation::DiscoverCharacteristics {
            service_handle: pacs.start_handle,
            end_group_handle: pacs.end_handle,
        },
        timeout,
    )? {
        GattResult::Characteristics(values) => values,
        other => {
            return Err(format!(
                "unexpected PACS characteristic result: {other:?}"
            ))
        }
    };

    let ascs_chars_request = *next_request_id;
    *next_request_id += 1;
    let ascs_characteristics = match execute_gatt(
        stack,
        tracker,
        connection,
        ascs_chars_request,
        GattOperation::DiscoverCharacteristics {
            service_handle: ascs.start_handle,
            end_group_handle: ascs.end_handle,
        },
        timeout,
    )? {
        GattResult::Characteristics(values) => values,
        other => {
            return Err(format!(
                "unexpected ASCS characteristic result: {other:?}"
            ))
        }
    };

    if ascs_characteristics.is_empty() {
        return Err("ASCS characteristic discovery returned empty".into());
    }

    let include_request = *next_request_id;
    *next_request_id += 1;
    let included_services = match execute_gatt(
        stack,
        tracker,
        connection,
        include_request,
        GattOperation::DiscoverIncludedServices {
            service_handle: cas.start_handle,
            end_group_handle: cas.end_handle,
        },
        timeout,
    )? {
        GattResult::IncludedServices(values) => values,
        other => {
            return Err(format!(
                "unexpected CAS included-service result: {other:?}"
            ))
        }
    };

    let cas_includes_csis = included_services
        .iter()
        .any(|service| service.uuid == Uuid::from_16_bits(CSIS_UUID));
    if !cas_includes_csis {
        return Err("CAS does not include CSIS".into());
    }

    let supported = characteristic_with_uuid(
        &characteristics,
        PACS_SUPPORTED_AUDIO_CONTEXTS_UUID,
    )
    .ok_or_else(|| {
        "PACS does not expose Supported Audio Contexts characteristic"
            .to_string()
    })?;

    let read_request = *next_request_id;
    *next_request_id += 1;
    let supported_contexts_value = match execute_gatt(
        stack,
        tracker,
        connection,
        read_request,
        GattOperation::ReadValue {
            attribute_handle: supported.handle,
        },
        timeout,
    )? {
        GattResult::Value {
            attribute_handle,
            value,
        } if attribute_handle == supported.handle => value,
        other => {
            return Err(format!(
                "unexpected Supported Audio Contexts read result: {other:?}"
            ))
        }
    };

    if supported_contexts_value.is_empty() {
        return Err(
            "PACS Supported Audio Contexts returned an empty value".into()
        );
    }

    let descriptor_probe_count = if let Some(characteristic) =
        ascs_characteristics.iter().find(|characteristic| {
            characteristic.handle < characteristic.end_group_handle
        })
    {
        let descriptor_request = *next_request_id;
        *next_request_id += 1;
        match execute_gatt(
            stack,
            tracker,
            connection,
            descriptor_request,
            GattOperation::DiscoverDescriptors {
                starting_handle: characteristic.handle + 1,
                ending_handle: characteristic.end_group_handle,
            },
            timeout,
        )? {
            GattResult::Descriptors(values) => values.len(),
            other => {
                return Err(format!(
                    "unexpected descriptor discovery result: {other:?}"
                ))
            }
        }
    } else {
        0
    };

    Ok(GattSmokeMemberResult {
        identity_address: identity_address.to_string(),
        connection,
        mtu,
        service_count: services.len(),
        pacs,
        ascs,
        cas,
        pacs_characteristic_count: characteristics.len(),
        ascs_characteristic_count: ascs_characteristics.len(),
        cas_included_service_count: included_services.len(),
        cas_includes_csis,
        descriptor_probe_count,
        supported_contexts_handle: supported.handle,
        supported_contexts_value,
    })
}

#[cfg(windows)]
pub fn execute_gatt_smoke(
    config: GattSmokeConfig,
) -> Result<GattSmokeResult, GattSmokeExecuteError> {
    let mut session =
        open_dual_encrypted_bond_session(config.bond)
            .map_err(GattSmokeExecuteError::Bond)?;

    let members = session.members.clone();
    let mut next_request_id = 1u64;
    let probe = (|| -> Result<Vec<GattSmokeMemberResult>, String> {
        let mut output = Vec::with_capacity(members.len());
        for member in &members {
            output.push(inspect_member(
                &session.stack,
                &mut session.tracker,
                &member.identity_address,
                member.connection,
                config.gatt_timeout,
                &mut next_request_id,
            )?);
        }
        Ok(output)
    })();

    let finish = session.finish();

    match (probe, finish) {
        (Ok(members), Ok(bond)) => {
            Ok(GattSmokeResult { bond, members })
        }
        (Ok(_), Err(error)) => Err(GattSmokeExecuteError::Bond(error)),
        (Err(detail), Ok(bond)) => Err(GattSmokeExecuteError::Probe {
            detail,
            cleanup: bond.cleanup,
            bond_unchanged: bond.bond_unchanged,
            exit: bond.exit,
        }),
        (Err(detail), Err(cleanup)) => {
            Err(GattSmokeExecuteError::ProbeAndCleanup {
                detail,
                cleanup: Box::new(cleanup),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_requires_explicit_gatt_hil_consent() {
        let args = vec![
            "--sfi".into(), "a.sfi".into(),
            "--ddc".into(), "a.ddc".into(),
            "--bond-root".into(), "bonding".into(),
            "--name".into(), "Galaxy Buds3 Pro (2F23)".into(),
        ];
        assert!(parse_gatt_smoke_args(&args).is_err());

        let mut confirmed = args;
        confirmed.push("--confirm-gatt-hil".into());
        let config = parse_gatt_smoke_args(&confirmed).unwrap();
        assert_eq!(
            config.gatt_timeout,
            Duration::from_secs(DEFAULT_GATT_TIMEOUT_SECONDS)
        );
    }

    #[test]
    fn required_audio_services_are_identified_by_uuid_not_handle() {
        let services = vec![
            GattService {
                handle: 0x0040,
                end_group_handle: 0x0049,
                uuid: Uuid::from_16_bits(ASCS_UUID),
            },
            GattService {
                handle: 0x0020,
                end_group_handle: 0x0035,
                uuid: Uuid::from_16_bits(PACS_UUID),
            },
            GattService {
                handle: 0x0050,
                end_group_handle: 0x0055,
                uuid: Uuid::from_16_bits(CAS_UUID),
            },
        ];

        assert_eq!(
            required_service(&services, PACS_UUID, "PACS").unwrap(),
            GattServiceRange {
                start_handle: 0x0020,
                end_handle: 0x0035,
            }
        );
        assert!(required_service(&services, 0x1800, "fake").is_err());
    }
}
