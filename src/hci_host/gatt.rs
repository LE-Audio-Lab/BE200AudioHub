use bumble::Uuid;
use bumble_att::AttPdu;

use super::connection::ConnectionToken;

pub const ATT_DEFAULT_MTU: u16 = 23;
pub const GATT_PRIMARY_SERVICE_UUID: u16 = 0x2800;
pub const GATT_CHARACTERISTIC_UUID: u16 = 0x2803;
pub const CCCD_NOTIFICATION: u16 = 0x0001;
pub const CCCD_INDICATION: u16 = 0x0002;
const ATT_ATTRIBUTE_NOT_FOUND_ERROR: u8 = 0x0A;
const ATT_ATTRIBUTE_NOT_LONG_ERROR: u8 = 0x0B;
const ATT_INVALID_OFFSET_ERROR: u8 = 0x07;

pub type GattRequestId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattService {
    pub handle: u16,
    pub end_group_handle: u16,
    pub uuid: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattCharacteristic {
    pub declaration_handle: u16,
    pub handle: u16,
    pub end_group_handle: u16,
    pub properties: u8,
    pub uuid: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattDescriptor {
    pub handle: u16,
    pub uuid: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GattOperation {
    ExchangeMtu { client_rx_mtu: u16 },
    DiscoverServices,
    DiscoverCharacteristics {
        service_handle: u16,
        end_group_handle: u16,
    },
    DiscoverDescriptors {
        starting_handle: u16,
        ending_handle: u16,
    },
    ReadValue {
        attribute_handle: u16,
    },
    WriteRequest {
        attribute_handle: u16,
        value: Vec<u8>,
    },
    WriteCommand {
        attribute_handle: u16,
        value: Vec<u8>,
    },
    Subscribe {
        value_handle: u16,
        cccd_handle: u16,
        indicate: bool,
    },
    Unsubscribe {
        value_handle: u16,
        cccd_handle: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattRequest {
    pub request_id: GattRequestId,
    pub connection: ConnectionToken,
    pub operation: GattOperation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GattResult {
    Mtu { mtu: u16 },
    Services(Vec<GattService>),
    Characteristics(Vec<GattCharacteristic>),
    Descriptors(Vec<GattDescriptor>),
    Value {
        attribute_handle: u16,
        value: Vec<u8>,
    },
    WriteComplete {
        attribute_handle: u16,
        with_response: bool,
    },
    Subscription {
        value_handle: u16,
        cccd_handle: u16,
        indicate: bool,
        enabled: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GattFailureKind {
    InvalidRequest(String),
    Att {
        request_opcode: u8,
        attribute_handle: u16,
        error_code: u8,
    },
    Protocol(String),
    Disconnected { reason: u8 },
    Cancelled,
    NotEncrypted,
    Busy,
    BearerPoisoned { previous_request_id: GattRequestId },
    DuplicateRequestId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattFailure {
    pub request_id: GattRequestId,
    pub connection: ConnectionToken,
    pub kind: GattFailureKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GattValueEvent {
    Notification {
        connection: ConnectionToken,
        attribute_handle: u16,
        value: Vec<u8>,
    },
    Indication {
        connection: ConnectionToken,
        attribute_handle: u16,
        value: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GattReport {
    Complete {
        request_id: GattRequestId,
        connection: ConnectionToken,
        result: GattResult,
    },
    Failed(GattFailure),
    Value(GattValueEvent),
    BearerPoisoned {
        connection: ConnectionToken,
        request_id: GattRequestId,
    },
    LateResponseDropped {
        connection: ConnectionToken,
        request_id: GattRequestId,
        opcode: u8,
    },
    UnexpectedPdu {
        connection: ConnectionToken,
        opcode: u8,
    },
}

#[derive(Debug)]
pub enum GattAction {
    Send(AttPdu),
    Await,
    Complete(GattResult),
    Failed(GattFailure),
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    ExchangeMtu { client_rx_mtu: u16 },
    DiscoverServices { next: u16 },
    DiscoverCharacteristics { next: u16, end: u16 },
    DiscoverDescriptors { next: u16, end: u16 },
    ReadInitial { handle: u16 },
    ReadBlob { handle: u16, offset: u16 },
    WriteRequest { handle: u16 },
    WriteCommand { handle: u16 },
    Subscribe {
        value_handle: u16,
        cccd_handle: u16,
        indicate: bool,
        enabled: bool,
    },
    Complete,
    Failed,
}

#[derive(Debug)]
pub struct GattTransaction {
    request_id: GattRequestId,
    connection: ConnectionToken,
    mtu: u16,
    phase: Phase,
    services: Vec<GattService>,
    characteristics: Vec<GattCharacteristic>,
    descriptors: Vec<GattDescriptor>,
    read_value: Vec<u8>,
}

impl GattTransaction {
    pub fn begin(
        request: GattRequest,
        current_mtu: u16,
    ) -> Result<(Self, AttPdu), GattFailure> {
        validate_request(&request)?;
        let mtu = current_mtu.max(ATT_DEFAULT_MTU);
        let (phase, pdu) = match &request.operation {
            GattOperation::ExchangeMtu { client_rx_mtu } => (
                Phase::ExchangeMtu { client_rx_mtu: *client_rx_mtu },
                AttPdu::ExchangeMtuRequest {
                    client_rx_mtu: *client_rx_mtu,
                },
            ),
            GattOperation::DiscoverServices => (
                Phase::DiscoverServices { next: 0x0001 },
                service_request(0x0001),
            ),
            GattOperation::DiscoverCharacteristics {
                service_handle,
                end_group_handle,
            } => (
                Phase::DiscoverCharacteristics {
                    next: *service_handle,
                    end: *end_group_handle,
                },
                characteristic_request(*service_handle, *end_group_handle),
            ),
            GattOperation::DiscoverDescriptors {
                starting_handle,
                ending_handle,
            } => (
                Phase::DiscoverDescriptors {
                    next: *starting_handle,
                    end: *ending_handle,
                },
                AttPdu::FindInformationRequest {
                    starting_handle: *starting_handle,
                    ending_handle: *ending_handle,
                },
            ),
            GattOperation::ReadValue { attribute_handle } => (
                Phase::ReadInitial {
                    handle: *attribute_handle,
                },
                AttPdu::ReadRequest {
                    attribute_handle: *attribute_handle,
                },
            ),
            GattOperation::WriteRequest {
                attribute_handle,
                value,
            } => (
                Phase::WriteRequest {
                    handle: *attribute_handle,
                },
                AttPdu::WriteRequest {
                    attribute_handle: *attribute_handle,
                    attribute_value: value.clone(),
                },
            ),
            GattOperation::WriteCommand {
                attribute_handle,
                value,
            } => (
                Phase::WriteCommand {
                    handle: *attribute_handle,
                },
                AttPdu::WriteCommand {
                    attribute_handle: *attribute_handle,
                    attribute_value: value.clone(),
                },
            ),
            GattOperation::Subscribe {
                value_handle,
                cccd_handle,
                indicate,
            } => {
                let bits = if *indicate {
                    CCCD_INDICATION
                } else {
                    CCCD_NOTIFICATION
                };
                (
                    Phase::Subscribe {
                        value_handle: *value_handle,
                        cccd_handle: *cccd_handle,
                        indicate: *indicate,
                        enabled: true,
                    },
                    AttPdu::WriteRequest {
                        attribute_handle: *cccd_handle,
                        attribute_value: bits.to_le_bytes().to_vec(),
                    },
                )
            }
            GattOperation::Unsubscribe {
                value_handle,
                cccd_handle,
            } => (
                Phase::Subscribe {
                    value_handle: *value_handle,
                    cccd_handle: *cccd_handle,
                    indicate: false,
                    enabled: false,
                },
                AttPdu::WriteRequest {
                    attribute_handle: *cccd_handle,
                    attribute_value: 0u16.to_le_bytes().to_vec(),
                },
            ),
        };

        Ok((
            Self {
                request_id: request.request_id,
                connection: request.connection,
                mtu,
                phase,
                services: Vec::new(),
                characteristics: Vec::new(),
                descriptors: Vec::new(),
                read_value: Vec::new(),
            },
            pdu,
        ))
    }

    pub fn request_id(&self) -> GattRequestId {
        self.request_id
    }

    pub fn connection(&self) -> ConnectionToken {
        self.connection
    }

    pub fn mtu(&self) -> u16 {
        self.mtu
    }

    pub fn on_request_sent(&mut self) -> GattAction {
        match self.phase {
            Phase::WriteCommand { handle } => {
                self.phase = Phase::Complete;
                GattAction::Complete(GattResult::WriteComplete {
                    attribute_handle: handle,
                    with_response: false,
                })
            }
            _ => GattAction::Await,
        }
    }

    pub fn on_disconnect(&mut self, reason: u8) -> GattAction {
        if matches!(self.phase, Phase::Complete | Phase::Failed) {
            return GattAction::Ignored;
        }
        self.fail(GattFailureKind::Disconnected { reason })
    }

    pub fn cancel(&mut self) -> GattAction {
        if matches!(self.phase, Phase::Complete | Phase::Failed) {
            return GattAction::Ignored;
        }
        self.fail(GattFailureKind::Cancelled)
    }

    pub fn on_pdu(&mut self, pdu: AttPdu) -> GattAction {
        if matches!(self.phase, Phase::Complete | Phase::Failed) {
            return GattAction::Ignored;
        }

        if let AttPdu::ErrorResponse {
            request_opcode_in_error,
            attribute_handle_in_error,
            error_code,
        } = &pdu
        {
            if *error_code == ATT_ATTRIBUTE_NOT_FOUND_ERROR {
                match self.phase {
                    Phase::DiscoverServices { .. } => {
                        return self.complete_services();
                    }
                    Phase::DiscoverCharacteristics { end, .. } => {
                        return self.complete_characteristics(end);
                    }
                    Phase::DiscoverDescriptors { .. } => {
                        return self.complete_descriptors();
                    }
                    _ => {}
                }
            }
            if matches!(self.phase, Phase::ReadBlob { .. })
                && matches!(
                    *error_code,
                    ATT_ATTRIBUTE_NOT_LONG_ERROR | ATT_INVALID_OFFSET_ERROR
                )
            {
                let handle = match self.phase {
                    Phase::ReadBlob { handle, .. } => handle,
                    _ => unreachable!(),
                };
                return self.complete_value(handle);
            }
            return self.fail(GattFailureKind::Att {
                request_opcode: *request_opcode_in_error,
                attribute_handle: *attribute_handle_in_error,
                error_code: *error_code,
            });
        }

        match self.phase.clone() {
            Phase::ExchangeMtu { client_rx_mtu } => match pdu {
                AttPdu::ExchangeMtuResponse { server_rx_mtu } => {
                    self.mtu = client_rx_mtu
                        .min(server_rx_mtu)
                        .max(ATT_DEFAULT_MTU);
                    self.complete(GattResult::Mtu { mtu: self.mtu })
                }
                other => self.unexpected("Exchange MTU Response", &other),
            },
            Phase::DiscoverServices { next } => match pdu {
                AttPdu::ReadByGroupTypeResponse {
                    length,
                    attribute_data_list,
                } => {
                    let entries = match parse_group_entries(
                        length,
                        &attribute_data_list,
                    ) {
                        Ok(entries) => entries,
                        Err(error) => return self.fail(error),
                    };
                    if entries.is_empty() {
                        return self.complete_services();
                    }
                    let mut last_end = 0u16;
                    for (handle, end_group_handle, value) in entries {
                        if handle < next || end_group_handle < handle {
                            return self.fail(GattFailureKind::Protocol(
                                format!(
                                    "invalid service range {handle:#06x}..={end_group_handle:#06x}"
                                ),
                            ));
                        }
                        let uuid = match Uuid::from_bytes(&value) {
                            Ok(uuid) => uuid,
                            Err(error) => {
                                return self.fail(GattFailureKind::Protocol(
                                    format!("invalid service UUID: {error}"),
                                ))
                            }
                        };
                        self.services.push(GattService {
                            handle,
                            end_group_handle,
                            uuid,
                        });
                        last_end = end_group_handle;
                    }
                    if last_end == 0xFFFF {
                        self.complete_services()
                    } else {
                        let next = last_end.saturating_add(1);
                        self.phase = Phase::DiscoverServices { next };
                        GattAction::Send(service_request(next))
                    }
                }
                other => self.unexpected("Read By Group Type Response", &other),
            },
            Phase::DiscoverCharacteristics { next, end } => match pdu {
                AttPdu::ReadByTypeResponse {
                    length,
                    attribute_data_list,
                } => {
                    let entries = match parse_type_entries(
                        length,
                        &attribute_data_list,
                    ) {
                        Ok(entries) => entries,
                        Err(error) => return self.fail(error),
                    };
                    if entries.is_empty() {
                        finalize_characteristics(
                            &mut self.characteristics,
                            end,
                        );
                        return self.complete_characteristics(end);
                    }
                    let mut last_decl = 0u16;
                    for (declaration_handle, value) in entries {
                        if declaration_handle < next || value.len() < 3 {
                            return self.fail(GattFailureKind::Protocol(
                                "invalid characteristic declaration".into(),
                            ));
                        }
                        let properties = value[0];
                        let handle =
                            u16::from_le_bytes([value[1], value[2]]);
                        let uuid = match Uuid::from_bytes(&value[3..]) {
                            Ok(uuid) => uuid,
                            Err(error) => {
                                return self.fail(GattFailureKind::Protocol(
                                    format!(
                                        "invalid characteristic UUID: {error}"
                                    ),
                                ))
                            }
                        };
                        if let Some(previous) =
                            self.characteristics.last_mut()
                        {
                            previous.end_group_handle =
                                declaration_handle.saturating_sub(1);
                        }
                        self.characteristics.push(GattCharacteristic {
                            declaration_handle,
                            handle,
                            end_group_handle: end,
                            properties,
                            uuid,
                        });
                        last_decl = declaration_handle;
                    }
                    if last_decl >= end {
                        self.complete_characteristics(end)
                    } else {
                        let next = last_decl.saturating_add(1);
                        self.phase =
                            Phase::DiscoverCharacteristics { next, end };
                        GattAction::Send(characteristic_request(next, end))
                    }
                }
                other => self.unexpected("Read By Type Response", &other),
            },
            Phase::DiscoverDescriptors { next, end } => match pdu {
                AttPdu::FindInformationResponse {
                    format,
                    information_data,
                } => {
                    let entries = match parse_information_entries(
                        format,
                        &information_data,
                    ) {
                        Ok(entries) => entries,
                        Err(error) => return self.fail(error),
                    };
                    if entries.is_empty() {
                        return self.complete_descriptors();
                    }
                    let mut last = 0u16;
                    for (handle, uuid) in entries {
                        if handle < next {
                            return self.fail(GattFailureKind::Protocol(
                                format!(
                                    "descriptor handle regressed to {handle:#06x}"
                                ),
                            ));
                        }
                        self.descriptors.push(GattDescriptor { handle, uuid });
                        last = handle;
                    }
                    if last >= end {
                        self.complete_descriptors()
                    } else {
                        let next = last.saturating_add(1);
                        self.phase =
                            Phase::DiscoverDescriptors { next, end };
                        GattAction::Send(
                            AttPdu::FindInformationRequest {
                                starting_handle: next,
                                ending_handle: end,
                            },
                        )
                    }
                }
                other => self.unexpected("Find Information Response", &other),
            },
            Phase::ReadInitial { handle } => match pdu {
                AttPdu::ReadResponse { attribute_value } => {
                    let chunk = usize::from(self.mtu.saturating_sub(1));
                    let part_len = attribute_value.len();
                    self.read_value = attribute_value;
                    if part_len < chunk {
                        self.complete_value(handle)
                    } else {
                        let offset = u16::try_from(part_len)
                            .unwrap_or(u16::MAX);
                        self.phase = Phase::ReadBlob { handle, offset };
                        GattAction::Send(AttPdu::ReadBlobRequest {
                            attribute_handle: handle,
                            value_offset: offset,
                        })
                    }
                }
                other => self.unexpected("Read Response", &other),
            },
            Phase::ReadBlob { handle, offset } => match pdu {
                AttPdu::ReadBlobResponse {
                    part_attribute_value,
                } => {
                    let chunk = usize::from(self.mtu.saturating_sub(1));
                    let part_len = part_attribute_value.len();
                    self.read_value.extend_from_slice(&part_attribute_value);
                    if part_len < chunk {
                        self.complete_value(handle)
                    } else {
                        let next = offset.saturating_add(
                            u16::try_from(part_len).unwrap_or(u16::MAX),
                        );
                        self.phase =
                            Phase::ReadBlob { handle, offset: next };
                        GattAction::Send(AttPdu::ReadBlobRequest {
                            attribute_handle: handle,
                            value_offset: next,
                        })
                    }
                }
                other => self.unexpected("Read Blob Response", &other),
            },
            Phase::WriteRequest { handle } => match pdu {
                AttPdu::WriteResponse => {
                    self.complete(GattResult::WriteComplete {
                        attribute_handle: handle,
                        with_response: true,
                    })
                }
                other => self.unexpected("Write Response", &other),
            },
            Phase::WriteCommand { .. } => GattAction::Ignored,
            Phase::Subscribe {
                value_handle,
                cccd_handle,
                indicate,
                enabled,
            } => match pdu {
                AttPdu::WriteResponse => {
                    self.complete(GattResult::Subscription {
                        value_handle,
                        cccd_handle,
                        indicate,
                        enabled,
                    })
                }
                other => self.unexpected("Write Response", &other),
            },
            Phase::Complete | Phase::Failed => GattAction::Ignored,
        }
    }

    fn complete_services(&mut self) -> GattAction {
        let services = std::mem::take(&mut self.services);
        self.complete(GattResult::Services(services))
    }

    fn complete_characteristics(
        &mut self,
        end_group_handle: u16,
    ) -> GattAction {
        finalize_characteristics(
            &mut self.characteristics,
            end_group_handle,
        );
        let characteristics = std::mem::take(&mut self.characteristics);
        self.complete(GattResult::Characteristics(characteristics))
    }

    fn complete_descriptors(&mut self) -> GattAction {
        let descriptors = std::mem::take(&mut self.descriptors);
        self.complete(GattResult::Descriptors(descriptors))
    }

    fn complete_value(&mut self, attribute_handle: u16) -> GattAction {
        let value = std::mem::take(&mut self.read_value);
        self.complete(GattResult::Value {
            attribute_handle,
            value,
        })
    }

    fn complete(&mut self, result: GattResult) -> GattAction {
        self.phase = Phase::Complete;
        GattAction::Complete(result)
    }

    fn fail(&mut self, kind: GattFailureKind) -> GattAction {
        self.phase = Phase::Failed;
        GattAction::Failed(GattFailure {
            request_id: self.request_id,
            connection: self.connection,
            kind,
        })
    }

    fn unexpected(&mut self, expected: &str, actual: &AttPdu) -> GattAction {
        self.fail(GattFailureKind::Protocol(format!(
            "expected {expected}, got {actual:?}"
        )))
    }
}

fn validate_request(request: &GattRequest) -> Result<(), GattFailure> {
    let invalid = |message: String| GattFailure {
        request_id: request.request_id,
        connection: request.connection,
        kind: GattFailureKind::InvalidRequest(message),
    };

    match &request.operation {
        GattOperation::ExchangeMtu { client_rx_mtu }
            if *client_rx_mtu < ATT_DEFAULT_MTU =>
        {
            Err(invalid(format!(
                "client ATT MTU must be >= {ATT_DEFAULT_MTU}"
            )))
        }
        GattOperation::DiscoverCharacteristics {
            service_handle,
            end_group_handle,
        } if *service_handle == 0 || service_handle > end_group_handle => {
            Err(invalid("invalid service handle range".into()))
        }
        GattOperation::DiscoverDescriptors {
            starting_handle,
            ending_handle,
        } if *starting_handle == 0 || starting_handle > ending_handle => {
            Err(invalid("invalid descriptor handle range".into()))
        }
        GattOperation::ReadValue { attribute_handle }
        | GattOperation::WriteRequest {
            attribute_handle, ..
        }
        | GattOperation::WriteCommand {
            attribute_handle, ..
        } if *attribute_handle == 0 => {
            Err(invalid("attribute handle must be nonzero".into()))
        }
        GattOperation::Subscribe {
            value_handle,
            cccd_handle,
            ..
        }
        | GattOperation::Unsubscribe {
            value_handle,
            cccd_handle,
        } if *value_handle == 0 || *cccd_handle == 0 => {
            Err(invalid("value and CCCD handles must be nonzero".into()))
        }
        _ => Ok(()),
    }
}

fn service_request(starting_handle: u16) -> AttPdu {
    AttPdu::ReadByGroupTypeRequest {
        starting_handle,
        ending_handle: 0xFFFF,
        attribute_group_type: Uuid::from_16_bits(
            GATT_PRIMARY_SERVICE_UUID,
        ),
    }
}

fn characteristic_request(
    starting_handle: u16,
    ending_handle: u16,
) -> AttPdu {
    AttPdu::ReadByTypeRequest {
        starting_handle,
        ending_handle,
        attribute_type: Uuid::from_16_bits(GATT_CHARACTERISTIC_UUID),
    }
}

fn finalize_characteristics(
    characteristics: &mut [GattCharacteristic],
    end_group_handle: u16,
) {
    if let Some(last) = characteristics.last_mut() {
        last.end_group_handle = end_group_handle;
    }
}

fn parse_group_entries(
    length: u8,
    data: &[u8],
) -> Result<Vec<(u16, u16, Vec<u8>)>, GattFailureKind> {
    let length = usize::from(length);
    if length < 4 {
        return Err(GattFailureKind::Protocol(format!(
            "Read By Group Type entry length {length} < 4"
        )));
    }
    if !data.len().is_multiple_of(length) {
        return Err(GattFailureKind::Protocol(
            "Read By Group Type data list is not a multiple of entry length"
                .into(),
        ));
    }
    Ok(data
        .chunks_exact(length)
        .map(|chunk| {
            (
                u16::from_le_bytes([chunk[0], chunk[1]]),
                u16::from_le_bytes([chunk[2], chunk[3]]),
                chunk[4..].to_vec(),
            )
        })
        .collect())
}

fn parse_type_entries(
    length: u8,
    data: &[u8],
) -> Result<Vec<(u16, Vec<u8>)>, GattFailureKind> {
    let length = usize::from(length);
    if length < 2 {
        return Err(GattFailureKind::Protocol(format!(
            "Read By Type entry length {length} < 2"
        )));
    }
    if !data.len().is_multiple_of(length) {
        return Err(GattFailureKind::Protocol(
            "Read By Type data list is not a multiple of entry length".into(),
        ));
    }
    Ok(data
        .chunks_exact(length)
        .map(|chunk| {
            (
                u16::from_le_bytes([chunk[0], chunk[1]]),
                chunk[2..].to_vec(),
            )
        })
        .collect())
}

fn parse_information_entries(
    format: u8,
    data: &[u8],
) -> Result<Vec<(u16, Uuid)>, GattFailureKind> {
    let uuid_size = match format {
        1 => 2usize,
        2 => 16usize,
        other => {
            return Err(GattFailureKind::Protocol(format!(
                "invalid Find Information format {other}"
            )))
        }
    };
    let entry = 2 + uuid_size;
    if !data.len().is_multiple_of(entry) {
        return Err(GattFailureKind::Protocol(
            "Find Information data is not a multiple of entry size".into(),
        ));
    }
    let mut output = Vec::new();
    for chunk in data.chunks_exact(entry) {
        let handle = u16::from_le_bytes([chunk[0], chunk[1]]);
        let uuid = Uuid::from_bytes(&chunk[2..]).map_err(|error| {
            GattFailureKind::Protocol(format!(
                "invalid descriptor UUID: {error}"
            ))
        })?;
        output.push((handle, uuid));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(handle: u16, generation: u64) -> ConnectionToken {
        ConnectionToken {
            connection_handle: handle,
            generation,
        }
    }

    fn request(id: u64, operation: GattOperation) -> GattRequest {
        GattRequest {
            request_id: id,
            connection: token(0x0800, 1),
            operation,
        }
    }

    fn drive_send(
        _transaction: &mut GattTransaction,
        action: GattAction,
    ) -> AttPdu {
        match action {
            GattAction::Send(pdu) => pdu,
            other => panic!("expected send, got {other:?}"),
        }
    }

    #[test]
    fn exchange_mtu_uses_smaller_peer_value_and_floor_23() {
        let (mut tx, first) = GattTransaction::begin(
            request(
                1,
                GattOperation::ExchangeMtu { client_rx_mtu: 247 },
            ),
            ATT_DEFAULT_MTU,
        )
        .unwrap();
        assert_eq!(
            first,
            AttPdu::ExchangeMtuRequest { client_rx_mtu: 247 }
        );
        assert!(matches!(tx.on_request_sent(), GattAction::Await));
        assert!(matches!(
            tx.on_pdu(AttPdu::ExchangeMtuResponse { server_rx_mtu: 185 }),
            GattAction::Complete(GattResult::Mtu { mtu: 185 })
        ));
    }

    #[test]
    fn service_discovery_paginates_until_attribute_not_found() {
        let (mut tx, first) = GattTransaction::begin(
            request(2, GattOperation::DiscoverServices),
            23,
        )
        .unwrap();
        assert_eq!(first, service_request(1));

        let next = tx.on_pdu(AttPdu::ReadByGroupTypeResponse {
            length: 6,
            attribute_data_list: vec![
                0x01, 0x00, 0x05, 0x00, 0x50, 0x18,
                0x06, 0x00, 0x09, 0x00, 0x4E, 0x18,
            ],
        });
        assert_eq!(
            drive_send(&mut tx, next),
            service_request(0x000A)
        );

        let done = tx.on_pdu(AttPdu::ErrorResponse {
            request_opcode_in_error: 0x10,
            attribute_handle_in_error: 0x000A,
            error_code: ATT_ATTRIBUTE_NOT_FOUND_ERROR,
        });
        let GattAction::Complete(GattResult::Services(services)) = done else {
            panic!("expected services");
        };
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].handle, 1);
        assert_eq!(services[1].end_group_handle, 9);
    }

    #[test]
    fn characteristic_discovery_updates_previous_end_handle() {
        let (mut tx, _) = GattTransaction::begin(
            request(
                3,
                GattOperation::DiscoverCharacteristics {
                    service_handle: 1,
                    end_group_handle: 10,
                },
            ),
            23,
        )
        .unwrap();

        let done = tx.on_pdu(AttPdu::ReadByTypeResponse {
            length: 7,
            attribute_data_list: vec![
                0x02, 0x00, 0x02, 0x03, 0x00, 0x00, 0x2A,
                0x06, 0x00, 0x10, 0x07, 0x00, 0x01, 0x2A,
            ],
        });
        let next = drive_send(&mut tx, done);
        assert_eq!(
            next,
            characteristic_request(7, 10)
        );

        let final_action = tx.on_pdu(AttPdu::ErrorResponse {
            request_opcode_in_error: 0x08,
            attribute_handle_in_error: 7,
            error_code: ATT_ATTRIBUTE_NOT_FOUND_ERROR,
        });
        let GattAction::Complete(GattResult::Characteristics(chars)) =
            final_action
        else {
            panic!("expected characteristics");
        };
        assert_eq!(chars.len(), 2);
        assert_eq!(chars[0].end_group_handle, 5);
        assert_eq!(chars[1].end_group_handle, 10);
    }

    #[test]
    fn descriptor_discovery_parses_16_bit_uuid_entries() {
        let (mut tx, _) = GattTransaction::begin(
            request(
                4,
                GattOperation::DiscoverDescriptors {
                    starting_handle: 4,
                    ending_handle: 6,
                },
            ),
            23,
        )
        .unwrap();
        let done = tx.on_pdu(AttPdu::FindInformationResponse {
            format: 1,
            information_data: vec![
                0x04, 0x00, 0x02, 0x29,
                0x05, 0x00, 0x01, 0x29,
                0x06, 0x00, 0x04, 0x29,
            ],
        });
        let GattAction::Complete(GattResult::Descriptors(desc)) = done else {
            panic!("expected descriptors");
        };
        assert_eq!(desc.len(), 3);
        assert_eq!(desc[0].handle, 4);
    }

    #[test]
    fn long_read_continues_until_short_blob() {
        let (mut tx, _) = GattTransaction::begin(
            request(
                5,
                GattOperation::ReadValue {
                    attribute_handle: 0x0042,
                },
            ),
            5 + ATT_DEFAULT_MTU,
        )
        .unwrap();
        let first = vec![0xAA; 27];
        let next = tx.on_pdu(AttPdu::ReadResponse {
            attribute_value: first,
        });
        assert_eq!(
            drive_send(&mut tx, next),
            AttPdu::ReadBlobRequest {
                attribute_handle: 0x0042,
                value_offset: 27,
            }
        );
        let done = tx.on_pdu(AttPdu::ReadBlobResponse {
            part_attribute_value: vec![0xBB; 3],
        });
        let GattAction::Complete(GattResult::Value { value, .. }) = done else {
            panic!("expected read value");
        };
        assert_eq!(value.len(), 30);
    }

    #[test]
    fn read_blob_not_long_error_finishes_accumulated_value() {
        let (mut tx, _) = GattTransaction::begin(
            request(
                6,
                GattOperation::ReadValue {
                    attribute_handle: 0x0042,
                },
            ),
            23,
        )
        .unwrap();
        let _ = tx.on_pdu(AttPdu::ReadResponse {
            attribute_value: vec![0xAA; 22],
        });
        let done = tx.on_pdu(AttPdu::ErrorResponse {
            request_opcode_in_error: 0x0C,
            attribute_handle_in_error: 0x0042,
            error_code: ATT_ATTRIBUTE_NOT_LONG_ERROR,
        });
        assert!(matches!(
            done,
            GattAction::Complete(GattResult::Value { .. })
        ));
    }

    #[test]
    fn att_error_is_typed_failure() {
        let (mut tx, _) = GattTransaction::begin(
            request(
                7,
                GattOperation::ReadValue {
                    attribute_handle: 0x1234,
                },
            ),
            23,
        )
        .unwrap();
        let failed = tx.on_pdu(AttPdu::ErrorResponse {
            request_opcode_in_error: 0x0A,
            attribute_handle_in_error: 0x1234,
            error_code: 0x05,
        });
        assert!(matches!(
            failed,
            GattAction::Failed(GattFailure {
                kind: GattFailureKind::Att {
                    error_code: 0x05,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn write_command_completes_after_successful_send_without_response() {
        let (mut tx, first) = GattTransaction::begin(
            request(
                8,
                GattOperation::WriteCommand {
                    attribute_handle: 0x0043,
                    value: vec![1, 2, 3],
                },
            ),
            23,
        )
        .unwrap();
        assert!(matches!(first, AttPdu::WriteCommand { .. }));
        assert!(matches!(
            tx.on_request_sent(),
            GattAction::Complete(GattResult::WriteComplete {
                with_response: false,
                ..
            })
        ));
    }

    #[test]
    fn subscription_writes_cccd_and_waits_for_write_response() {
        let (mut tx, first) = GattTransaction::begin(
            request(
                9,
                GattOperation::Subscribe {
                    value_handle: 0x0046,
                    cccd_handle: 0x0047,
                    indicate: true,
                },
            ),
            23,
        )
        .unwrap();
        assert_eq!(
            first,
            AttPdu::WriteRequest {
                attribute_handle: 0x0047,
                attribute_value: CCCD_INDICATION.to_le_bytes().to_vec(),
            }
        );
        assert!(matches!(
            tx.on_pdu(AttPdu::WriteResponse),
            GattAction::Complete(GattResult::Subscription {
                enabled: true,
                indicate: true,
                ..
            })
        ));
    }

    #[test]
    fn disconnect_and_cancel_are_terminal_failures() {
        let (mut tx, _) = GattTransaction::begin(
            request(10, GattOperation::DiscoverServices),
            23,
        )
        .unwrap();
        assert!(matches!(
            tx.on_disconnect(0x08),
            GattAction::Failed(GattFailure {
                kind: GattFailureKind::Disconnected { reason: 0x08 },
                ..
            })
        ));

        let (mut tx, _) = GattTransaction::begin(
            request(11, GattOperation::DiscoverServices),
            23,
        )
        .unwrap();
        assert!(matches!(
            tx.cancel(),
            GattAction::Failed(GattFailure {
                kind: GattFailureKind::Cancelled,
                ..
            })
        ));
    }

    #[test]
    fn invalid_ranges_fail_before_transport_io() {
        let error = GattTransaction::begin(
            request(
                12,
                GattOperation::DiscoverDescriptors {
                    starting_handle: 8,
                    ending_handle: 7,
                },
            ),
            23,
        )
        .unwrap_err();
        assert!(matches!(
            error.kind,
            GattFailureKind::InvalidRequest(_)
        ));
    }
}
