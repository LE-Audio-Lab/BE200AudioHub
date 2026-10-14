use bumble::{Address, AddressType};
use bumble_hci::Command;

pub const DISCONNECT_REASON_REMOTE_USER_TERMINATED: u8 = 0x13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectConfigError {
    InvalidConnectionHandle(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectFailureKind {
    CommandStatus { status: u8 },
    DisconnectionEventStatus { status: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisconnectFailure {
    pub connection: ConnectionToken,
    pub kind: DisconnectFailureKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectState {
    Idle,
    Submitting,
    AwaitingEvent,
    Disconnected {
        connection: ConnectionToken,
        event_reason: u8,
    },
    Failed(DisconnectFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectEvent {
    Disconnected {
        connection_handle: u16,
        reason: u8,
    },
    Failed {
        connection_handle: u16,
        status: u8,
    },
}

#[derive(Debug)]
pub enum DisconnectAction {
    AwaitEvent,
    Disconnected {
        connection: ConnectionToken,
        event_reason: u8,
    },
    Failed(DisconnectFailure),
    Ignored,
}

pub struct DisconnectTransaction {
    connection: ConnectionToken,
    awaiting_event: bool,
    terminal: bool,
}

impl DisconnectTransaction {
    pub fn begin(
        connection: ConnectionToken,
    ) -> Result<(Self, Command), DisconnectConfigError> {
        if connection.connection_handle > 0x0eff {
            return Err(DisconnectConfigError::InvalidConnectionHandle(
                connection.connection_handle,
            ));
        }

        Ok((
            Self {
                connection,
                awaiting_event: false,
                terminal: false,
            },
            Command::Disconnect {
                connection_handle: connection.connection_handle,
                reason: DISCONNECT_REASON_REMOTE_USER_TERMINATED,
            },
        ))
    }

    pub fn on_command_status(
        &mut self,
        opcode: u16,
        status: u8,
    ) -> DisconnectAction {
        if self.terminal
            || opcode != bumble_hci::codes::HCI_DISCONNECT_COMMAND
        {
            return DisconnectAction::Ignored;
        }

        if status != 0 {
            self.terminal = true;
            return DisconnectAction::Failed(DisconnectFailure {
                connection: self.connection,
                kind: DisconnectFailureKind::CommandStatus { status },
            });
        }

        self.awaiting_event = true;
        DisconnectAction::AwaitEvent
    }

    pub fn on_event(
        &mut self,
        event: DisconnectEvent,
    ) -> DisconnectAction {
        if self.terminal || !self.awaiting_event {
            return DisconnectAction::Ignored;
        }

        match event {
            DisconnectEvent::Disconnected {
                connection_handle,
                reason,
            } => {
                if connection_handle != self.connection.connection_handle {
                    return DisconnectAction::Ignored;
                }
                self.terminal = true;
                DisconnectAction::Disconnected {
                    connection: self.connection,
                    event_reason: reason,
                }
            }
            DisconnectEvent::Failed {
                connection_handle,
                status,
            } => {
                if connection_handle != self.connection.connection_handle {
                    return DisconnectAction::Ignored;
                }
                self.terminal = true;
                DisconnectAction::Failed(DisconnectFailure {
                    connection: self.connection,
                    kind: DisconnectFailureKind::DisconnectionEventStatus {
                        status,
                    },
                })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LePeerIdentity {
    /// Bluetooth address bytes in HCI/wire order.
    pub address_le: [u8; 6],
    /// Public=0, Random=1.
    pub address_type: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeInitiationParameters {
    pub scan_interval: u16,
    pub scan_window: u16,
    pub connection_interval_min: u16,
    pub connection_interval_max: u16,
    pub max_latency: u16,
    pub supervision_timeout: u16,
    pub min_ce_length: u16,
    pub max_ce_length: u16,
}

impl Default for LeInitiationParameters {
    fn default() -> Self {
        // Values validated in the sealed Experiment direct/FAL path.
        Self {
            scan_interval: 0x0010,
            scan_window: 0x0010,
            connection_interval_min: 24,
            connection_interval_max: 40,
            max_latency: 0,
            supervision_timeout: 42,
            min_ce_length: 0,
            max_ce_length: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FalConnectRequest {
    pub peer: LePeerIdentity,
    pub parameters: LeInitiationParameters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FalConnectConfigError {
    InvalidAddressType(u8),
    ZeroScanInterval,
    ScanWindowExceedsInterval,
    InvalidConnectionIntervalRange,
    ZeroSupervisionTimeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionToken {
    pub connection_handle: u16,
    pub generation: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeConnectionSetSnapshot {
    pub connections: Vec<LeConnectionSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeConnectionSnapshot {
    pub connection_handle: u16,
    pub generation: u64,
    pub role: u8,
    pub peer_address_le: [u8; 6],
    pub peer_address_type: u8,
    pub connection_interval: u16,
    pub peripheral_latency: u16,
    pub supervision_timeout: u16,
    pub encryption_enabled: u8,
    pub encryption_key_size: u8,
}

impl LeConnectionSnapshot {
    pub const fn token(&self) -> ConnectionToken {
        ConnectionToken {
            connection: self.connection,
            generation: self.generation,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FalConnectFailureKind {
    CommandStatus { opcode: u16, status: u8 },
    MalformedCapacityResponse,
    EmptyFilterAcceptList,
    ConnectionFailed { status: u8 },
    Cancelled { connection_status: u8 },
    CleanupAfterConnection {
        connection_handle: u16,
        status: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FalConnectFailure {
    pub kind: FalConnectFailureKind,
    pub cleanup_status: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FalConnectionState {
    Idle,
    Preparing,
    Initiating,
    AwaitingConnection,
    Cancelling,
    CancelAccepted,
    CancelRejected { status: u8 },
    Cleaning,
    Connected(LeConnectionSnapshot),
    ConnectedAfterCancel(LeConnectionSnapshot),
    Failed(FalConnectFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionEvent {
    Established(LeConnectionSnapshot),
    Failed { status: u8 },
}

#[derive(Debug)]
pub enum FalConnectAction {
    Send(Command),
    AwaitConnection,
    CancelAccepted,
    CancelRejected { status: u8 },
    Connected(LeConnectionSnapshot),
    ConnectedAfterCancel(LeConnectionSnapshot),
    Failed(FalConnectFailure),
    Ignored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    ReadingCapacity,
    ClearingBeforeAdd,
    AddingPeer,
    SubmittingConnection,
    AwaitingConnection,
    Cancelling,
    ClearingAfterSuccess,
    ClearingAfterFailure,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CancelProgress {
    None,
    Pending,
    Accepted,
    Rejected,
}

pub struct FalConnectTransaction {
    request: FalConnectRequest,
    phase: Phase,
    pending_opcode: u16,
    pending_connection: Option<LeConnectionSnapshot>,
    failure: Option<FalConnectFailure>,
    cancel_progress: CancelProgress,
}

impl FalConnectTransaction {
    pub fn begin(
        request: FalConnectRequest,
    ) -> Result<(Self, Command), FalConnectConfigError> {
        validate_request(&request)?;
        let command = Command::LeReadFilterAcceptListSize;
        Ok((
            Self {
                request,
                phase: Phase::ReadingCapacity,
                pending_opcode: command.op_code(),
                pending_connection: None,
                failure: None,
                cancel_progress: CancelProgress::None,
            },
            command,
        ))
    }

    pub fn request_cancel(&mut self) -> FalConnectAction {
        if self.phase != Phase::AwaitingConnection
            || matches!(
                self.cancel_progress,
                CancelProgress::Pending | CancelProgress::Accepted
            )
        {
            return FalConnectAction::Ignored;
        }

        self.cancel_progress = CancelProgress::Pending;
        self.phase = Phase::Cancelling;
        self.send(Command::LeCreateConnectionCancel)
    }

    pub fn on_command_complete(
        &mut self,
        opcode: u16,
        status: u8,
        parameters_after_status: &[u8],
    ) -> FalConnectAction {
        if opcode != self.pending_opcode {
            return FalConnectAction::Ignored;
        }

        match self.phase {
            Phase::ReadingCapacity => {
                if status != 0 {
                    return self.fail_now(
                        FalConnectFailureKind::CommandStatus { opcode, status },
                    );
                }
                if parameters_after_status.len() != 1 {
                    return self.fail_now(
                        FalConnectFailureKind::MalformedCapacityResponse,
                    );
                }
                if parameters_after_status[0] == 0 {
                    return self.fail_now(
                        FalConnectFailureKind::EmptyFilterAcceptList,
                    );
                }

                self.phase = Phase::ClearingBeforeAdd;
                self.send(Command::LeClearFilterAcceptList)
            }
            Phase::ClearingBeforeAdd => {
                if status != 0 {
                    return self.fail_now(
                        FalConnectFailureKind::CommandStatus { opcode, status },
                    );
                }
                self.phase = Phase::AddingPeer;
                self.send(self.add_peer_command())
            }
            Phase::AddingPeer => {
                if status != 0 {
                    self.failure = Some(FalConnectFailure {
                        kind: FalConnectFailureKind::CommandStatus {
                            opcode,
                            status,
                        },
                        cleanup_status: None,
                    });
                    self.phase = Phase::ClearingAfterFailure;
                    return self.send(Command::LeClearFilterAcceptList);
                }

                self.phase = Phase::SubmittingConnection;
                self.send(self.create_connection_command())
            }
            Phase::Cancelling => {
                self.pending_opcode = 0;
                self.phase = Phase::AwaitingConnection;

                if status == 0 {
                    self.cancel_progress = CancelProgress::Accepted;
                    FalConnectAction::CancelAccepted
                } else {
                    self.cancel_progress = CancelProgress::Rejected;
                    FalConnectAction::CancelRejected { status }
                }
            }
            Phase::ClearingAfterSuccess => {
                let connection = self
                    .pending_connection
                    .take()
                    .expect("success cleanup always owns a connection");

                if status != 0 {
                    self.phase = Phase::Failed;
                    return FalConnectAction::Failed(FalConnectFailure {
                        kind: FalConnectFailureKind::CleanupAfterConnection {
                            connection_handle: connection.connection_handle,
                            status,
                        },
                        cleanup_status: Some(status),
                    });
                }

                self.phase = Phase::Complete;
                if matches!(
                    self.cancel_progress,
                    CancelProgress::Pending | CancelProgress::Accepted
                ) {
                    FalConnectAction::ConnectedAfterCancel(connection)
                } else {
                    FalConnectAction::Connected(connection)
                }
            }
            Phase::ClearingAfterFailure => {
                let mut failure = self
                    .failure
                    .take()
                    .expect("failure cleanup always owns a failure");
                if status != 0 {
                    failure.cleanup_status = Some(status);
                }
                self.phase = Phase::Failed;
                FalConnectAction::Failed(failure)
            }
            Phase::SubmittingConnection
            | Phase::AwaitingConnection
            | Phase::Complete
            | Phase::Failed => FalConnectAction::Ignored,
        }
    }

    pub fn on_command_status(
        &mut self,
        opcode: u16,
        status: u8,
    ) -> FalConnectAction {
        if self.phase != Phase::SubmittingConnection
            || opcode != self.pending_opcode
        {
            return FalConnectAction::Ignored;
        }

        if status != 0 {
            self.failure = Some(FalConnectFailure {
                kind: FalConnectFailureKind::CommandStatus { opcode, status },
                cleanup_status: None,
            });
            self.phase = Phase::ClearingAfterFailure;
            return self.send(Command::LeClearFilterAcceptList);
        }

        self.phase = Phase::AwaitingConnection;
        self.pending_opcode = 0;
        FalConnectAction::AwaitConnection
    }

    pub fn on_connection_event(
        &mut self,
        event: ConnectionEvent,
    ) -> FalConnectAction {
        if !matches!(
            self.phase,
            Phase::AwaitingConnection | Phase::Cancelling
        ) {
            return FalConnectAction::Ignored;
        }

        match event {
            ConnectionEvent::Established(connection) => {
                self.pending_connection = Some(connection);
                self.phase = Phase::ClearingAfterSuccess;
                self.send(Command::LeClearFilterAcceptList)
            }
            ConnectionEvent::Failed { status } => {
                let kind = if matches!(
                    self.cancel_progress,
                    CancelProgress::Pending | CancelProgress::Accepted
                ) {
                    FalConnectFailureKind::Cancelled {
                        connection_status: status,
                    }
                } else {
                    FalConnectFailureKind::ConnectionFailed { status }
                };
                self.failure = Some(FalConnectFailure {
                    kind,
                    cleanup_status: None,
                });
                self.phase = Phase::ClearingAfterFailure;
                self.send(Command::LeClearFilterAcceptList)
            }
        }
    }

    fn add_peer_command(&self) -> Command {
        Command::LeAddDeviceToFilterAcceptList {
            address_type: self.request.peer.address_type,
            address: Address::from_bytes(
                self.request.peer.address_le,
                AddressType(self.request.peer.address_type),
            ),
        }
    }

    fn create_connection_command(&self) -> Command {
        let p = self.request.parameters;
        Command::LeExtendedCreateConnection {
            initiator_filter_policy: 1,
            own_address_type: 0,
            peer_address_type: 0,
            peer_address: Address::from_bytes(
                [0; 6],
                AddressType::PUBLIC_DEVICE,
            ),
            initiating_phys: 0x01,
            scan_intervals: vec![p.scan_interval],
            scan_windows: vec![p.scan_window],
            connection_interval_mins: vec![p.connection_interval_min],
            connection_interval_maxs: vec![p.connection_interval_max],
            max_latencies: vec![p.max_latency],
            supervision_timeouts: vec![p.supervision_timeout],
            min_ce_lengths: vec![p.min_ce_length],
            max_ce_lengths: vec![p.max_ce_length],
        }
    }

    fn send(&mut self, command: Command) -> FalConnectAction {
        self.pending_opcode = command.op_code();
        FalConnectAction::Send(command)
    }

    fn fail_now(
        &mut self,
        kind: FalConnectFailureKind,
    ) -> FalConnectAction {
        self.phase = Phase::Failed;
        FalConnectAction::Failed(FalConnectFailure {
            kind,
            cleanup_status: None,
        })
    }
}

fn validate_request(
    request: &FalConnectRequest,
) -> Result<(), FalConnectConfigError> {
    if !matches!(request.peer.address_type, 0 | 1) {
        return Err(FalConnectConfigError::InvalidAddressType(
            request.peer.address_type,
        ));
    }

    let p = request.parameters;
    if p.scan_interval == 0 {
        return Err(FalConnectConfigError::ZeroScanInterval);
    }
    if p.scan_window == 0 || p.scan_window > p.scan_interval {
        return Err(FalConnectConfigError::ScanWindowExceedsInterval);
    }
    if p.connection_interval_min > p.connection_interval_max {
        return Err(
            FalConnectConfigError::InvalidConnectionIntervalRange,
        );
    }
    if p.supervision_timeout == 0 {
        return Err(FalConnectConfigError::ZeroSupervisionTimeout);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bumble_hci::codes::{
        HCI_LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST_COMMAND,
        HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
        HCI_LE_READ_FILTER_ACCEPT_LIST_SIZE_COMMAND,
    };

    use super::*;

    fn request() -> FalConnectRequest {
        FalConnectRequest {
            peer: LePeerIdentity {
                address_le: [0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8],
                address_type: 0,
            },
            parameters: LeInitiationParameters::default(),
        }
    }

    fn snapshot(handle: u16) -> LeConnectionSnapshot {
        LeConnectionSnapshot {
            connection_handle: handle,
            generation: 1,
            role: 0,
            peer_address_le: [1, 2, 3, 4, 5, 6],
            peer_address_type: 1,
            connection_interval: 24,
            peripheral_latency: 0,
            supervision_timeout: 42,
            encryption_enabled: 0,
            encryption_key_size: 0,
        }
    }

    fn expect_send(action: FalConnectAction, opcode: u16) -> Command {
        match action {
            FalConnectAction::Send(command) => {
                assert_eq!(command.op_code(), opcode);
                command
            }
            other => panic!("expected Send({opcode:#06x}), got {other:?}"),
        }
    }

    #[test]
    fn disconnect_command_reason_is_fixed_to_0x13() {
        let (_tx, command) =
            DisconnectTransaction::begin(ConnectionToken { connection_handle: 0x0800, generation: 7 }).unwrap();
        match command {
            Command::Disconnect {
                connection_handle,
                reason,
            } => {
                assert_eq!(connection_handle, 0x0800);
                assert_eq!(
                    reason,
                    DISCONNECT_REASON_REMOTE_USER_TERMINATED
                );
                assert_eq!(reason, 0x13);
            }
            other => panic!("expected Disconnect command, got {other:?}"),
        }
    }

    #[test]
    fn local_0x16_event_reason_is_result_not_command_parameter() {
        let (mut tx, _command) =
            DisconnectTransaction::begin(ConnectionToken { connection_handle: 0x0800, generation: 7 }).unwrap();

        assert!(matches!(
            tx.on_command_status(
                bumble_hci::codes::HCI_DISCONNECT_COMMAND,
                0,
            ),
            DisconnectAction::AwaitEvent
        ));

        match tx.on_event(DisconnectEvent::Disconnected {
            connection_handle: 0x0800,
            reason: 0x16,
        }) {
            DisconnectAction::Disconnected {
                connection,
                event_reason,
            } => {
                assert_eq!(connection.connection_handle, 0x0800);
                assert_eq!(connection.generation, 7);
                assert_eq!(event_reason, 0x16);
                assert_ne!(
                    event_reason,
                    DISCONNECT_REASON_REMOTE_USER_TERMINATED
                );
            }
            other => panic!("expected disconnected result, got {other:?}"),
        }
    }

    #[test]
    fn disconnect_rejects_invalid_handle_and_nonzero_status() {
        assert!(matches!(
            DisconnectTransaction::begin(ConnectionToken { connection_handle: 0x0f00, generation: 7 }),
            Err(DisconnectConfigError::InvalidConnectionHandle(0x0f00))
        ));

        let (mut tx, _command) =
            DisconnectTransaction::begin(ConnectionToken { connection_handle: 0x0800, generation: 7 }).unwrap();
        match tx.on_command_status(
            bumble_hci::codes::HCI_DISCONNECT_COMMAND,
            0x12,
        ) {
            DisconnectAction::Failed(failure) => {
                assert_eq!(failure.connection.generation, 7);
                assert_eq!(
                    failure.kind,
                    DisconnectFailureKind::CommandStatus {
                        status: 0x12,
                    }
                );
            }
            other => panic!("expected disconnect failure, got {other:?}"),
        }
    }

    #[test]
    fn invalid_request_is_rejected_before_controller_io() {
        let mut invalid = request();
        invalid.peer.address_type = 2;
        assert_eq!(
            FalConnectTransaction::begin(invalid).err(),
            Some(FalConnectConfigError::InvalidAddressType(2))
        );

        let mut invalid = request();
        invalid.parameters.scan_window =
            invalid.parameters.scan_interval + 1;
        assert_eq!(
            FalConnectTransaction::begin(invalid).err(),
            Some(FalConnectConfigError::ScanWindowExceedsInterval)
        );
    }

    #[test]
    fn successful_fal_connect_preserves_experiment_command_policy() {
        let (mut tx, first) =
            FalConnectTransaction::begin(request()).unwrap();
        assert_eq!(
            first.op_code(),
            HCI_LE_READ_FILTER_ACCEPT_LIST_SIZE_COMMAND
        );

        expect_send(
            tx.on_command_complete(first.op_code(), 0, &[8]),
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        );
        let add = expect_send(
            tx.on_command_complete(
                HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
                0,
                &[],
            ),
            HCI_LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST_COMMAND,
        );
        match add {
            Command::LeAddDeviceToFilterAcceptList {
                address_type,
                address,
            } => {
                assert_eq!(address_type, 0);
                assert_eq!(
                    address.address_bytes(),
                    &[0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8]
                );
            }
            other => panic!("unexpected FAL add command: {other:?}"),
        }

        let create = expect_send(
            tx.on_command_complete(
                HCI_LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST_COMMAND,
                0,
                &[],
            ),
            HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
        );
        match create {
            Command::LeExtendedCreateConnection {
                initiator_filter_policy,
                own_address_type,
                peer_address,
                initiating_phys,
                scan_intervals,
                scan_windows,
                connection_interval_mins,
                connection_interval_maxs,
                max_latencies,
                supervision_timeouts,
                ..
            } => {
                assert_eq!(initiator_filter_policy, 1);
                assert_eq!(own_address_type, 0);
                assert_eq!(peer_address.address_bytes(), &[0; 6]);
                assert_eq!(initiating_phys, 1);
                assert_eq!(scan_intervals, vec![0x0010]);
                assert_eq!(scan_windows, vec![0x0010]);
                assert_eq!(connection_interval_mins, vec![24]);
                assert_eq!(connection_interval_maxs, vec![40]);
                assert_eq!(max_latencies, vec![0]);
                assert_eq!(supervision_timeouts, vec![42]);
            }
            other => panic!("unexpected create command: {other:?}"),
        }

        assert!(matches!(
            tx.on_command_status(
                HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
                0,
            ),
            FalConnectAction::AwaitConnection
        ));

        expect_send(
            tx.on_connection_event(ConnectionEvent::Established(
                snapshot(0x0800),
            )),
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        );

        match tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        ) {
            FalConnectAction::Connected(connection) => {
                assert_eq!(connection, snapshot(0x0800));
            }
            other => panic!("expected connected action, got {other:?}"),
        }
    }

    #[test]
    fn create_status_failure_clears_fal_before_reporting_failure() {
        let (mut tx, first) =
            FalConnectTransaction::begin(request()).unwrap();
        let _ = tx.on_command_complete(first.op_code(), 0, &[8]);
        let _ = tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_complete(
            HCI_LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );

        expect_send(
            tx.on_command_status(
                HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
                0x0c,
            ),
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        );

        let failure = match tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        ) {
            FalConnectAction::Failed(failure) => failure,
            other => panic!("expected failure, got {other:?}"),
        };
        assert_eq!(
            failure.kind,
            FalConnectFailureKind::CommandStatus {
                opcode: HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
                status: 0x0c,
            }
        );
    }

    #[test]
    fn accepted_cancel_waits_for_terminal_connection_event() {
        let (mut tx, first) =
            FalConnectTransaction::begin(request()).unwrap();
        let _ = tx.on_command_complete(first.op_code(), 0, &[8]);
        let _ = tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_complete(
            HCI_LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_status(
            HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
            0,
        );

        let cancel = expect_send(
            tx.request_cancel(),
            bumble_hci::codes::HCI_LE_CREATE_CONNECTION_CANCEL_COMMAND,
        );
        assert!(matches!(cancel, Command::LeCreateConnectionCancel));

        assert!(matches!(
            tx.on_command_complete(
                bumble_hci::codes::HCI_LE_CREATE_CONNECTION_CANCEL_COMMAND,
                0,
                &[],
            ),
            FalConnectAction::CancelAccepted
        ));

        expect_send(
            tx.on_connection_event(ConnectionEvent::Failed {
                status: 0x02,
            }),
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        );

        let failure = match tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        ) {
            FalConnectAction::Failed(failure) => failure,
            other => panic!("expected cancelled failure, got {other:?}"),
        };
        assert_eq!(
            failure.kind,
            FalConnectFailureKind::Cancelled {
                connection_status: 0x02,
            }
        );
    }

    #[test]
    fn late_success_while_cancel_pending_is_preserved() {
        let (mut tx, first) =
            FalConnectTransaction::begin(request()).unwrap();
        let _ = tx.on_command_complete(first.op_code(), 0, &[8]);
        let _ = tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_complete(
            HCI_LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_status(
            HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
            0,
        );
        let _ = tx.request_cancel();

        expect_send(
            tx.on_connection_event(ConnectionEvent::Established(
                snapshot(0x0801),
            )),
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        );

        match tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        ) {
            FalConnectAction::ConnectedAfterCancel(connection) => {
                assert_eq!(connection, snapshot(0x0801));
            }
            other => {
                panic!("expected late connected-after-cancel, got {other:?}")
            }
        }
    }

    #[test]
    fn rejected_cancel_keeps_initiation_transaction_alive() {
        let (mut tx, first) =
            FalConnectTransaction::begin(request()).unwrap();
        let _ = tx.on_command_complete(first.op_code(), 0, &[8]);
        let _ = tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_complete(
            HCI_LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_status(
            HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
            0,
        );
        let _ = tx.request_cancel();

        assert!(matches!(
            tx.on_command_complete(
                bumble_hci::codes::HCI_LE_CREATE_CONNECTION_CANCEL_COMMAND,
                0x0c,
                &[],
            ),
            FalConnectAction::CancelRejected { status: 0x0c }
        ));

        expect_send(
            tx.on_connection_event(ConnectionEvent::Established(
                snapshot(0x0802),
            )),
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        );
        match tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        ) {
            FalConnectAction::Connected(connection) => {
                assert_eq!(connection, snapshot(0x0802));
            }
            other => panic!("expected normal connected result, got {other:?}"),
        }
    }

    #[test]
    fn connection_failure_records_fal_cleanup_failure() {
        let (mut tx, first) =
            FalConnectTransaction::begin(request()).unwrap();
        let _ = tx.on_command_complete(first.op_code(), 0, &[8]);
        let _ = tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_complete(
            HCI_LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_status(
            HCI_LE_EXTENDED_CREATE_CONNECTION_COMMAND,
            0,
        );

        let _ = expect_send(
            tx.on_connection_event(ConnectionEvent::Failed {
                status: 0x3e,
            }),
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
        );

        let failure = match tx.on_command_complete(
            HCI_LE_CLEAR_FILTER_ACCEPT_LIST_COMMAND,
            0x0c,
            &[],
        ) {
            FalConnectAction::Failed(failure) => failure,
            other => panic!("expected failure, got {other:?}"),
        };
        assert_eq!(
            failure.kind,
            FalConnectFailureKind::ConnectionFailed { status: 0x3e }
        );
        assert_eq!(failure.cleanup_status, Some(0x0c));
    }
}
