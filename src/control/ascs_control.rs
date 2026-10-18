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
        let end = self
            .sink
            .ase_value_handle
            .max(self.sink.ase_value_handle)
            .saturating_add(
                self.sink
                    .codec_specific_configuration
                    .codec_frames_per_sdu
                    .map(|_| 0)
                    .unwrap_or(0),
            );
        let end = self
            .sink
            .ascs_control_point
            .value_handle
            .checked_sub(1)
            .unwrap_or(self.sink.ase_value_handle);

        // The selected ASE's actual end-group handle is not derivable from the
        // control point. The planner therefore must preserve it explicitly.
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
                    enabled: true,
                    ..
                } = result
                else {
                    return self.unexpected(result);
                };
                if value_handle != self.sink.ase_value_handle
                    || cccd_handle
                        != self
                            .ase_cccd_handle
                            .expect("stored Sink ASE CCCD")
                {
                    return self.unexpected(
                        GattResult::Subscription {
                            value_handle,
                            cccd_handle,
                            indicate: false,
                            enabled: true,
                        },
                    );
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
                    enabled: true,
                    ..
                } = result
                else {
                    return self.unexpected(result);
                };
                if value_handle
                    != self.sink.ascs_control_point.value_handle
                    || cccd_handle
                        != self
                            .control_point_cccd_handle
                            .expect("stored Control Point CCCD")
                {
                    return self.unexpected(
                        GattResult::Subscription {
                            value_handle,
                            cccd_handle,
                            indicate: false,
                            enabled: true,
                        },
                    );
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
