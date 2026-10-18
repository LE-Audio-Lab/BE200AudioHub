use bumble_profiles::{
    ascs::{AseState, AseStatus, AseStatusDetails},
    bap::ContextType,
    le_audio::MetadataTag,
};

use crate::{
    plane::l1_hci::{
        gatt::transaction::{
        GattFailure, GattOperation, GattReport, GattRequest,
        GattRequestId, GattResult, GattValueEvent,
        },
        iso::cig_cis::CisRef,
    },
};

use super::{
    ascs_control::PreparedAseControl,
    audio_discovery::parse_ase_status_compat,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamingGatePhase {
    Idle,
    Reading,
    WaitingRetry,
    Complete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamingGateAction {
    BeginGatt(GattRequest),
    RetryLater,
    Complete(AseStatus),
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamingGateFailure {
    InvalidStartState(StreamingGatePhase),
    RequestIdOverflow,
    Gatt(GattFailure),
    BearerPoisoned {
        request_id: GattRequestId,
    },
    UnexpectedResult {
        phase: StreamingGatePhase,
        result: GattResult,
    },
    AseStatusParse(String),
    AseIdentityMismatch {
        expected: u8,
        actual: u8,
    },
    UnexpectedAseState {
        ase_id: u8,
        state: u8,
    },
    StreamingReadbackMismatch {
        detail: String,
    },
}

pub struct StreamingGateMachine {
    prepared: PreparedAseControl,
    cis: CisRef,
    context: ContextType,
    phase: StreamingGatePhase,
    pending_request_id: Option<GattRequestId>,
    next_request_id: GattRequestId,
    streaming_notified: bool,
}

impl StreamingGateMachine {
    pub fn new(
        prepared: PreparedAseControl,
        cis: CisRef,
        context: ContextType,
        first_request_id: GattRequestId,
    ) -> Self {
        Self {
            prepared,
            cis,
            context,
            phase: StreamingGatePhase::Idle,
            pending_request_id: None,
            next_request_id: first_request_id,
            streaming_notified: false,
        }
    }

    pub fn phase(&self) -> StreamingGatePhase {
        self.phase
    }

    pub fn pending_request_id(&self) -> Option<GattRequestId> {
        self.pending_request_id
    }

    pub fn next_request_id(&self) -> GattRequestId {
        self.next_request_id
    }

    pub fn cis(&self) -> CisRef {
        self.cis
    }

    pub fn start(
        &mut self,
    ) -> Result<StreamingGateAction, StreamingGateFailure> {
        if self.phase != StreamingGatePhase::Idle {
            return Err(StreamingGateFailure::InvalidStartState(
                self.phase,
            ));
        }
        self.issue_read()
    }

    pub fn retry(
        &mut self,
    ) -> Result<StreamingGateAction, StreamingGateFailure> {
        if self.phase != StreamingGatePhase::WaitingRetry {
            return Err(StreamingGateFailure::InvalidStartState(
                self.phase,
            ));
        }
        self.issue_read()
    }

    pub fn on_gatt_report(
        &mut self,
        report: &GattReport,
    ) -> Result<StreamingGateAction, StreamingGateFailure> {
        match report {
            GattReport::Value(event) => self.on_value_event(event),
            GattReport::Complete {
                request_id,
                connection,
                result,
            } if Some(*request_id) == self.pending_request_id
                && *connection == self.prepared.sink.connection =>
            {
                self.pending_request_id = None;
                self.on_result(result.clone())
            }
            GattReport::Failed(failure)
                if Some(failure.request_id) == self.pending_request_id
                    && failure.connection
                        == self.prepared.sink.connection =>
            {
                self.phase = StreamingGatePhase::Failed;
                self.pending_request_id = None;
                Err(StreamingGateFailure::Gatt(failure.clone()))
            }
            GattReport::BearerPoisoned {
                connection,
                request_id,
            } if Some(*request_id) == self.pending_request_id
                && *connection == self.prepared.sink.connection =>
            {
                self.phase = StreamingGatePhase::Failed;
                self.pending_request_id = None;
                Err(StreamingGateFailure::BearerPoisoned {
                    request_id: *request_id,
                })
            }
            _ => Ok(StreamingGateAction::Ignored),
        }
    }

    fn on_value_event(
        &mut self,
        event: &GattValueEvent,
    ) -> Result<StreamingGateAction, StreamingGateFailure> {
        let (connection, attribute_handle, value) = match event {
            GattValueEvent::Notification {
                connection,
                attribute_handle,
                value,
            }
            | GattValueEvent::Indication {
                connection,
                attribute_handle,
                value,
            } => (connection, attribute_handle, value),
        };

        if *connection != self.prepared.sink.connection
            || *attribute_handle != self.prepared.sink.ase_value_handle
            || matches!(
                self.phase,
                StreamingGatePhase::Idle
                    | StreamingGatePhase::Complete
                    | StreamingGatePhase::Failed
            )
        {
            return Ok(StreamingGateAction::Ignored);
        }

        let status = self.parse_status(value)?;
        match status.state {
            AseState::STREAMING => {
                self.verify_streaming(&status)?;
                self.streaming_notified = true;
                if self.phase == StreamingGatePhase::WaitingRetry {
                    self.issue_read()
                } else {
                    Ok(StreamingGateAction::Ignored)
                }
            }
            AseState::ENABLING => Ok(StreamingGateAction::Ignored),
            other => self.fail(
                StreamingGateFailure::UnexpectedAseState {
                    ase_id: status.ase_id,
                    state: other.0,
                },
            ),
        }
    }

    fn on_result(
        &mut self,
        result: GattResult,
    ) -> Result<StreamingGateAction, StreamingGateFailure> {
        let GattResult::Value {
            attribute_handle,
            value,
        } = result.clone()
        else {
            return self.fail(
                StreamingGateFailure::UnexpectedResult {
                    phase: self.phase,
                    result,
                },
            );
        };
        if attribute_handle != self.prepared.sink.ase_value_handle {
            return self.fail(
                StreamingGateFailure::UnexpectedResult {
                    phase: self.phase,
                    result,
                },
            );
        }

        let status = self.parse_status(&value)?;
        match status.state {
            AseState::STREAMING => {
                self.verify_streaming(&status)?;
                self.phase = StreamingGatePhase::Complete;
                Ok(StreamingGateAction::Complete(status))
            }
            AseState::ENABLING => {
                if self.streaming_notified {
                    self.streaming_notified = false;
                    self.issue_read()
                } else {
                    self.phase = StreamingGatePhase::WaitingRetry;
                    Ok(StreamingGateAction::RetryLater)
                }
            }
            other => self.fail(
                StreamingGateFailure::UnexpectedAseState {
                    ase_id: status.ase_id,
                    state: other.0,
                },
            ),
        }
    }

    fn parse_status(
        &mut self,
        value: &[u8],
    ) -> Result<AseStatus, StreamingGateFailure> {
        let status = parse_ase_status_compat(value).map_err(|error| {
            StreamingGateFailure::AseStatusParse(format!(
                "{error:?}"
            ))
        })?;
        if status.ase_id != self.prepared.sink.ase_id {
            return self.fail(
                StreamingGateFailure::AseIdentityMismatch {
                    expected: self.prepared.sink.ase_id,
                    actual: status.ase_id,
                },
            );
        }
        Ok(status)
    }

    fn verify_streaming(
        &mut self,
        status: &AseStatus,
    ) -> Result<(), StreamingGateFailure> {
        match &status.details {
            AseStatusDetails::Metadata {
                cig_id,
                cis_id,
                metadata,
            } if *cig_id == self.cis.cig_id
                && *cis_id == self.cis.cis_id
                && metadata.entries.iter().any(|entry| {
                    entry.tag == MetadataTag::STREAMING_AUDIO_CONTEXTS
                        && entry.data == self.context.0.to_le_bytes()
                }) =>
            {
                Ok(())
            }
            other => self.fail(
                StreamingGateFailure::StreamingReadbackMismatch {
                    detail: format!("{other:?}"),
                },
            ),
        }
    }

    fn issue_read(
        &mut self,
    ) -> Result<StreamingGateAction, StreamingGateFailure> {
        let request_id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(StreamingGateFailure::RequestIdOverflow)?;
        self.pending_request_id = Some(request_id);
        self.phase = StreamingGatePhase::Reading;
        Ok(StreamingGateAction::BeginGatt(GattRequest {
            request_id,
            connection: self.prepared.sink.connection,
            operation: GattOperation::ReadValue {
                attribute_handle:
                    self.prepared.sink.ase_value_handle,
            },
        }))
    }

    fn fail<T>(
        &mut self,
        failure: StreamingGateFailure,
    ) -> Result<T, StreamingGateFailure> {
        self.phase = StreamingGatePhase::Failed;
        self.pending_request_id = None;
        Err(failure)
    }
}

#[cfg(test)]
mod tests {
    use bumble_hci::CodingFormat;
    use bumble_profiles::{
        ascs::{AseEndpoint, AudioRole},
        bap::{
            AudioLocation, CodecSpecificConfiguration,
            ContextType, FrameDuration, SamplingFrequency,
        },
        le_audio::{Metadata, MetadataEntry, MetadataTag},
    };

    use crate::{
        architecture::types::AudioSide,
        control::{
            audio_snapshot::GattCharacteristicRange,
            codec_plan::PlannedSink,
        },
        plane::l1_hci::{
            acl::connection::ConnectionToken,
            gatt::transaction::{GattResult, GattValueEvent},
        },
    };

    use super::*;

    fn prepared() -> PreparedAseControl {
        PreparedAseControl {
            sink: PlannedSink {
                side: AudioSide::Left,
                member_index: 0,
                connection: ConnectionToken {
                    connection_handle: 0x0800,
                    generation: 1,
                },
                ase_id: 1,
                ase_value_handle: 0x0046,
                ase_end_group_handle: 0x0048,
                ascs_control_point: GattCharacteristicRange {
                    declaration_handle: 0x0042,
                    value_handle: 0x0043,
                    end_group_handle: 0x0045,
                    properties: 0x12,
                },
                audio_location: AudioLocation::FRONT_LEFT,
                codec_id: CodingFormat::LC3,
                codec_specific_configuration:
                    CodecSpecificConfiguration {
                        sampling_frequency: Some(
                            SamplingFrequency::FREQ_48000,
                        ),
                        frame_duration: Some(
                            FrameDuration::DURATION_10000_US,
                        ),
                        audio_channel_allocation: Some(
                            AudioLocation::FRONT_LEFT,
                        ),
                        octets_per_codec_frame: Some(100),
                        codec_frames_per_sdu: Some(1),
                    },
            },
            ase_cccd_handle: 0x0047,
            control_point_cccd_handle: 0x0044,
        }
    }

    fn status_bytes(state: AseState) -> Vec<u8> {
        let mut endpoint = AseEndpoint::new(1, AudioRole::Sink);
        endpoint.state = state;
        endpoint.qos.cig_id = 1;
        endpoint.qos.cis_id = 1;
        endpoint.metadata = Metadata::new(vec![MetadataEntry::new(
            MetadataTag::STREAMING_AUDIO_CONTEXTS,
            ContextType::MEDIA.0.to_le_bytes(),
        )]);
        endpoint.value().unwrap()
    }

    #[test]
    fn enabling_read_retries_until_streaming_readback() {
        let prepared = prepared();
        let connection = prepared.sink.connection;
        let mut machine = StreamingGateMachine::new(
            prepared,
            CisRef {
                cig_id: 1,
                cis_id: 1,
            },
            ContextType::MEDIA,
            50,
        );

        let StreamingGateAction::BeginGatt(first) =
            machine.start().unwrap()
        else {
            panic!("expected initial read");
        };
        assert_eq!(first.request_id, 50);

        assert_eq!(
            machine
                .on_gatt_report(&GattReport::Complete {
                    request_id: 50,
                    connection,
                    result: GattResult::Value {
                        attribute_handle: 0x0046,
                        value: status_bytes(AseState::ENABLING),
                    },
                })
                .unwrap(),
            StreamingGateAction::RetryLater
        );
        assert_eq!(
            machine.phase(),
            StreamingGatePhase::WaitingRetry
        );

        let StreamingGateAction::BeginGatt(second) =
            machine.retry().unwrap()
        else {
            panic!("expected retry read");
        };
        assert_eq!(second.request_id, 51);

        let action = machine
            .on_gatt_report(&GattReport::Complete {
                request_id: 51,
                connection,
                result: GattResult::Value {
                    attribute_handle: 0x0046,
                    value: status_bytes(AseState::STREAMING),
                },
            })
            .unwrap();
        assert!(matches!(
            action,
            StreamingGateAction::Complete(AseStatus {
                state: AseState::STREAMING,
                ..
            })
        ));
        assert_eq!(machine.phase(), StreamingGatePhase::Complete);
    }

    #[test]
    fn streaming_notification_shortcuts_wait_but_requires_readback() {
        let prepared = prepared();
        let connection = prepared.sink.connection;
        let mut machine = StreamingGateMachine::new(
            prepared,
            CisRef {
                cig_id: 1,
                cis_id: 1,
            },
            ContextType::MEDIA,
            70,
        );

        let StreamingGateAction::BeginGatt(first) =
            machine.start().unwrap()
        else {
            panic!("expected first read");
        };
        machine
            .on_gatt_report(&GattReport::Complete {
                request_id: first.request_id,
                connection,
                result: GattResult::Value {
                    attribute_handle: 0x0046,
                    value: status_bytes(AseState::ENABLING),
                },
            })
            .unwrap();

        let StreamingGateAction::BeginGatt(confirm) = machine
            .on_gatt_report(&GattReport::Value(
                GattValueEvent::Notification {
                    connection,
                    attribute_handle: 0x0046,
                    value: status_bytes(AseState::STREAMING),
                },
            ))
            .unwrap()
        else {
            panic!("STREAMING notification must trigger readback");
        };
        assert_eq!(confirm.request_id, 71);
        assert_eq!(machine.phase(), StreamingGatePhase::Reading);

        assert!(matches!(
            machine
                .on_gatt_report(&GattReport::Complete {
                    request_id: confirm.request_id,
                    connection,
                    result: GattResult::Value {
                        attribute_handle: 0x0046,
                        value: status_bytes(AseState::STREAMING),
                    },
                })
                .unwrap(),
            StreamingGateAction::Complete(_)
        ));
    }

    #[test]
    fn streaming_requires_expected_cis_and_media_context() {
        let prepared = prepared();
        let connection = prepared.sink.connection;
        let mut machine = StreamingGateMachine::new(
            prepared,
            CisRef {
                cig_id: 1,
                cis_id: 2,
            },
            ContextType::MEDIA,
            90,
        );
        let StreamingGateAction::BeginGatt(read) =
            machine.start().unwrap()
        else {
            panic!("expected read");
        };

        assert!(matches!(
            machine.on_gatt_report(&GattReport::Complete {
                request_id: read.request_id,
                connection,
                result: GattResult::Value {
                    attribute_handle: 0x0046,
                    value: status_bytes(AseState::STREAMING),
                },
            }),
            Err(StreamingGateFailure::StreamingReadbackMismatch { .. })
        ));
        assert_eq!(machine.phase(), StreamingGatePhase::Failed);
    }
}
