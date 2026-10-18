use std::collections::BTreeSet;

use bumble::Uuid;
use bumble_profiles::{
    ascs::{
        AseState, AseStatus, AseStatusDetails,
        ASE_CONTROL_POINT_CHARACTERISTIC, SINK_ASE_CHARACTERISTIC,
    },
    bap::AudioLocation,
    pacs::{
        AudioContexts, PacRecord, AVAILABLE_AUDIO_CONTEXTS_CHARACTERISTIC,
        SINK_AUDIO_LOCATION_CHARACTERISTIC, SINK_PAC_CHARACTERISTIC,
        SUPPORTED_AUDIO_CONTEXTS_CHARACTERISTIC,
    },
};

use crate::hci_host::{
    connection::ConnectionToken,
    gatt::{
        GattCharacteristic, GattFailure, GattOperation, GattReport,
        GattRequest, GattRequestId, GattResult, GattService,
    },
};

use super::audio_snapshot::{
    AscsSinkSnapshot, AudioMemberSnapshot, CasSnapshot,
    GattCharacteristicRange, GattServiceRange, PacsSinkSnapshot,
    SinkAseSnapshot,
};

pub const PACS_UUID: u16 = 0x1850;
pub const ASCS_UUID: u16 = 0x184E;
pub const CAS_UUID: u16 = 0x1853;
pub const CSIS_UUID: u16 = 0x1846;
pub const CONTROL_CLIENT_MTU: u16 = 247;
pub const MIN_CONTROL_MTU: u16 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDiscoveryPhase {
    Idle,
    ExchangeMtu,
    DiscoverServices,
    DiscoverPacsCharacteristics,
    ReadSinkPac,
    ReadSinkLocation,
    ReadAvailableContexts,
    ReadSupportedContexts,
    DiscoverAscsCharacteristics,
    ReadSinkAse { index: usize },
    DiscoverCasIncludes,
    Complete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioDiscoveryAction {
    BeginGatt(GattRequest),
    Complete(AudioMemberSnapshot),
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioDiscoveryFailure {
    InvalidStartState(AudioDiscoveryPhase),
    RequestIdOverflow,
    Gatt(GattFailure),
    BearerPoisoned {
        request_id: GattRequestId,
    },
    UnexpectedResult {
        phase: AudioDiscoveryPhase,
        result: GattResult,
    },
    MissingService {
        uuid: u16,
    },
    DuplicateService {
        uuid: u16,
    },
    MissingCharacteristic {
        uuid: u16,
    },
    DuplicateCharacteristic {
        uuid: u16,
    },
    MtuTooSmall {
        mtu: u16,
    },
    Pacs(String),
    AudioLocation(String),
    AudioContexts(String),
    AseStatus(String),
    InvalidAseSet(String),
    MissingCsisInclude,
    DuplicateCsisInclude,
}

pub struct AudioDiscoveryMachine {
    member_index: usize,
    connection: ConnectionToken,
    phase: AudioDiscoveryPhase,
    pending_request_id: Option<GattRequestId>,
    next_request_id: GattRequestId,

    mtu: Option<u16>,
    pacs_service: Option<GattServiceRange>,
    ascs_service: Option<GattServiceRange>,
    cas_service: Option<GattServiceRange>,

    sink_pac: Option<GattCharacteristic>,
    sink_location: Option<GattCharacteristic>,
    available_contexts: Option<GattCharacteristic>,
    supported_contexts: Option<GattCharacteristic>,

    sink_pac_records: Option<Vec<PacRecord>>,
    sink_audio_location: Option<AudioLocation>,
    available_audio_contexts: Option<AudioContexts>,
    supported_audio_contexts: Option<AudioContexts>,

    sink_ase_characteristics: Vec<GattCharacteristic>,
    sink_ases: Vec<SinkAseSnapshot>,
    control_point: Option<GattCharacteristic>,
}

impl AudioDiscoveryMachine {
    pub fn new(
        member_index: usize,
        connection: ConnectionToken,
        first_request_id: GattRequestId,
    ) -> Self {
        Self {
            member_index,
            connection,
            phase: AudioDiscoveryPhase::Idle,
            pending_request_id: None,
            next_request_id: first_request_id,
            mtu: None,
            pacs_service: None,
            ascs_service: None,
            cas_service: None,
            sink_pac: None,
            sink_location: None,
            available_contexts: None,
            supported_contexts: None,
            sink_pac_records: None,
            sink_audio_location: None,
            available_audio_contexts: None,
            supported_audio_contexts: None,
            sink_ase_characteristics: Vec::new(),
            sink_ases: Vec::new(),
            control_point: None,
        }
    }

    pub fn phase(&self) -> AudioDiscoveryPhase {
        self.phase
    }

    pub fn pending_request_id(&self) -> Option<GattRequestId> {
        self.pending_request_id
    }

    pub fn next_request_id(&self) -> GattRequestId {
        self.next_request_id
    }

    pub fn start(
        &mut self,
    ) -> Result<AudioDiscoveryAction, AudioDiscoveryFailure> {
        if self.phase != AudioDiscoveryPhase::Idle {
            return Err(AudioDiscoveryFailure::InvalidStartState(
                self.phase,
            ));
        }
        self.issue(
            AudioDiscoveryPhase::ExchangeMtu,
            GattOperation::ExchangeMtu {
                client_rx_mtu: CONTROL_CLIENT_MTU,
            },
        )
    }

    pub fn on_gatt_report(
        &mut self,
        report: &GattReport,
    ) -> Result<AudioDiscoveryAction, AudioDiscoveryFailure> {
        match report {
            GattReport::Complete {
                request_id,
                connection,
                result,
            } if Some(*request_id) == self.pending_request_id => {
                if *connection != self.connection {
                    return Ok(AudioDiscoveryAction::Ignored);
                }
                self.pending_request_id = None;
                self.on_result(result.clone())
            }
            GattReport::Failed(failure)
                if Some(failure.request_id) == self.pending_request_id
                    && failure.connection == self.connection =>
            {
                self.phase = AudioDiscoveryPhase::Failed;
                self.pending_request_id = None;
                Err(AudioDiscoveryFailure::Gatt(failure.clone()))
            }
            GattReport::BearerPoisoned {
                connection,
                request_id,
            } if Some(*request_id) == self.pending_request_id
                && *connection == self.connection =>
            {
                self.phase = AudioDiscoveryPhase::Failed;
                self.pending_request_id = None;
                Err(AudioDiscoveryFailure::BearerPoisoned {
                    request_id: *request_id,
                })
            }
            _ => Ok(AudioDiscoveryAction::Ignored),
        }
    }

    fn on_result(
        &mut self,
        result: GattResult,
    ) -> Result<AudioDiscoveryAction, AudioDiscoveryFailure> {
        match self.phase {
            AudioDiscoveryPhase::ExchangeMtu => {
                let GattResult::Mtu { mtu } = result else {
                    return self.unexpected(result);
                };
                if mtu < MIN_CONTROL_MTU {
                    return self.fail(AudioDiscoveryFailure::MtuTooSmall {
                        mtu,
                    });
                }
                self.mtu = Some(mtu);
                self.issue(
                    AudioDiscoveryPhase::DiscoverServices,
                    GattOperation::DiscoverServices,
                )
            }
            AudioDiscoveryPhase::DiscoverServices => {
                let GattResult::Services(services) = result else {
                    return self.unexpected(result);
                };
                self.pacs_service =
                    Some(required_service(&services, PACS_UUID)?);
                self.ascs_service =
                    Some(required_service(&services, ASCS_UUID)?);
                self.cas_service =
                    Some(required_service(&services, CAS_UUID)?);

                let pacs = self.pacs_service.expect("stored PACS service");
                self.issue(
                    AudioDiscoveryPhase::DiscoverPacsCharacteristics,
                    GattOperation::DiscoverCharacteristics {
                        service_handle: pacs.start_handle,
                        end_group_handle: pacs.end_handle,
                    },
                )
            }
            AudioDiscoveryPhase::DiscoverPacsCharacteristics => {
                let GattResult::Characteristics(values) = result else {
                    return self.unexpected(result);
                };
                self.sink_pac = Some(required_characteristic(
                    &values,
                    SINK_PAC_CHARACTERISTIC,
                )?);
                self.sink_location = Some(required_characteristic(
                    &values,
                    SINK_AUDIO_LOCATION_CHARACTERISTIC,
                )?);
                self.available_contexts = Some(required_characteristic(
                    &values,
                    AVAILABLE_AUDIO_CONTEXTS_CHARACTERISTIC,
                )?);
                self.supported_contexts = Some(required_characteristic(
                    &values,
                    SUPPORTED_AUDIO_CONTEXTS_CHARACTERISTIC,
                )?);

                let handle =
                    self.sink_pac.as_ref().expect("stored Sink PAC").handle;
                self.issue(
                    AudioDiscoveryPhase::ReadSinkPac,
                    GattOperation::ReadValue {
                        attribute_handle: handle,
                    },
                )
            }
            AudioDiscoveryPhase::ReadSinkPac => {
                let value = self.expect_value(
                    result,
                    self.sink_pac
                        .as_ref()
                        .expect("stored Sink PAC")
                        .handle,
                )?;
                let records = PacRecord::list_from_bytes(&value)
                    .map_err(|error| {
                        AudioDiscoveryFailure::Pacs(error.to_string())
                    })?;
                if records.is_empty() {
                    return self.fail(AudioDiscoveryFailure::Pacs(
                        "Sink PAC list is empty".into(),
                    ));
                }
                self.sink_pac_records = Some(records);

                let handle = self
                    .sink_location
                    .as_ref()
                    .expect("stored Sink Audio Location")
                    .handle;
                self.issue(
                    AudioDiscoveryPhase::ReadSinkLocation,
                    GattOperation::ReadValue {
                        attribute_handle: handle,
                    },
                )
            }
            AudioDiscoveryPhase::ReadSinkLocation => {
                let value = self.expect_value(
                    result,
                    self.sink_location
                        .as_ref()
                        .expect("stored Sink Audio Location")
                        .handle,
                )?;
                let bytes: [u8; 4] = value.try_into().map_err(
                    |value: Vec<u8>| {
                        AudioDiscoveryFailure::AudioLocation(format!(
                            "Sink Audio Location has {} bytes, expected 4",
                            value.len()
                        ))
                    },
                )?;
                self.sink_audio_location =
                    Some(AudioLocation(u32::from_le_bytes(bytes)));

                let handle = self
                    .available_contexts
                    .as_ref()
                    .expect("stored Available Audio Contexts")
                    .handle;
                self.issue(
                    AudioDiscoveryPhase::ReadAvailableContexts,
                    GattOperation::ReadValue {
                        attribute_handle: handle,
                    },
                )
            }
            AudioDiscoveryPhase::ReadAvailableContexts => {
                let value = self.expect_value(
                    result,
                    self.available_contexts
                        .as_ref()
                        .expect("stored Available Audio Contexts")
                        .handle,
                )?;
                self.available_audio_contexts = Some(
                    AudioContexts::decode(&value).map_err(|error| {
                        AudioDiscoveryFailure::AudioContexts(
                            error.to_string(),
                        )
                    })?,
                );

                let handle = self
                    .supported_contexts
                    .as_ref()
                    .expect("stored Supported Audio Contexts")
                    .handle;
                self.issue(
                    AudioDiscoveryPhase::ReadSupportedContexts,
                    GattOperation::ReadValue {
                        attribute_handle: handle,
                    },
                )
            }
            AudioDiscoveryPhase::ReadSupportedContexts => {
                let value = self.expect_value(
                    result,
                    self.supported_contexts
                        .as_ref()
                        .expect("stored Supported Audio Contexts")
                        .handle,
                )?;
                self.supported_audio_contexts = Some(
                    AudioContexts::decode(&value).map_err(|error| {
                        AudioDiscoveryFailure::AudioContexts(
                            error.to_string(),
                        )
                    })?,
                );

                let ascs = self.ascs_service.expect("stored ASCS service");
                self.issue(
                    AudioDiscoveryPhase::DiscoverAscsCharacteristics,
                    GattOperation::DiscoverCharacteristics {
                        service_handle: ascs.start_handle,
                        end_group_handle: ascs.end_handle,
                    },
                )
            }
            AudioDiscoveryPhase::DiscoverAscsCharacteristics => {
                let GattResult::Characteristics(values) = result else {
                    return self.unexpected(result);
                };
                self.control_point = Some(required_characteristic(
                    &values,
                    ASE_CONTROL_POINT_CHARACTERISTIC,
                )?);

                self.sink_ase_characteristics = values
                    .into_iter()
                    .filter(|value| {
                        value.uuid
                            == Uuid::from_16_bits(
                                SINK_ASE_CHARACTERISTIC,
                            )
                    })
                    .collect();
                self.sink_ase_characteristics
                    .sort_by_key(|value| value.handle);

                if self.sink_ase_characteristics.is_empty() {
                    return self.fail(
                        AudioDiscoveryFailure::MissingCharacteristic {
                            uuid: SINK_ASE_CHARACTERISTIC,
                        },
                    );
                }

                self.read_sink_ase(0)
            }
            AudioDiscoveryPhase::ReadSinkAse { index } => {
                let characteristic = self
                    .sink_ase_characteristics
                    .get(index)
                    .expect("ASE phase index must be valid")
                    .clone();
                let value =
                    self.expect_value(result, characteristic.handle)?;
                let status = parse_ase_status_compat(&value)?;

                self.sink_ases.push(SinkAseSnapshot {
                    declaration_handle:
                        characteristic.declaration_handle,
                    value_handle: characteristic.handle,
                    end_group_handle:
                        characteristic.end_group_handle,
                    properties: characteristic.properties,
                    status,
                });

                let next = index + 1;
                if next < self.sink_ase_characteristics.len() {
                    self.read_sink_ase(next)
                } else {
                    validate_ase_set(&self.sink_ases)?;
                    let cas =
                        self.cas_service.expect("stored CAS service");
                    self.issue(
                        AudioDiscoveryPhase::DiscoverCasIncludes,
                        GattOperation::DiscoverIncludedServices {
                            service_handle: cas.start_handle,
                            end_group_handle: cas.end_handle,
                        },
                    )
                }
            }
            AudioDiscoveryPhase::DiscoverCasIncludes => {
                let GattResult::IncludedServices(values) = result else {
                    return self.unexpected(result);
                };
                let csis = exactly_one_service(&values, CSIS_UUID)
                    .map_err(|error| match error {
                        ServiceLookupError::Missing => {
                            AudioDiscoveryFailure::MissingCsisInclude
                        }
                        ServiceLookupError::Duplicate => {
                            AudioDiscoveryFailure::DuplicateCsisInclude
                        }
                    })?;

                let snapshot = self.build_snapshot(csis)?;
                self.phase = AudioDiscoveryPhase::Complete;
                Ok(AudioDiscoveryAction::Complete(snapshot))
            }
            AudioDiscoveryPhase::Idle
            | AudioDiscoveryPhase::Complete
            | AudioDiscoveryPhase::Failed => {
                self.unexpected(result)
            }
        }
    }

    fn read_sink_ase(
        &mut self,
        index: usize,
    ) -> Result<AudioDiscoveryAction, AudioDiscoveryFailure> {
        let characteristic = self
            .sink_ase_characteristics
            .get(index)
            .expect("requested Sink ASE index must exist");
        self.issue(
            AudioDiscoveryPhase::ReadSinkAse { index },
            GattOperation::ReadValue {
                attribute_handle: characteristic.handle,
            },
        )
    }

    fn issue(
        &mut self,
        phase: AudioDiscoveryPhase,
        operation: GattOperation,
    ) -> Result<AudioDiscoveryAction, AudioDiscoveryFailure> {
        if self.pending_request_id.is_some() {
            return self.fail(AudioDiscoveryFailure::Pacs(
                "attempted to issue GATT while another request is pending"
                    .into(),
            ));
        }
        let request_id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(AudioDiscoveryFailure::RequestIdOverflow)?;
        self.phase = phase;
        self.pending_request_id = Some(request_id);
        Ok(AudioDiscoveryAction::BeginGatt(GattRequest {
            request_id,
            connection: self.connection,
            operation,
        }))
    }

    fn expect_value(
        &mut self,
        result: GattResult,
        expected_handle: u16,
    ) -> Result<Vec<u8>, AudioDiscoveryFailure> {
        match result {
            GattResult::Value {
                attribute_handle,
                value,
            } if attribute_handle == expected_handle => Ok(value),
            other => Err(AudioDiscoveryFailure::UnexpectedResult {
                phase: self.phase,
                result: other,
            }),
        }
    }

    fn build_snapshot(
        &self,
        included_csis: GattServiceRange,
    ) -> Result<AudioMemberSnapshot, AudioDiscoveryFailure> {
        Ok(AudioMemberSnapshot {
            member_index: self.member_index,
            connection: self.connection,
            mtu: self.mtu.expect("completed discovery missing MTU"),
            pacs: PacsSinkSnapshot {
                service: self
                    .pacs_service
                    .expect("completed discovery missing PACS"),
                sink_pac_handle: self
                    .sink_pac
                    .as_ref()
                    .expect("completed discovery missing Sink PAC")
                    .handle,
                sink_audio_location_handle: self
                    .sink_location
                    .as_ref()
                    .expect("completed discovery missing Sink Location")
                    .handle,
                available_contexts_handle: self
                    .available_contexts
                    .as_ref()
                    .expect("completed discovery missing Available Contexts")
                    .handle,
                supported_contexts_handle: self
                    .supported_contexts
                    .as_ref()
                    .expect("completed discovery missing Supported Contexts")
                    .handle,
                sink_pac_records: self
                    .sink_pac_records
                    .clone()
                    .expect("completed discovery missing PAC records"),
                sink_audio_location: self
                    .sink_audio_location
                    .expect("completed discovery missing audio location"),
                available_contexts: self
                    .available_audio_contexts
                    .expect("completed discovery missing available contexts"),
                supported_contexts: self
                    .supported_audio_contexts
                    .expect("completed discovery missing supported contexts"),
            },
            ascs: AscsSinkSnapshot {
                service: self
                    .ascs_service
                    .expect("completed discovery missing ASCS"),
                sink_ases: self.sink_ases.clone(),
                control_point: {
                    let value = self
                        .control_point
                        .as_ref()
                        .expect("completed discovery missing control point");
                    GattCharacteristicRange {
                        declaration_handle: value.declaration_handle,
                        value_handle: value.handle,
                        end_group_handle: value.end_group_handle,
                        properties: value.properties,
                    }
                },
            },
            cas: CasSnapshot {
                service: self
                    .cas_service
                    .expect("completed discovery missing CAS"),
                included_csis,
            },
        })
    }

    fn unexpected(
        &mut self,
        result: GattResult,
    ) -> Result<AudioDiscoveryAction, AudioDiscoveryFailure> {
        self.fail(AudioDiscoveryFailure::UnexpectedResult {
            phase: self.phase,
            result,
        })
    }

    fn fail<T>(
        &mut self,
        failure: AudioDiscoveryFailure,
    ) -> Result<T, AudioDiscoveryFailure> {
        self.phase = AudioDiscoveryPhase::Failed;
        self.pending_request_id = None;
        Err(failure)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceLookupError {
    Missing,
    Duplicate,
}

fn exactly_one_service(
    services: &[GattService],
    uuid: u16,
) -> Result<GattServiceRange, ServiceLookupError> {
    let wanted = Uuid::from_16_bits(uuid);
    let mut matches = services
        .iter()
        .filter(|service| service.uuid == wanted);
    let first = matches.next().ok_or(ServiceLookupError::Missing)?;
    if matches.next().is_some() {
        return Err(ServiceLookupError::Duplicate);
    }
    Ok(GattServiceRange {
        start_handle: first.handle,
        end_handle: first.end_group_handle,
    })
}

fn required_service(
    services: &[GattService],
    uuid: u16,
) -> Result<GattServiceRange, AudioDiscoveryFailure> {
    exactly_one_service(services, uuid).map_err(|error| match error {
        ServiceLookupError::Missing => {
            AudioDiscoveryFailure::MissingService { uuid }
        }
        ServiceLookupError::Duplicate => {
            AudioDiscoveryFailure::DuplicateService { uuid }
        }
    })
}

fn required_characteristic(
    values: &[GattCharacteristic],
    uuid: u16,
) -> Result<GattCharacteristic, AudioDiscoveryFailure> {
    let wanted = Uuid::from_16_bits(uuid);
    let mut matches = values.iter().filter(|value| value.uuid == wanted);
    let first = matches
        .next()
        .ok_or(AudioDiscoveryFailure::MissingCharacteristic { uuid })?;
    if matches.next().is_some() {
        return Err(AudioDiscoveryFailure::DuplicateCharacteristic {
            uuid,
        });
    }
    Ok(first.clone())
}

fn parse_ase_status_compat(
    value: &[u8],
) -> Result<AseStatus, AudioDiscoveryFailure> {
    if value.len() >= 2 && value[1] == AseState::IDLE.0 {
        let ase_id = value[0];
        if ase_id == 0 {
            return Err(AudioDiscoveryFailure::AseStatus(
                "ASE ID zero is invalid".into(),
            ));
        }
        return Ok(AseStatus {
            ase_id,
            state: AseState::IDLE,
            details: AseStatusDetails::None,
        });
    }

    AseStatus::from_bytes(value).map_err(|error| {
        AudioDiscoveryFailure::AseStatus(error.to_string())
    })
}

fn validate_ase_set(
    values: &[SinkAseSnapshot],
) -> Result<(), AudioDiscoveryFailure> {
    let mut ids = BTreeSet::new();
    for value in values {
        if value.status.ase_id == 0 || !ids.insert(value.status.ase_id) {
            return Err(AudioDiscoveryFailure::InvalidAseSet(
                format!(
                    "invalid or duplicate Sink ASE ID {}",
                    value.status.ase_id
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bumble_hci::CodingFormat;
    use bumble_profiles::{
        bap::{
            CodecSpecificCapabilities, ContextType,
            SupportedFrameDuration, SupportedSamplingFrequency,
        },
        le_audio::Metadata,
        pacs::PacCodecCapabilities,
    };

    use super::*;

    fn connection() -> ConnectionToken {
        ConnectionToken {
            connection_handle: 0x0800,
            generation: 1,
        }
    }

    fn service(handle: u16, end: u16, uuid: u16) -> GattService {
        GattService {
            handle,
            end_group_handle: end,
            uuid: Uuid::from_16_bits(uuid),
        }
    }

    fn characteristic(
        declaration: u16,
        handle: u16,
        end: u16,
        uuid: u16,
    ) -> GattCharacteristic {
        GattCharacteristic {
            declaration_handle: declaration,
            handle,
            end_group_handle: end,
            properties: 0x12,
            uuid: Uuid::from_16_bits(uuid),
        }
    }

    fn pac_bytes() -> Vec<u8> {
        PacRecord::list_to_bytes(&[PacRecord {
            coding_format: CodingFormat::LC3,
            codec_specific_capabilities:
                PacCodecCapabilities::Standard(
                    CodecSpecificCapabilities {
                        supported_sampling_frequencies:
                            SupportedSamplingFrequency::FREQ_48000,
                        supported_frame_durations:
                            SupportedFrameDuration::
                                DURATION_10000_US_SUPPORTED,
                        supported_audio_channel_count: vec![1],
                        min_octets_per_codec_frame: 26,
                        max_octets_per_codec_frame: 155,
                        supported_max_codec_frames_per_sdu: 1,
                    },
                ),
            metadata: Metadata::default(),
        }])
        .unwrap()
    }

    fn begin_request(
        action: AudioDiscoveryAction,
    ) -> GattRequest {
        let AudioDiscoveryAction::BeginGatt(request) = action else {
            panic!("expected GATT request");
        };
        request
    }

    fn complete(
        machine: &mut AudioDiscoveryMachine,
        request: GattRequest,
        result: GattResult,
    ) -> AudioDiscoveryAction {
        machine
            .on_gatt_report(&GattReport::Complete {
                request_id: request.request_id,
                connection: request.connection,
                result,
            })
            .unwrap()
    }

    #[test]
    fn member_discovery_builds_sink_snapshot_with_two_idle_ases() {
        let mut machine =
            AudioDiscoveryMachine::new(0, connection(), 10);

        let request = begin_request(machine.start().unwrap());
        assert!(matches!(
            request.operation,
            GattOperation::ExchangeMtu {
                client_rx_mtu: CONTROL_CLIENT_MTU
            }
        ));

        let request =
            begin_request(complete(&mut machine, request, GattResult::Mtu {
                mtu: 247,
            }));
        assert!(matches!(
            request.operation,
            GattOperation::DiscoverServices
        ));

        let services = vec![
            service(0x20, 0x35, PACS_UUID),
            service(0x40, 0x4D, ASCS_UUID),
            service(0x50, 0x55, CAS_UUID),
        ];
        let request = begin_request(complete(
            &mut machine,
            request,
            GattResult::Services(services),
        ));

        let pacs_chars = vec![
            characteristic(
                0x21,
                0x22,
                0x22,
                SINK_PAC_CHARACTERISTIC,
            ),
            characteristic(
                0x23,
                0x24,
                0x24,
                SINK_AUDIO_LOCATION_CHARACTERISTIC,
            ),
            characteristic(
                0x25,
                0x26,
                0x26,
                AVAILABLE_AUDIO_CONTEXTS_CHARACTERISTIC,
            ),
            characteristic(
                0x27,
                0x28,
                0x28,
                SUPPORTED_AUDIO_CONTEXTS_CHARACTERISTIC,
            ),
        ];
        let request = begin_request(complete(
            &mut machine,
            request,
            GattResult::Characteristics(pacs_chars),
        ));

        let request = begin_request(complete(
            &mut machine,
            request.clone(),
            GattResult::Value {
                attribute_handle:
                    request.operation.operation_handle().unwrap_or(0),
                value: pac_bytes(),
            },
        ));

        let sink_location_handle = match request.operation {
            GattOperation::ReadValue { attribute_handle } => {
                attribute_handle
            }
            _ => panic!("expected Sink Audio Location read"),
        };
        let request = begin_request(complete(
            &mut machine,
            request,
            GattResult::Value {
                attribute_handle: sink_location_handle,
                value: AudioLocation::FRONT_LEFT.0
                    .to_le_bytes()
                    .to_vec(),
            },
        ));

        let available_handle = match request.operation {
            GattOperation::ReadValue { attribute_handle } => {
                attribute_handle
            }
            _ => panic!("expected Available Contexts read"),
        };
        let contexts = AudioContexts {
            sink: ContextType::MEDIA,
            source: ContextType::PROHIBITED,
        };
        let request = begin_request(complete(
            &mut machine,
            request,
            GattResult::Value {
                attribute_handle: available_handle,
                value: contexts.encode().to_vec(),
            },
        ));

        let supported_handle = match request.operation {
            GattOperation::ReadValue { attribute_handle } => {
                attribute_handle
            }
            _ => panic!("expected Supported Contexts read"),
        };
        let request = begin_request(complete(
            &mut machine,
            request,
            GattResult::Value {
                attribute_handle: supported_handle,
                value: contexts.encode().to_vec(),
            },
        ));

        let ascs_chars = vec![
            characteristic(
                0x41,
                0x42,
                0x44,
                ASE_CONTROL_POINT_CHARACTERISTIC,
            ),
            characteristic(
                0x45,
                0x46,
                0x48,
                SINK_ASE_CHARACTERISTIC,
            ),
            characteristic(
                0x49,
                0x4A,
                0x4C,
                SINK_ASE_CHARACTERISTIC,
            ),
        ];
        let request = begin_request(complete(
            &mut machine,
            request,
            GattResult::Characteristics(ascs_chars),
        ));

        let first_ase_handle = match request.operation {
            GattOperation::ReadValue { attribute_handle } => {
                attribute_handle
            }
            _ => panic!("expected first ASE read"),
        };
        let request = begin_request(complete(
            &mut machine,
            request,
            GattResult::Value {
                attribute_handle: first_ase_handle,
                // Buds3 Pro Idle quirk: semantic header plus trailing bytes.
                value: vec![1, 0, 0, 0, 0],
            },
        ));

        let second_ase_handle = match request.operation {
            GattOperation::ReadValue { attribute_handle } => {
                attribute_handle
            }
            _ => panic!("expected second ASE read"),
        };
        let request = begin_request(complete(
            &mut machine,
            request,
            GattResult::Value {
                attribute_handle: second_ase_handle,
                value: vec![2, 0],
            },
        ));

        let action = complete(
            &mut machine,
            request,
            GattResult::IncludedServices(vec![
                service(0x56, 0x5A, CSIS_UUID),
            ]),
        );

        let AudioDiscoveryAction::Complete(snapshot) = action else {
            panic!("expected completed audio member snapshot");
        };
        assert_eq!(machine.phase(), AudioDiscoveryPhase::Complete);
        assert_eq!(snapshot.member_index, 0);
        assert_eq!(snapshot.mtu, 247);
        assert_eq!(
            snapshot.pacs.sink_audio_location,
            AudioLocation::FRONT_LEFT
        );
        assert_eq!(snapshot.pacs.sink_pac_records.len(), 1);
        assert_eq!(snapshot.ascs.sink_ases.len(), 2);
        assert_eq!(snapshot.ascs.sink_ases[0].status.ase_id, 1);
        assert_eq!(snapshot.ascs.sink_ases[1].status.ase_id, 2);
        assert_eq!(
            snapshot.ascs.control_point.value_handle,
            0x42
        );
        assert_eq!(
            snapshot.cas.included_csis,
            GattServiceRange {
                start_handle: 0x56,
                end_handle: 0x5A,
            }
        );
        assert_eq!(machine.next_request_id(), 21);
    }

    #[test]
    fn idle_status_tolerates_trailing_bytes_but_non_idle_remains_strict() {
        assert_eq!(
            parse_ase_status_compat(&[3, 0, 0xAA, 0xBB])
                .unwrap(),
            AseStatus {
                ase_id: 3,
                state: AseState::IDLE,
                details: AseStatusDetails::None,
            }
        );

        assert!(parse_ase_status_compat(&[
            3,
            AseState::CODEC_CONFIGURED.0,
        ])
        .is_err());
    }
}

#[cfg(test)]
trait TestGattOperationHandle {
    fn operation_handle(&self) -> Option<u16>;
}

#[cfg(test)]
impl TestGattOperationHandle for GattOperation {
    fn operation_handle(&self) -> Option<u16> {
        match self {
            GattOperation::ReadValue { attribute_handle } => {
                Some(*attribute_handle)
            }
            _ => None,
        }
    }
}
