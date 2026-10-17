use bumble_hci::{
    codes::{HCI_DISCONNECT_COMMAND, HCI_LE_REMOVE_CIG_COMMAND},
    Command,
};

pub const CIS_DISCONNECT_REASON_REMOTE_USER_TERMINATED: u8 = 0x13;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CigRemoveFailureKind {
    CommandCompleteStatus { status: u8 },
    MalformedResponse,
    CigIdMismatch { expected: u8, actual: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CigRemoveFailure {
    pub cig_id: u8,
    pub kind: CigRemoveFailureKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CigRemoveState {
    Idle,
    Removing { cig_id: u8 },
    Removed { cig_id: u8 },
    Failed(CigRemoveFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CigRemoveAction {
    Complete { cig_id: u8 },
    Failed(CigRemoveFailure),
    Ignored,
}

pub struct CigRemoveTransaction {
    cig_id: u8,
    terminal: bool,
}

impl CigRemoveTransaction {
    pub fn begin(cig_id: u8) -> (Self, Command) {
        (
            Self {
                cig_id,
                terminal: false,
            },
            Command::LeRemoveCig { cig_id },
        )
    }

    pub fn on_command_complete(
        &mut self,
        opcode: u16,
        status: u8,
        parameters_after_status: &[u8],
    ) -> CigRemoveAction {
        if self.terminal || opcode != HCI_LE_REMOVE_CIG_COMMAND {
            return CigRemoveAction::Ignored;
        }

        self.terminal = true;

        if status != 0 {
            return CigRemoveAction::Failed(CigRemoveFailure {
                cig_id: self.cig_id,
                kind: CigRemoveFailureKind::CommandCompleteStatus { status },
            });
        }

        let [actual] = parameters_after_status else {
            return CigRemoveAction::Failed(CigRemoveFailure {
                cig_id: self.cig_id,
                kind: CigRemoveFailureKind::MalformedResponse,
            });
        };

        if *actual != self.cig_id {
            return CigRemoveAction::Failed(CigRemoveFailure {
                cig_id: self.cig_id,
                kind: CigRemoveFailureKind::CigIdMismatch {
                    expected: self.cig_id,
                    actual: *actual,
                },
            });
        }

        CigRemoveAction::Complete {
            cig_id: self.cig_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CisDisconnectFailureKind {
    CommandStatus { status: u8 },
    DisconnectionEventStatus { status: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CisDisconnectFailure {
    pub connection_handle: u16,
    pub kind: CisDisconnectFailureKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CisDisconnectState {
    Idle,
    Submitting { connection_handle: u16 },
    AwaitingEvent { connection_handle: u16 },
    Disconnected {
        connection_handle: u16,
        event_reason: u8,
    },
    Failed(CisDisconnectFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CisDisconnectEvent {
    Disconnected {
        connection_handle: u16,
        reason: u8,
    },
    Failed {
        connection_handle: u16,
        status: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CisDisconnectAction {
    AwaitEvent { connection_handle: u16 },
    Disconnected {
        connection_handle: u16,
        event_reason: u8,
    },
    Failed(CisDisconnectFailure),
    Ignored,
}

pub struct CisDisconnectTransaction {
    connection_handle: u16,
    awaiting_event: bool,
    terminal: bool,
}

impl CisDisconnectTransaction {
    pub fn begin(connection_handle: u16) -> (Self, Command) {
        (
            Self {
                connection_handle,
                awaiting_event: false,
                terminal: false,
            },
            Command::Disconnect {
                connection_handle,
                reason: CIS_DISCONNECT_REASON_REMOTE_USER_TERMINATED,
            },
        )
    }

    pub const fn connection_handle(&self) -> u16 {
        self.connection_handle
    }

    pub fn on_command_status(
        &mut self,
        opcode: u16,
        status: u8,
    ) -> CisDisconnectAction {
        if self.terminal || opcode != HCI_DISCONNECT_COMMAND {
            return CisDisconnectAction::Ignored;
        }

        if status != 0 {
            self.terminal = true;
            return CisDisconnectAction::Failed(CisDisconnectFailure {
                connection_handle: self.connection_handle,
                kind: CisDisconnectFailureKind::CommandStatus { status },
            });
        }

        self.awaiting_event = true;
        CisDisconnectAction::AwaitEvent {
            connection_handle: self.connection_handle,
        }
    }

    pub fn on_event(
        &mut self,
        event: CisDisconnectEvent,
    ) -> CisDisconnectAction {
        if self.terminal || !self.awaiting_event {
            return CisDisconnectAction::Ignored;
        }

        match event {
            CisDisconnectEvent::Disconnected {
                connection_handle,
                reason,
            } if connection_handle == self.connection_handle => {
                self.terminal = true;
                CisDisconnectAction::Disconnected {
                    connection_handle,
                    event_reason: reason,
                }
            }
            CisDisconnectEvent::Failed {
                connection_handle,
                status,
            } if connection_handle == self.connection_handle => {
                self.terminal = true;
                CisDisconnectAction::Failed(CisDisconnectFailure {
                    connection_handle,
                    kind: CisDisconnectFailureKind::DisconnectionEventStatus {
                        status,
                    },
                })
            }
            _ => CisDisconnectAction::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cis_disconnect_command_uses_remote_user_terminated_reason() {
        let (_transaction, command) =
            CisDisconnectTransaction::begin(0x0900);
        assert_eq!(
            command,
            Command::Disconnect {
                connection_handle: 0x0900,
                reason: 0x13,
            }
        );
    }

    #[test]
    fn cis_disconnect_accepts_local_0x16_event_after_0x13_command() {
        let (mut transaction, _) =
            CisDisconnectTransaction::begin(0x0900);
        assert_eq!(
            transaction.on_command_status(HCI_DISCONNECT_COMMAND, 0),
            CisDisconnectAction::AwaitEvent {
                connection_handle: 0x0900,
            }
        );
        assert_eq!(
            transaction.on_event(CisDisconnectEvent::Disconnected {
                connection_handle: 0x0900,
                reason: 0x16,
            }),
            CisDisconnectAction::Disconnected {
                connection_handle: 0x0900,
                event_reason: 0x16,
            }
        );
    }

    #[test]
    fn cis_disconnect_failure_and_unrelated_events_are_explicit() {
        let (mut rejected, _) =
            CisDisconnectTransaction::begin(0x0900);
        assert!(matches!(
            rejected.on_command_status(HCI_DISCONNECT_COMMAND, 0x12),
            CisDisconnectAction::Failed(CisDisconnectFailure {
                connection_handle: 0x0900,
                kind: CisDisconnectFailureKind::CommandStatus {
                    status: 0x12,
                },
            })
        ));

        let (mut transaction, _) =
            CisDisconnectTransaction::begin(0x0900);
        assert_eq!(
            transaction.on_command_status(HCI_DISCONNECT_COMMAND, 0),
            CisDisconnectAction::AwaitEvent {
                connection_handle: 0x0900,
            }
        );
        assert_eq!(
            transaction.on_event(CisDisconnectEvent::Disconnected {
                connection_handle: 0x0901,
                reason: 0x16,
            }),
            CisDisconnectAction::Ignored
        );
        assert!(matches!(
            transaction.on_event(CisDisconnectEvent::Failed {
                connection_handle: 0x0900,
                status: 0x01,
            }),
            CisDisconnectAction::Failed(CisDisconnectFailure {
                connection_handle: 0x0900,
                kind: CisDisconnectFailureKind::DisconnectionEventStatus {
                    status: 0x01,
                },
            })
        ));
    }

    #[test]
    fn remove_cig_builds_typed_command() {
        let (_transaction, command) = CigRemoveTransaction::begin(7);
        assert_eq!(
            command,
            Command::LeRemoveCig { cig_id: 7 }
        );
        assert_eq!(command.op_code(), HCI_LE_REMOVE_CIG_COMMAND);
    }

    #[test]
    fn remove_cig_requires_success_and_matching_echoed_id() {
        let (mut transaction, _) = CigRemoveTransaction::begin(7);
        assert_eq!(
            transaction.on_command_complete(
                HCI_LE_REMOVE_CIG_COMMAND,
                0,
                &[7],
            ),
            CigRemoveAction::Complete { cig_id: 7 }
        );
        assert_eq!(
            transaction.on_command_complete(
                HCI_LE_REMOVE_CIG_COMMAND,
                0,
                &[7],
            ),
            CigRemoveAction::Ignored
        );
    }

    #[test]
    fn remove_cig_rejects_status_malformed_and_wrong_id() {
        let (mut status, _) = CigRemoveTransaction::begin(7);
        assert_eq!(
            status.on_command_complete(
                HCI_LE_REMOVE_CIG_COMMAND,
                0x0c,
                &[7],
            ),
            CigRemoveAction::Failed(CigRemoveFailure {
                cig_id: 7,
                kind: CigRemoveFailureKind::CommandCompleteStatus {
                    status: 0x0c,
                },
            })
        );

        let (mut malformed, _) = CigRemoveTransaction::begin(7);
        assert!(matches!(
            malformed.on_command_complete(
                HCI_LE_REMOVE_CIG_COMMAND,
                0,
                &[],
            ),
            CigRemoveAction::Failed(CigRemoveFailure {
                kind: CigRemoveFailureKind::MalformedResponse,
                ..
            })
        ));

        let (mut mismatch, _) = CigRemoveTransaction::begin(7);
        assert_eq!(
            mismatch.on_command_complete(
                HCI_LE_REMOVE_CIG_COMMAND,
                0,
                &[8],
            ),
            CigRemoveAction::Failed(CigRemoveFailure {
                cig_id: 7,
                kind: CigRemoveFailureKind::CigIdMismatch {
                    expected: 7,
                    actual: 8,
                },
            })
        );
    }

    #[test]
    fn remove_cig_ignores_unrelated_completion() {
        let (mut transaction, _) = CigRemoveTransaction::begin(7);
        assert_eq!(
            transaction.on_command_complete(0x0c03, 0, &[]),
            CigRemoveAction::Ignored
        );
        assert_eq!(
            transaction.on_command_complete(
                HCI_LE_REMOVE_CIG_COMMAND,
                0,
                &[7],
            ),
            CigRemoveAction::Complete { cig_id: 7 }
        );
    }
}
