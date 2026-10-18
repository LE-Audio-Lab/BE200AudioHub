use bumble::Uuid;

use crate::hci_host::{
    gatt::{
        GattFailure, GattOperation, GattReport, GattRequest,
        GattRequestId, GattResult,
    },
};

use super::codec_plan::PlannedSink;

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
    descriptors: &[crate::hci_host::gatt::GattDescriptor],
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


#[cfg(test)]
mod tests {
    use bumble_hci::CodingFormat;
    use bumble_profiles::bap::{
        AudioLocation, CodecSpecificConfiguration, FrameDuration,
        SamplingFrequency,
    };

    use crate::{
        architecture::types::AudioSide,
        control::audio_snapshot::GattCharacteristicRange,
        hci_host::{
            connection::ConnectionToken,
            gatt::{GattDescriptor, GattResult},
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
