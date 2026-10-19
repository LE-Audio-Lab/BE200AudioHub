use bumble::Uuid;
use bumble_profiles::ascs::{
    AseControlResponse, AseOperation, AseResponseCode, AseState,
    AseStatus,
};

use crate::plane::l1_hci::gatt::transaction::{
        GattFailure, GattOperation, GattReport, GattRequest,
        GattRequestId, GattResult, GattValueEvent,

};

use crate::plane::l1_control::{
    discovery::audio::parse_ase_status_compat,
    planning::codec::PlannedSink,
};

pub const CCCD_UUID: u16 = 0x2902;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedAseControl {
    pub sink: PlannedSink,
    pub ase_cccd_handle: u16,
    pub control_point_cccd_handle: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsePreparePhase {
    Idle,
    DiscoverAseDescriptors,
    DiscoverControlPointDescriptors,
    SubscribeAse,
    SubscribeControlPoint,
    Complete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AsePrepareAction {
    BeginGatt(GattRequest),
    Complete(PreparedAseControl),
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AsePrepareFailure {
    InvalidStartState(AsePreparePhase),
    RequestIdOverflow,
    Gatt(GattFailure),
    BearerPoisoned {
        request_id: GattRequestId,
    },
    MissingDescriptorRange {
        label: &'static str,
    },
    MissingCccd {
        label: &'static str,
    },
    DuplicateCccd {
        label: &'static str,
    },
    UnexpectedResult {
        phase: AsePreparePhase,
        result: GattResult,
    },
}

pub struct AsePrepareMachine {
    sink: PlannedSink,
    phase: AsePreparePhase,
    pending_request_id: Option<GattRequestId>,
    next_request_id: GattRequestId,
    ase_cccd_handle: Option<u16>,
    control_point_cccd_handle: Option<u16>,
}

impl AsePrepareMachine {
    pub fn new(
        sink: PlannedSink,
        first_request_id: GattRequestId,
    ) -> Self {
        Self {
            sink,
            phase: AsePreparePhase::Idle,
            pending_request_id: None,
            next_request_id: first_request_id,
            ase_cccd_handle: None,
            control_point_cccd_handle: None,
        }
    }

    pub fn phase(&self) -> AsePreparePhase {
        self.phase
    }

    pub fn pending_request_id(&self) -> Option<GattRequestId> {
        self.pending_request_id
    }

    pub fn gate_snapshot(&self) -> AseOperationGateSnapshot {
        AseOperationGateSnapshot {
            phase: self.phase,
            pending_request_id: self.pending_request_id,
            write_complete: self.write_complete,
            cp_success: self.cp_success,
            notified_state: self
                .notified_status
                .as_ref()
                .map(|status| status.state),
        }
    }

    pub fn next_request_id(&self) -> GattRequestId {
        self.next_request_id
    }

    pub fn start(
        &mut self,
    ) -> Result<AsePrepareAction, AsePrepareFailure> {
        if self.phase != AsePreparePhase::Idle {
            return Err(AsePrepareFailure::InvalidStartState(
                self.phase,
            ));
        }

        let start = self.sink.ase_value_handle.saturating_add(1);
        let ase_end = self.sink.ase_end_group_handle;
        if start > ase_end {
            return self.fail(
                AsePrepareFailure::MissingDescriptorRange {
                    label: "Sink ASE",
                },
            );
        }

        self.issue(
            AsePreparePhase::DiscoverAseDescriptors,
            GattOperation::DiscoverDescriptors {
                starting_handle: start,
                ending_handle: ase_end,
            },
        )
    }

    pub fn on_gatt_report(
        &mut self,
        report: &GattReport,
    ) -> Result<AsePrepareAction, AsePrepareFailure> {
        match report {
            GattReport::Complete {
                request_id,
                connection,
                result,
            } if Some(*request_id) == self.pending_request_id => {
                if *connection != self.sink.connection {
                    return Ok(AsePrepareAction::Ignored);
                }
                self.pending_request_id = None;
                self.on_result(result.clone())
            }
            GattReport::Failed(failure)
                if Some(failure.request_id) == self.pending_request_id
                    && failure.connection == self.sink.connection =>
            {
                self.phase = AsePreparePhase::Failed;
                self.pending_request_id = None;
                Err(AsePrepareFailure::Gatt(failure.clone()))
            }
            GattReport::BearerPoisoned {
                connection,
                request_id,
            } if Some(*request_id) == self.pending_request_id
                && *connection == self.sink.connection =>
            {
                self.phase = AsePreparePhase::Failed;
                self.pending_request_id = None;
                Err(AsePrepareFailure::BearerPoisoned {
                    request_id: *request_id,
                })
            }
            _ => Ok(AsePrepareAction::Ignored),
        }
    }

    fn on_result(
        &mut self,
        result: GattResult,
    ) -> Result<AsePrepareAction, AsePrepareFailure> {
        match self.phase {
            AsePreparePhase::DiscoverAseDescriptors => {
                let GattResult::Descriptors(values) = result else {
                    return self.unexpected(result);
                };
                self.ase_cccd_handle =
                    Some(exactly_one_cccd(&values, "Sink ASE")?);

                let start = self
                    .sink
                    .ascs_control_point
                    .value_handle
                    .saturating_add(1);
                let end = self.sink.ascs_control_point.end_group_handle;
                if start > end {
                    return self.fail(
                        AsePrepareFailure::MissingDescriptorRange {
                            label: "ASCS Control Point",
                        },
                    );
                }

                self.issue(
                    AsePreparePhase::DiscoverControlPointDescriptors,
                    GattOperation::DiscoverDescriptors {
                        starting_handle: start,
                        ending_handle: end,
                    },
                )
            }
            AsePreparePhase::DiscoverControlPointDescriptors => {
                let GattResult::Descriptors(values) = result else {
                    return self.unexpected(result);
                };
                self.control_point_cccd_handle = Some(
                    exactly_one_cccd(&values, "ASCS Control Point")?,
                );

                self.issue(
                    AsePreparePhase::SubscribeAse,
                    GattOperation::Subscribe {
                        value_handle: self.sink.ase_value_handle,
                        cccd_handle: self
                            .ase_cccd_handle
                            .expect("stored Sink ASE CCCD"),
                        indicate: false,
                    },
                )
            }
            AsePreparePhase::SubscribeAse => {
                let GattResult::Subscription {
                    value_handle,
                    cccd_handle,
                    indicate,
                    enabled,
                } = result.clone()
                else {
                    return self.unexpected(result);
                };
                if value_handle != self.sink.ase_value_handle
                    || cccd_handle
                        != self
                            .ase_cccd_handle
                            .expect("stored Sink ASE CCCD")
                    || indicate
                    || !enabled
                {
                    return self.unexpected(result);
                }

                self.issue(
                    AsePreparePhase::SubscribeControlPoint,
                    GattOperation::Subscribe {
                        value_handle: self
                            .sink
                            .ascs_control_point
                            .value_handle,
                        cccd_handle: self
                            .control_point_cccd_handle
                            .expect("stored Control Point CCCD"),
                        indicate: false,
                    },
                )
            }
            AsePreparePhase::SubscribeControlPoint => {
                let GattResult::Subscription {
                    value_handle,
                    cccd_handle,
                    indicate,
                    enabled,
                } = result.clone()
                else {
                    return self.unexpected(result);
                };
                if value_handle
                    != self.sink.ascs_control_point.value_handle
                    || cccd_handle
                        != self
                            .control_point_cccd_handle
                            .expect("stored Control Point CCCD")
                    || indicate
                    || !enabled
                {
                    return self.unexpected(result);
                }

                let prepared = PreparedAseControl {
                    sink: self.sink.clone(),
                    ase_cccd_handle: self
                        .ase_cccd_handle
                        .expect("completed preparation missing ASE CCCD"),
                    control_point_cccd_handle: self
                        .control_point_cccd_handle
                        .expect("completed preparation missing CP CCCD"),
                };
                self.phase = AsePreparePhase::Complete;
                Ok(AsePrepareAction::Complete(prepared))
            }
            AsePreparePhase::Idle
            | AsePreparePhase::Complete
            | AsePreparePhase::Failed => self.unexpected(result),
        }
    }

    fn issue(
        &mut self,
        phase: AsePreparePhase,
        operation: GattOperation,
    ) -> Result<AsePrepareAction, AsePrepareFailure> {
        let request_id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(AsePrepareFailure::RequestIdOverflow)?;
        self.phase = phase;
        self.pending_request_id = Some(request_id);
        Ok(AsePrepareAction::BeginGatt(GattRequest {
            request_id,
            connection: self.sink.connection,
            operation,
        }))
    }

    fn unexpected(
        &mut self,
        result: GattResult,
    ) -> Result<AsePrepareAction, AsePrepareFailure> {
        self.fail(AsePrepareFailure::UnexpectedResult {
            phase: self.phase,
            result,
        })
    }

    fn fail<T>(
        &mut self,
        failure: AsePrepareFailure,
    ) -> Result<T, AsePrepareFailure> {
        self.phase = AsePreparePhase::Failed;
        self.pending_request_id = None;
        Err(failure)
    }
}

fn exactly_one_cccd(
    descriptors: &[crate::plane::l1_hci::gatt::transaction::GattDescriptor],
    label: &'static str,
) -> Result<u16, AsePrepareFailure> {
    let wanted = Uuid::from_16_bits(CCCD_UUID);
    let mut values =
        descriptors.iter().filter(|value| value.uuid == wanted);
    let first = values
        .next()
        .ok_or(AsePrepareFailure::MissingCccd { label })?;
    if values.next().is_some() {
        return Err(AsePrepareFailure::DuplicateCccd { label });
    }
    Ok(first.handle)
}



#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AseOperationPhase {
    Idle,
    WaitingGates,
    ReadingBack,
    Complete,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AseOperationAction {
    BeginGatt(GattRequest),
    Await,
    Complete(AseStatus),
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AseOperationFailure {
    InvalidStartState(AseOperationPhase),
    RequestIdOverflow,
    Encode(String),
    Gatt(GattFailure),
    BearerPoisoned {
        request_id: GattRequestId,
    },
    UnexpectedResult {
        phase: AseOperationPhase,
        result: GattResult,
    },
    ControlPointParse(String),
    ControlPointMissingAse {
        ase_id: u8,
    },
    ControlPointDuplicateAse {
        ase_id: u8,
    },
    ControlPointRejected {
        ase_id: u8,
        code: u8,
        reason: u8,
    },
    AseStatusParse(String),
    AseIdentityMismatch {
        expected: u8,
        actual: u8,
    },
    ReadbackStateMismatch {
        ase_id: u8,
        expected_state: u8,
        actual_state: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AseOperationCompletion {
    VerifiedReadback,
    NotificationGate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AseOperationGateSnapshot {
    pub phase: AseOperationPhase,
    pub pending_request_id: Option<GattRequestId>,
    pub write_complete: bool,
    pub cp_success: bool,
    pub notified_state: Option<AseState>,
}

pub struct AseOperationMachine {
    prepared: PreparedAseControl,
    operation: AseOperation,
    expected_notification_state: AseState,
    expected_readback_state: AseState,
    completion: AseOperationCompletion,
    phase: AseOperationPhase,
    pending_request_id: Option<GattRequestId>,
    next_request_id: GattRequestId,
    write_complete: bool,
    cp_success: bool,
    notified_status: Option<AseStatus>,
}

impl AseOperationMachine {
    pub fn new(
        prepared: PreparedAseControl,
        operation: AseOperation,
        expected_state: AseState,
        first_request_id: GattRequestId,
    ) -> Self {
        Self::new_with_states(
            prepared,
            operation,
            expected_state,
            expected_state,
            first_request_id,
        )
    }

    pub fn new_with_states(
        prepared: PreparedAseControl,
        operation: AseOperation,
        expected_notification_state: AseState,
        expected_readback_state: AseState,
        first_request_id: GattRequestId,
    ) -> Self {
        Self {
            prepared,
            operation,
            expected_notification_state,
            expected_readback_state,
            completion: AseOperationCompletion::VerifiedReadback,
            phase: AseOperationPhase::Idle,
            pending_request_id: None,
            next_request_id: first_request_id,
            write_complete: false,
            cp_success: false,
            notified_status: None,
        }
    }

    pub fn new_notification_gate(
        prepared: PreparedAseControl,
        operation: AseOperation,
        expected_notification_state: AseState,
        first_request_id: GattRequestId,
    ) -> Self {
        Self {
            prepared,
            operation,
            expected_notification_state,
            expected_readback_state: expected_notification_state,
            completion: AseOperationCompletion::NotificationGate,
            phase: AseOperationPhase::Idle,
            pending_request_id: None,
            next_request_id: first_request_id,
            write_complete: false,
            cp_success: false,
            notified_status: None,
        }
    }

    pub fn phase(&self) -> AseOperationPhase {
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
    ) -> Result<AseOperationAction, AseOperationFailure> {
        if self.phase != AseOperationPhase::Idle {
            return Err(AseOperationFailure::InvalidStartState(
                self.phase,
            ));
        }
        let value = match self.operation.to_bytes() {
            Ok(value) => value,
            Err(error) => {
                return self.fail(AseOperationFailure::Encode(
                    error.to_string(),
                ))
            }
        };
        self.issue(
            AseOperationPhase::WaitingGates,
            GattOperation::WriteRequest {
                attribute_handle:
                    self.prepared.sink.ascs_control_point.value_handle,
                value,
            },
        )
    }

    pub fn on_gatt_report(
        &mut self,
        report: &GattReport,
    ) -> Result<AseOperationAction, AseOperationFailure> {
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
                self.phase = AseOperationPhase::Failed;
                self.pending_request_id = None;
                Err(AseOperationFailure::Gatt(failure.clone()))
            }

            GattReport::BearerPoisoned {
                connection,
                request_id,
            } if Some(*request_id) == self.pending_request_id
                && *connection == self.prepared.sink.connection =>
            {
                self.phase = AseOperationPhase::Failed;
                self.pending_request_id = None;
                Err(AseOperationFailure::BearerPoisoned {
                    request_id: *request_id,
                })
            }

            _ => Ok(AseOperationAction::Ignored),
        }
    }

    fn on_result(
        &mut self,
        result: GattResult,
    ) -> Result<AseOperationAction, AseOperationFailure> {
        match self.phase {
            AseOperationPhase::WaitingGates => {
                let GattResult::WriteComplete {
                    attribute_handle,
                    with_response: true,
                } = result.clone()
                else {
                    return self.unexpected(result);
                };
                if attribute_handle
                    != self.prepared.sink.ascs_control_point.value_handle
                {
                    return self.unexpected(result);
                }
                self.write_complete = true;
                self.maybe_complete_or_begin_readback()
            }

            AseOperationPhase::ReadingBack => {
                let GattResult::Value {
                    attribute_handle,
                    value,
                } = result.clone()
                else {
                    return self.unexpected(result);
                };
                if attribute_handle
                    != self.prepared.sink.ase_value_handle
                {
                    return self.unexpected(result);
                }

                let status = match parse_ase_status_compat(&value) {
                    Ok(status) => status,
                    Err(error) => {
                        return self.fail(
                            AseOperationFailure::AseStatusParse(
                                format!("{error:?}"),
                            ),
                        )
                    }
                };
                self.verify_status_identity(&status)?;
                let release_cached_codec_configured =
                    matches!(&self.operation, AseOperation::Release(_))
                        && status.state == AseState::CODEC_CONFIGURED;
                if status.state != self.expected_readback_state
                    && !release_cached_codec_configured
                {
                    return self.fail(
                        AseOperationFailure::ReadbackStateMismatch {
                            ase_id: status.ase_id,
                            expected_state:
                                self.expected_readback_state.0,
                            actual_state: status.state.0,
                        },
                    );
                }

                self.phase = AseOperationPhase::Complete;
                Ok(AseOperationAction::Complete(status))
            }

            AseOperationPhase::Idle
            | AseOperationPhase::Complete
            | AseOperationPhase::Failed => self.unexpected(result),
        }
    }

    fn on_value_event(
        &mut self,
        event: &GattValueEvent,
    ) -> Result<AseOperationAction, AseOperationFailure> {
        if self.phase != AseOperationPhase::WaitingGates {
            return Ok(AseOperationAction::Ignored);
        }

        let (connection, attribute_handle, value) =
            value_event_parts(event);
        if connection != self.prepared.sink.connection {
            return Ok(AseOperationAction::Ignored);
        }

        if attribute_handle
            == self.prepared.sink.ascs_control_point.value_handle
        {
            let response = match AseControlResponse::from_bytes(value) {
                Ok(response) => response,
                Err(error) => {
                    return self.fail(
                        AseOperationFailure::ControlPointParse(
                            error.to_string(),
                        ),
                    )
                }
            };
            if response.opcode != self.operation.opcode() {
                return Ok(AseOperationAction::Ignored);
            }

            let mut matches = response
                .responses
                .iter()
                .filter(|entry| {
                    entry.ase_id == self.prepared.sink.ase_id
                });
            let entry = matches.next().ok_or(
                AseOperationFailure::ControlPointMissingAse {
                    ase_id: self.prepared.sink.ase_id,
                },
            )?;
            if matches.next().is_some() {
                return self.fail(
                    AseOperationFailure::ControlPointDuplicateAse {
                        ase_id: self.prepared.sink.ase_id,
                    },
                );
            }
            if entry.code != AseResponseCode::SUCCESS {
                return self.fail(
                    AseOperationFailure::ControlPointRejected {
                        ase_id: entry.ase_id,
                        code: entry.code.0,
                        reason: entry.reason.0,
                    },
                );
            }

            self.cp_success = true;
            return self.maybe_complete_or_begin_readback();
        }

        if attribute_handle == self.prepared.sink.ase_value_handle {
            let status = match parse_ase_status_compat(value) {
                Ok(status) => status,
                Err(error) => {
                    return self.fail(
                        AseOperationFailure::AseStatusParse(
                            format!("{error:?}"),
                        ),
                    )
                }
            };
            self.verify_status_identity(&status)?;
            if status.state == self.expected_notification_state {
                self.notified_status = Some(status);
                return self.maybe_complete_or_begin_readback();
            }
            return Ok(AseOperationAction::Await);
        }

        Ok(AseOperationAction::Ignored)
    }

    fn verify_status_identity(
        &mut self,
        status: &AseStatus,
    ) -> Result<(), AseOperationFailure> {
        if status.ase_id != self.prepared.sink.ase_id {
            return self.fail(
                AseOperationFailure::AseIdentityMismatch {
                    expected: self.prepared.sink.ase_id,
                    actual: status.ase_id,
                },
            );
        }
        Ok(())
    }

    fn maybe_complete_or_begin_readback(
        &mut self,
    ) -> Result<AseOperationAction, AseOperationFailure> {
        if self.phase != AseOperationPhase::WaitingGates
            || self.pending_request_id.is_some()
            || !self.write_complete
            || !self.cp_success
        {
            return Ok(AseOperationAction::Await);
        }

        let Some(notified_status) = self.notified_status.clone() else {
            return Ok(AseOperationAction::Await);
        };

        if self.completion == AseOperationCompletion::NotificationGate {
            self.phase = AseOperationPhase::Complete;
            return Ok(AseOperationAction::Complete(notified_status));
        }

        self.issue(
            AseOperationPhase::ReadingBack,
            GattOperation::ReadValue {
                attribute_handle:
                    self.prepared.sink.ase_value_handle,
            },
        )
    }

    fn issue(
        &mut self,
        phase: AseOperationPhase,
        operation: GattOperation,
    ) -> Result<AseOperationAction, AseOperationFailure> {
        let request_id = self.next_request_id;
        self.next_request_id = match self.next_request_id.checked_add(1) {
            Some(next) => next,
            None => {
                return self.fail(
                    AseOperationFailure::RequestIdOverflow,
                )
            }
        };
        self.phase = phase;
        self.pending_request_id = Some(request_id);
        Ok(AseOperationAction::BeginGatt(GattRequest {
            request_id,
            connection: self.prepared.sink.connection,
            operation,
        }))
    }

    fn unexpected(
        &mut self,
        result: GattResult,
    ) -> Result<AseOperationAction, AseOperationFailure> {
        self.fail(AseOperationFailure::UnexpectedResult {
            phase: self.phase,
            result,
        })
    }

    fn fail<T>(
        &mut self,
        failure: AseOperationFailure,
    ) -> Result<T, AseOperationFailure> {
        self.phase = AseOperationPhase::Failed;
        self.pending_request_id = None;
        Err(failure)
    }
}

fn value_event_parts(
    event: &GattValueEvent,
) -> (crate::plane::l1_hci::acl::connection::ConnectionToken, u16, &[u8]) {
    match event {
        GattValueEvent::Notification {
            connection,
            attribute_handle,
            value,
        }
        | GattValueEvent::Indication {
            connection,
            attribute_handle,
            value,
        } => (*connection, *attribute_handle, value),
    }
}

#[cfg(test)]
mod tests {
    use bumble_hci::CodingFormat;
    use bumble_profiles::{
        ascs::{
            AseControlResponse, AseEndpoint, AseOpcode, AseOperation,
            AseReasonCode, AseResponse, AseResponseCode, AseState,
            AudioRole,
        },
        bap::{
            AudioLocation, CodecSpecificConfiguration, FrameDuration,
            SamplingFrequency,
        },
    };

    use crate::{
        plane::l1_control::interface::stream::AudioSide,
        plane::l1_control::discovery::snapshot::GattCharacteristicRange,
        plane::l1_hci::{
            acl::connection::ConnectionToken,
            gatt::transaction::{
                GattDescriptor, GattResult, GattValueEvent,
            },
        },
    };

    use super::*;

    fn sink() -> PlannedSink {
        PlannedSink {
            side: AudioSide::Left,
            member_index: 1,
            connection: ConnectionToken {
                connection_handle: 0x0801,
                generation: 2,
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
        }
    }

    fn cccd(handle: u16) -> GattDescriptor {
        GattDescriptor {
            handle,
            uuid: Uuid::from_16_bits(CCCD_UUID),
        }
    }

    fn expect_request(action: AsePrepareAction) -> GattRequest {
        let AsePrepareAction::BeginGatt(request) = action else {
            panic!("expected BeginGatt");
        };
        request
    }

    fn complete(
        machine: &mut AsePrepareMachine,
        request: GattRequest,
        result: GattResult,
    ) -> AsePrepareAction {
        machine
            .on_gatt_report(&GattReport::Complete {
                request_id: request.request_id,
                connection: request.connection,
                result,
            })
            .unwrap()
    }

    #[test]
    fn preparation_discovers_both_cccds_then_subscribes_ase_and_cp() {
        let mut machine = AsePrepareMachine::new(sink(), 40);

        let request = expect_request(machine.start().unwrap());
        assert_eq!(
            request.operation,
            GattOperation::DiscoverDescriptors {
                starting_handle: 0x0047,
                ending_handle: 0x0048,
            }
        );

        let request = expect_request(complete(
            &mut machine,
            request,
            GattResult::Descriptors(vec![cccd(0x0048)]),
        ));
        assert_eq!(
            request.operation,
            GattOperation::DiscoverDescriptors {
                starting_handle: 0x0044,
                ending_handle: 0x0045,
            }
        );

        let request = expect_request(complete(
            &mut machine,
            request,
            GattResult::Descriptors(vec![cccd(0x0045)]),
        ));
        assert_eq!(
            request.operation,
            GattOperation::Subscribe {
                value_handle: 0x0046,
                cccd_handle: 0x0048,
                indicate: false,
            }
        );

        let request = expect_request(complete(
            &mut machine,
            request,
            GattResult::Subscription {
                value_handle: 0x0046,
                cccd_handle: 0x0048,
                indicate: false,
                enabled: true,
            },
        ));
        assert_eq!(
            request.operation,
            GattOperation::Subscribe {
                value_handle: 0x0043,
                cccd_handle: 0x0045,
                indicate: false,
            }
        );

        let action = complete(
            &mut machine,
            request,
            GattResult::Subscription {
                value_handle: 0x0043,
                cccd_handle: 0x0045,
                indicate: false,
                enabled: true,
            },
        );

        let AsePrepareAction::Complete(prepared) = action else {
            panic!("expected prepared ASE control");
        };
        assert_eq!(machine.phase(), AsePreparePhase::Complete);
        assert_eq!(prepared.ase_cccd_handle, 0x0048);
        assert_eq!(prepared.control_point_cccd_handle, 0x0045);
        assert_eq!(prepared.sink.ase_id, 1);
        assert_eq!(machine.next_request_id(), 44);
    }

    fn prepared() -> PreparedAseControl {
        PreparedAseControl {
            sink: sink(),
            ase_cccd_handle: 0x0048,
            control_point_cccd_handle: 0x0045,
        }
    }

    fn operation_request(action: AseOperationAction) -> GattRequest {
        let AseOperationAction::BeginGatt(request) = action else {
            panic!("expected ASCS BeginGatt");
        };
        request
    }

    fn codec_status_bytes() -> Vec<u8> {
        let planned = sink();
        let mut endpoint = AseEndpoint::new(1, AudioRole::Sink);
        endpoint.state = AseState::CODEC_CONFIGURED;
        endpoint.codec_id = CodingFormat::LC3;
        endpoint.codec_specific_configuration =
            planned.codec_specific_configuration.to_bytes();
        endpoint.value().unwrap()
    }

    fn cp_response_bytes(
        code: AseResponseCode,
        reason: AseReasonCode,
    ) -> Vec<u8> {
        AseControlResponse {
            opcode: AseOpcode::ConfigCodec,
            responses: vec![AseResponse {
                ase_id: 1,
                code,
                reason,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    #[test]
    fn ascs_operation_requires_write_cp_and_ase_notification_before_readback() {
        let prepared = prepared();
        let operation = AseOperation::ConfigCodec(vec![
            prepared.sink.config_codec_parameters(),
        ]);
        let mut machine = AseOperationMachine::new(
            prepared.clone(),
            operation,
            AseState::CODEC_CONFIGURED,
            100,
        );

        let write = operation_request(machine.start().unwrap());
        assert_eq!(write.request_id, 100);
        assert!(matches!(
            write.operation,
            GattOperation::WriteRequest {
                attribute_handle: 0x0043,
                ..
            }
        ));

        // CP and ASE notifications may race ahead of ATT Write Response.
        assert_eq!(
            machine
                .on_gatt_report(&GattReport::Value(
                    GattValueEvent::Notification {
                        connection: prepared.sink.connection,
                        attribute_handle: 0x0043,
                        value: cp_response_bytes(
                            AseResponseCode::SUCCESS,
                            AseReasonCode::NONE,
                        ),
                    },
                ))
                .unwrap(),
            AseOperationAction::Await
        );
        assert_eq!(
            machine
                .on_gatt_report(&GattReport::Value(
                    GattValueEvent::Notification {
                        connection: prepared.sink.connection,
                        attribute_handle: 0x0046,
                        value: codec_status_bytes(),
                    },
                ))
                .unwrap(),
            AseOperationAction::Await
        );

        let readback = operation_request(
            machine
                .on_gatt_report(&GattReport::Complete {
                    request_id: write.request_id,
                    connection: write.connection,
                    result: GattResult::WriteComplete {
                        attribute_handle: 0x0043,
                        with_response: true,
                    },
                })
                .unwrap(),
        );
        assert_eq!(readback.request_id, 101);
        assert_eq!(
            readback.operation,
            GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        );
        assert_eq!(
            machine.phase(),
            AseOperationPhase::ReadingBack
        );

        let status = codec_status_bytes();
        let action = machine
            .on_gatt_report(&GattReport::Complete {
                request_id: readback.request_id,
                connection: readback.connection,
                result: GattResult::Value {
                    attribute_handle: 0x0046,
                    value: status,
                },
            })
            .unwrap();

        let AseOperationAction::Complete(status) = action else {
            panic!("expected completed ASCS operation");
        };
        assert_eq!(status.ase_id, 1);
        assert_eq!(status.state, AseState::CODEC_CONFIGURED);
        assert_eq!(machine.phase(), AseOperationPhase::Complete);
        assert_eq!(machine.next_request_id(), 102);
    }

    #[test]
    fn release_allows_releasing_notification_then_idle_readback() {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        let prepared = prepared();
        let mut machine = AseOperationMachine::new_with_states(
            prepared.clone(),
            AseOperation::Release(vec![prepared.sink.ase_id]),
            AseState::RELEASING,
            AseState::IDLE,
            200,
        );

        let write = operation_request(machine.start().unwrap());
        let _ = machine
            .on_gatt_report(&GattReport::Complete {
                request_id: write.request_id,
                connection: write.connection,
                result: GattResult::WriteComplete {
                    attribute_handle: 0x0043,
                    with_response: true,
                },
            })
            .unwrap();

        let cp = AseControlResponse {
            opcode: AseOpcode::Release,
            responses: vec![AseResponse {
                ase_id: 1,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap();
        assert_eq!(
            machine
                .on_gatt_report(&GattReport::Value(
                    GattValueEvent::Notification {
                        connection: prepared.sink.connection,
                        attribute_handle: 0x0043,
                        value: cp,
                    },
                ))
                .unwrap(),
            AseOperationAction::Await
        );

        assert_eq!(
            machine
                .on_gatt_report(&GattReport::Value(
                    GattValueEvent::Notification {
                        connection: prepared.sink.connection,
                        attribute_handle: 0x0046,
                        value: vec![1, AseState::RELEASING.0],
                    },
                ))
                .unwrap(),
            AseOperationAction::BeginGatt(GattRequest {
                request_id: 201,
                connection: prepared.sink.connection,
                operation: GattOperation::ReadValue {
                    attribute_handle: 0x0046,
                },
            })
        );

        let action = machine
            .on_gatt_report(&GattReport::Complete {
                request_id: 201,
                connection: prepared.sink.connection,
                result: GattResult::Value {
                    attribute_handle: 0x0046,
                    value: vec![1, AseState::IDLE.0],
                },
            })
            .unwrap();
        assert!(matches!(
            action,
            AseOperationAction::Complete(AseStatus {
                ase_id: 1,
                state: AseState::IDLE,
                ..
            })
        ));
    }

    #[test]
    fn release_notification_gate_completes_at_releasing_without_readback() {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        let prepared = prepared();
        let mut machine = AseOperationMachine::new_notification_gate(
            prepared.clone(),
            AseOperation::Release(vec![prepared.sink.ase_id]),
            AseState::RELEASING,
            210,
        );

        let write = operation_request(machine.start().unwrap());
        let _ = machine
            .on_gatt_report(&GattReport::Complete {
                request_id: write.request_id,
                connection: write.connection,
                result: GattResult::WriteComplete {
                    attribute_handle: 0x0043,
                    with_response: true,
                },
            })
            .unwrap();

        let cp = AseControlResponse {
            opcode: AseOpcode::Release,
            responses: vec![AseResponse {
                ase_id: 1,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap();
        assert_eq!(
            machine
                .on_gatt_report(&GattReport::Value(
                    GattValueEvent::Notification {
                        connection: prepared.sink.connection,
                        attribute_handle: 0x0043,
                        value: cp,
                    },
                ))
                .unwrap(),
            AseOperationAction::Await
        );

        let action = machine
            .on_gatt_report(&GattReport::Value(
                GattValueEvent::Notification {
                    connection: prepared.sink.connection,
                    attribute_handle: 0x0046,
                    value: vec![1, AseState::RELEASING.0],
                },
            ))
            .unwrap();

        assert!(matches!(
            action,
            AseOperationAction::Complete(AseStatus {
                ase_id: 1,
                state: AseState::RELEASING,
                ..
            })
        ));
        assert_eq!(machine.phase(), AseOperationPhase::Complete);
        assert_eq!(machine.pending_request_id(), None);
        assert_eq!(machine.next_request_id(), 211);
    }

    #[test]
    fn release_allows_cached_codec_configured_readback() {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        let prepared = prepared();
        let mut machine = AseOperationMachine::new_with_states(
            prepared.clone(),
            AseOperation::Release(vec![prepared.sink.ase_id]),
            AseState::RELEASING,
            AseState::IDLE,
            220,
        );

        let write = operation_request(machine.start().unwrap());
        let _ = machine
            .on_gatt_report(&GattReport::Complete {
                request_id: write.request_id,
                connection: write.connection,
                result: GattResult::WriteComplete {
                    attribute_handle: 0x0043,
                    with_response: true,
                },
            })
            .unwrap();

        let cp = AseControlResponse {
            opcode: AseOpcode::Release,
            responses: vec![AseResponse {
                ase_id: 1,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap();
        let _ = machine
            .on_gatt_report(&GattReport::Value(
                GattValueEvent::Notification {
                    connection: prepared.sink.connection,
                    attribute_handle: 0x0043,
                    value: cp,
                },
            ))
            .unwrap();

        let readback = operation_request(
            machine
                .on_gatt_report(&GattReport::Value(
                    GattValueEvent::Notification {
                        connection: prepared.sink.connection,
                        attribute_handle: 0x0046,
                        value: vec![1, AseState::RELEASING.0],
                    },
                ))
                .unwrap(),
        );

        let action = machine
            .on_gatt_report(&GattReport::Complete {
                request_id: readback.request_id,
                connection: readback.connection,
                result: GattResult::Value {
                    attribute_handle: 0x0046,
                    value: codec_status_bytes(),
                },
            })
            .unwrap();

        assert!(matches!(
            action,
            AseOperationAction::Complete(AseStatus {
                ase_id: 1,
                state: AseState::CODEC_CONFIGURED,
                ..
            })
        ));
    }

    #[test]
    fn ascs_operation_cp_rejection_is_terminal_before_readback() {
        let prepared = prepared();
        let operation = AseOperation::ConfigCodec(vec![
            prepared.sink.config_codec_parameters(),
        ]);
        let mut machine = AseOperationMachine::new(
            prepared.clone(),
            operation,
            AseState::CODEC_CONFIGURED,
            1,
        );
        let _write = operation_request(machine.start().unwrap());

        let error = machine
            .on_gatt_report(&GattReport::Value(
                GattValueEvent::Notification {
                    connection: prepared.sink.connection,
                    attribute_handle: 0x0043,
                    value: cp_response_bytes(
                        AseResponseCode::
                            REJECTED_CONFIGURATION_PARAMETER_VALUE,
                        AseReasonCode::
                            CODEC_SPECIFIC_CONFIGURATION,
                    ),
                },
            ))
            .unwrap_err();

        assert_eq!(
            error,
            AseOperationFailure::ControlPointRejected {
                ase_id: 1,
                code: AseResponseCode::
                    REJECTED_CONFIGURATION_PARAMETER_VALUE
                    .0,
                reason: AseReasonCode::
                    CODEC_SPECIFIC_CONFIGURATION
                    .0,
            }
        );
        assert_eq!(machine.phase(), AseOperationPhase::Failed);
        assert_eq!(machine.pending_request_id(), None);
    }

    #[test]
    fn preparation_rejects_missing_or_duplicate_cccd() {
        let mut machine = AsePrepareMachine::new(sink(), 1);
        let request = expect_request(machine.start().unwrap());
        assert_eq!(
            machine
                .on_gatt_report(&GattReport::Complete {
                    request_id: request.request_id,
                    connection: request.connection,
                    result: GattResult::Descriptors(vec![]),
                })
                .unwrap_err(),
            AsePrepareFailure::MissingCccd {
                label: "Sink ASE",
            }
        );

        let mut machine = AsePrepareMachine::new(sink(), 10);
        let request = expect_request(machine.start().unwrap());
        assert_eq!(
            machine
                .on_gatt_report(&GattReport::Complete {
                    request_id: request.request_id,
                    connection: request.connection,
                    result: GattResult::Descriptors(vec![
                        cccd(0x0047),
                        cccd(0x0048),
                    ]),
                })
                .unwrap_err(),
            AsePrepareFailure::DuplicateCccd {
                label: "Sink ASE",
            }
        );
    }
}
