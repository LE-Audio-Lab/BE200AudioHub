use bumble_hci::{
    codes::HCI_LE_REMOVE_CIG_COMMAND,
    Command,
};

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

#[cfg(test)]
mod tests {
    use super::*;

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
