use std::time::Duration;

use bumble_hci::metadata::supported_command_names;
use bumble_hci::{Command, ReturnParameters};
use bumble_host::{
    ControllerBufferInfo, Device, HOST_DEFAULT_MAXIMUM_ADVERTISING_DATA_LENGTH,
    HOST_EVENT_MASK, HOST_EVENT_MASK_PAGE_2, HOST_LE_EVENT_MASK, HOST_LE_EVENT_MASK_LEGACY,
    HOST_SUGGESTED_MAX_TX_OCTETS, HOST_SUGGESTED_MAX_TX_TIME, LeSuggestedDefaultDataLength,
};
use bumble_transport::{
    open_split_transport, Error, ExternalControllerInfo, ExternalHost, LocalControllerVersion,
    Result as TransportResult,
};

const BE200_TRANSPORT: &str = "usb:8087:0036";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachReport {
    pub hci_version: Option<u8>,
    pub manufacturer: Option<u16>,
    pub hci_subversion: Option<u16>,
    pub le_acl_data_packet_length: u16,
    pub total_num_le_acl_data_packets: u8,
    pub iso_data_packet_length: u16,
    pub total_num_iso_data_packets: u8,
    pub maximum_advertising_data_length: u16,
}

pub struct AttachedController {
    host: ExternalHost,
    device: Device,
    info: ExternalControllerInfo,
}

impl AttachedController {
    pub fn report(&self) -> AttachReport {
        let version = self.info.local_version.as_ref();
        AttachReport {
            hci_version: version.map(|value| value.hci_version),
            manufacturer: version.map(|value| value.company_identifier),
            hci_subversion: version.map(|value| value.hci_subversion),
            le_acl_data_packet_length: self.info.le_acl_data_packet_length,
            total_num_le_acl_data_packets: self.info.total_num_le_acl_data_packets,
            iso_data_packet_length: self.info.iso_data_packet_length,
            total_num_iso_data_packets: self.info.total_num_iso_data_packets,
            maximum_advertising_data_length: self.info.maximum_advertising_data_length,
        }
    }

    pub fn host_state(&self) -> &bumble_transport::ExternalHostState {
        self.host.state()
    }

    pub fn device(&self) -> &Device {
        &self.device
    }
}

pub fn attach_be200_existing_operational(
    timeout: Duration,
) -> TransportResult<AttachedController> {
    let transport = open_split_transport(BE200_TRANSPORT)?;
    let mut host = ExternalHost::new(transport);
    let mut device = Device::new(0);
    let info = initialize_existing_operational(&mut host, &mut device, timeout)?;
    Ok(AttachedController { host, device, info })
}

/// Mirror Bumble-RS ExternalHost::initialize_device(), except the controller
/// Reset command is deliberately omitted.
///
/// R1.5b/R1.6 already established that this BE200 is in Intel Operational
/// firmware. The first R2 hardware experiment preserves that controller state
/// and only re-establishes Host-side capability/event-mask/flow-control state.
///
/// Keep this function mechanically aligned with the pinned upstream
/// `ExternalHost::initialize_device()`. Any upstream refresh must diff this
/// function before changing the pinned revision.
pub fn initialize_existing_operational(
    host: &mut ExternalHost,
    device: &mut Device,
    timeout: Duration,
) -> TransportResult<ExternalControllerInfo> {
    // IMPORTANT: no Command::Reset here.
    let supported_commands =
        match send_successful_command(host, Command::ReadLocalSupportedCommands, timeout)? {
            ReturnParameters::ReadLocalSupportedCommands {
                supported_commands, ..
            } => supported_commands,
            response => {
                return Err(Error::Remote(format!(
                    "unexpected Read Local Supported Commands response: {response:?}"
                )))
            }
        };
    let supported_names = supported_command_names(&supported_commands);

    let local_version =
        if supported_names.contains(&"HCI_READ_LOCAL_VERSION_INFORMATION_COMMAND") {
            match send_successful_command(host, Command::ReadLocalVersionInformation, timeout)? {
                ReturnParameters::ReadLocalVersionInformation {
                    hci_version,
                    hci_subversion,
                    lmp_version,
                    company_identifier,
                    lmp_subversion,
                    ..
                } => Some(LocalControllerVersion {
                    hci_version,
                    hci_subversion,
                    lmp_version,
                    company_identifier,
                    lmp_subversion,
                }),
                response => {
                    return Err(Error::Remote(format!(
                        "unexpected Read Local Version Information response: {response:?}"
                    )))
                }
            }
        } else {
            None
        };

    let (local_le_features, local_le_features_max_page) = if supported_names
        .contains(&"HCI_LE_READ_ALL_LOCAL_SUPPORTED_FEATURES_COMMAND")
    {
        match send_successful_command(host, Command::LeReadAllLocalSupportedFeatures, timeout)? {
            ReturnParameters::LeReadAllLocalSupportedFeatures {
                max_page,
                le_features,
                ..
            } => (Some(le_features.to_vec()), Some(max_page)),
            response => {
                return Err(Error::Remote(format!(
                    "unexpected LE Read All Local Supported Features response: {response:?}"
                )))
            }
        }
    } else if supported_names.contains(&"HCI_LE_READ_LOCAL_SUPPORTED_FEATURES_COMMAND") {
        match send_successful_command(host, Command::LeReadLocalSupportedFeatures, timeout)? {
            ReturnParameters::LeReadLocalSupportedFeatures { le_features, .. } => {
                (Some(le_features.to_vec()), None)
            }
            response => {
                return Err(Error::Remote(format!(
                    "unexpected LE Read Local Supported Features response: {response:?}"
                )))
            }
        }
    } else {
        (None, None)
    };

    let mut local_lmp_features = Vec::new();
    if supported_names.contains(&"HCI_READ_LOCAL_EXTENDED_FEATURES_COMMAND") {
        let mut page_number = 0;
        loop {
            match send_successful_command(
                host,
                Command::ReadLocalExtendedFeatures { page_number },
                timeout,
            )? {
                ReturnParameters::ReadLocalExtendedFeatures {
                    page_number: response_page,
                    maximum_page_number,
                    extended_lmp_features,
                    ..
                } => {
                    if response_page != page_number {
                        return Err(Error::Remote(format!(
                            "Read Local Extended Features returned page {response_page}, expected {page_number}"
                        )));
                    }
                    local_lmp_features.push(extended_lmp_features);
                    if page_number >= maximum_page_number {
                        break;
                    }
                    page_number = page_number.checked_add(1).ok_or_else(|| {
                        Error::Remote("local LMP feature page number overflow".into())
                    })?;
                }
                response => {
                    return Err(Error::Remote(format!(
                        "unexpected Read Local Extended Features response: {response:?}"
                    )))
                }
            }
        }
    } else if supported_names.contains(&"HCI_READ_LOCAL_SUPPORTED_FEATURES_COMMAND") {
        match send_successful_command(host, Command::ReadLocalSupportedFeatures, timeout)? {
            ReturnParameters::ReadLocalSupportedFeatures { lmp_features, .. } => {
                local_lmp_features.push(lmp_features);
            }
            response => {
                return Err(Error::Remote(format!(
                    "unexpected Read Local Supported Features response: {response:?}"
                )))
            }
        }
    }

    send_successful_command(
        host,
        Command::SetEventMask {
            event_mask: HOST_EVENT_MASK,
        },
        timeout,
    )?;
    if supported_names.contains(&"HCI_SET_EVENT_MASK_PAGE_2_COMMAND") {
        send_successful_command(
            host,
            Command::SetEventMaskPage2 {
                event_mask_page_2: HOST_EVENT_MASK_PAGE_2,
            },
            timeout,
        )?;
    }
    let le_event_mask = if local_version
        .as_ref()
        .is_some_and(|version| version.hci_version <= 6)
    {
        HOST_LE_EVENT_MASK_LEGACY
    } else {
        HOST_LE_EVENT_MASK
    };
    send_successful_command(host, Command::LeSetEventMask { le_event_mask }, timeout)?;

    let mut info = ExternalControllerInfo {
        supported_commands,
        local_version,
        local_le_features,
        local_le_features_max_page,
        local_lmp_features,
        acl_data_packet_length: 0,
        total_num_acl_data_packets: 0,
        le_acl_data_packet_length: 0,
        total_num_le_acl_data_packets: 0,
        iso_data_packet_length: 0,
        total_num_iso_data_packets: 0,
        suggested_default_data_length: None,
        number_of_supported_advertising_sets: 0,
        maximum_advertising_data_length: HOST_DEFAULT_MAXIMUM_ADVERTISING_DATA_LENGTH,
    };

    if supported_names.contains(&"HCI_READ_BUFFER_SIZE_COMMAND") {
        match send_successful_command(host, Command::ReadBufferSize, timeout)? {
            ReturnParameters::ReadBufferSize {
                hc_acl_data_packet_length,
                hc_total_num_acl_data_packets,
                ..
            } => {
                info.acl_data_packet_length = hc_acl_data_packet_length;
                info.total_num_acl_data_packets = hc_total_num_acl_data_packets;
            }
            response => {
                return Err(Error::Remote(format!(
                    "unexpected Read Buffer Size response: {response:?}"
                )))
            }
        }
    }

    if supported_names.contains(&"HCI_LE_READ_BUFFER_SIZE_V2_COMMAND") {
        match send_successful_command(host, Command::LeReadBufferSizeV2, timeout)? {
            ReturnParameters::LeReadBufferSizeV2 {
                le_acl_data_packet_length,
                total_num_le_acl_data_packets,
                iso_data_packet_length,
                total_num_iso_data_packets,
                ..
            } => {
                info.le_acl_data_packet_length = le_acl_data_packet_length;
                info.total_num_le_acl_data_packets = total_num_le_acl_data_packets;
                info.iso_data_packet_length = iso_data_packet_length;
                info.total_num_iso_data_packets = total_num_iso_data_packets;
            }
            response => {
                return Err(Error::Remote(format!(
                    "unexpected LE Read Buffer Size V2 response: {response:?}"
                )))
            }
        }
    } else if supported_names.contains(&"HCI_LE_READ_BUFFER_SIZE_COMMAND") {
        match send_successful_command(host, Command::LeReadBufferSize, timeout)? {
            ReturnParameters::LeReadBufferSize {
                le_acl_data_packet_length,
                total_num_le_acl_data_packets,
                ..
            } => {
                info.le_acl_data_packet_length = le_acl_data_packet_length;
                info.total_num_le_acl_data_packets = total_num_le_acl_data_packets;
            }
            response => {
                return Err(Error::Remote(format!(
                    "unexpected LE Read Buffer Size response: {response:?}"
                )))
            }
        }
    }

    if supported_names.contains(&"HCI_LE_READ_SUGGESTED_DEFAULT_DATA_LENGTH_COMMAND")
        && supported_names.contains(&"HCI_LE_WRITE_SUGGESTED_DEFAULT_DATA_LENGTH_COMMAND")
    {
        match send_successful_command(host, Command::LeReadSuggestedDefaultDataLength, timeout)? {
            ReturnParameters::LeReadSuggestedDefaultDataLength {
                suggested_max_tx_octets,
                suggested_max_tx_time,
                ..
            } => {
                let mut suggestion = LeSuggestedDefaultDataLength {
                    suggested_max_tx_octets,
                    suggested_max_tx_time,
                };
                let target = LeSuggestedDefaultDataLength {
                    suggested_max_tx_octets: HOST_SUGGESTED_MAX_TX_OCTETS,
                    suggested_max_tx_time: HOST_SUGGESTED_MAX_TX_TIME,
                };
                if suggestion != target {
                    send_successful_command(
                        host,
                        Command::LeWriteSuggestedDefaultDataLength {
                            suggested_max_tx_octets: HOST_SUGGESTED_MAX_TX_OCTETS,
                            suggested_max_tx_time: HOST_SUGGESTED_MAX_TX_TIME,
                        },
                        timeout,
                    )?;
                    suggestion = target;
                }
                info.suggested_default_data_length = Some(suggestion);
            }
            response => {
                return Err(Error::Remote(format!(
                    "unexpected LE Read Suggested Default Data Length response: {response:?}"
                )))
            }
        }
    }

    if supported_names.contains(&"HCI_LE_READ_NUMBER_OF_SUPPORTED_ADVERTISING_SETS_COMMAND") {
        if let Some(response) = send_optional_successful_command(
            host,
            Command::LeReadNumberOfSupportedAdvertisingSets,
            timeout,
        )? {
            match response {
                ReturnParameters::LeReadNumberOfSupportedAdvertisingSets {
                    num_supported_advertising_sets,
                    ..
                } => info.number_of_supported_advertising_sets = num_supported_advertising_sets,
                response => {
                    return Err(Error::Remote(format!(
                        "unexpected LE Read Number Of Supported Advertising Sets response: {response:?}"
                    )))
                }
            }
        }
    }

    if supported_names.contains(&"HCI_LE_READ_MAXIMUM_ADVERTISING_DATA_LENGTH_COMMAND") {
        if let Some(response) = send_optional_successful_command(
            host,
            Command::LeReadMaximumAdvertisingDataLength,
            timeout,
        )? {
            match response {
                ReturnParameters::LeReadMaximumAdvertisingDataLength {
                    max_advertising_data_length,
                    ..
                } => info.maximum_advertising_data_length = max_advertising_data_length,
                response => {
                    return Err(Error::Remote(format!(
                        "unexpected LE Read Maximum Advertising Data Length response: {response:?}"
                    )))
                }
            }
        }
    }

    let classic_acl = supported_names
        .contains(&"HCI_READ_BUFFER_SIZE_COMMAND")
        .then_some(ControllerBufferInfo {
            data_packet_length: info.acl_data_packet_length,
            total_num_data_packets: info.total_num_acl_data_packets,
        });
    let has_le_acl_pool = supported_names.contains(&"HCI_LE_READ_BUFFER_SIZE_V2_COMMAND")
        || supported_names.contains(&"HCI_LE_READ_BUFFER_SIZE_COMMAND");
    let le_acl = has_le_acl_pool.then_some(ControllerBufferInfo {
        data_packet_length: info.le_acl_data_packet_length,
        total_num_data_packets: u16::from(info.total_num_le_acl_data_packets),
    });
    let iso = supported_names
        .contains(&"HCI_LE_READ_BUFFER_SIZE_V2_COMMAND")
        .then_some(ControllerBufferInfo {
            data_packet_length: info.iso_data_packet_length,
            total_num_data_packets: u16::from(info.total_num_iso_data_packets),
        });

    if !device.configure_controller_packet_pools(classic_acl, le_acl, iso) {
        return Err(Error::Remote(
            "cannot replace controller packet pools while packets are pending".into(),
        ));
    }

    Ok(info)
}

fn send_successful_command(
    host: &mut ExternalHost,
    command: Command,
    timeout: Duration,
) -> TransportResult<ReturnParameters> {
    let opcode = command.op_code();
    let response = host.send_command(command, timeout)?;
    if response.status() != Some(0) {
        return Err(Error::Remote(format!(
            "HCI command {opcode:#06x} failed with status {:?}",
            response.status()
        )));
    }
    response.return_parameters().cloned().ok_or_else(|| {
        Error::Remote(format!(
            "HCI command {opcode:#06x} returned Command Status instead of Command Complete"
        ))
    })
}

fn send_optional_successful_command(
    host: &mut ExternalHost,
    command: Command,
    timeout: Duration,
) -> TransportResult<Option<ReturnParameters>> {
    let opcode = command.op_code();
    let response = host.send_command(command, timeout)?;
    if response.status() != Some(0) {
        return Ok(None);
    }
    response
        .return_parameters()
        .cloned()
        .map(Some)
        .ok_or_else(|| {
            Error::Remote(format!(
                "HCI command {opcode:#06x} returned Command Status instead of Command Complete"
            ))
        })
}
