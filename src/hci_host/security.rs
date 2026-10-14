use bumble_hci::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredLeKeyMaterial {
    pub long_term_key: [u8; 16],
    pub encrypted_diversifier: u16,
    pub random_number: [u8; 8],
}

impl StoredLeKeyMaterial {
    pub const fn secure_connections(long_term_key: [u8; 16]) -> Self {
        Self {
            long_term_key,
            encrypted_diversifier: 0,
            random_number: [0; 8],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredEncryptionRequest {
    pub connection_handle: u16,
    pub key: StoredLeKeyMaterial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredEncryptionConfigError {
    InvalidConnectionHandle(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredEncryptionFailureKind {
    CommandStatus { status: u8 },
    EncryptionChangeStatus { status: u8 },
    EncryptionNotEnabled,
    ConnectionLost { reason: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredEncryptionFailure {
    pub connection_handle: u16,
    pub kind: StoredEncryptionFailureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredEncryptionState {
    Idle,
    Submitting,
    AwaitingChange,
    Encrypted {
        connection_handle: u16,
        encryption_key_size: u8,
    },
    Failed(StoredEncryptionFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptionEvent {
    Change {
        status: u8,
        connection_handle: u16,
        encryption_enabled: u8,
        encryption_key_size: u8,
    },
    Disconnected {
        connection_handle: u16,
        reason: u8,
    },
}

#[derive(Debug)]
pub enum StoredEncryptionAction {
    AwaitChange,
    Encrypted {
        connection_handle: u16,
        encryption_key_size: u8,
    },
    Failed(StoredEncryptionFailure),
    Ignored,
}

pub struct StoredEncryptionTransaction {
    connection_handle: u16,
    awaiting_change: bool,
    terminal: bool,
}

impl StoredEncryptionTransaction {
    pub fn begin(
        request: StoredEncryptionRequest,
    ) -> Result<(Self, Command), StoredEncryptionConfigError> {
        if request.connection_handle > 0x0eff {
            return Err(
                StoredEncryptionConfigError::InvalidConnectionHandle(
                    request.connection_handle,
                ),
            );
        }

        Ok((
            Self {
                connection_handle: request.connection_handle,
                awaiting_change: false,
                terminal: false,
            },
            Command::LeEnableEncryption {
                connection_handle: request.connection_handle,
                random_number: request.key.random_number,
                encrypted_diversifier: request.key.encrypted_diversifier,
                long_term_key: request.key.long_term_key,
            },
        ))
    }

    pub fn on_command_status(
        &mut self,
        opcode: u16,
        status: u8,
    ) -> StoredEncryptionAction {
        if self.terminal
            || opcode != bumble_hci::codes::HCI_LE_ENABLE_ENCRYPTION_COMMAND
        {
            return StoredEncryptionAction::Ignored;
        }

        if status != 0 {
            self.terminal = true;
            return StoredEncryptionAction::Failed(
                StoredEncryptionFailure {
                    connection_handle: self.connection_handle,
                    kind: StoredEncryptionFailureKind::CommandStatus {
                        status,
                    },
                },
            );
        }

        self.awaiting_change = true;
        StoredEncryptionAction::AwaitChange
    }

    pub fn on_event(
        &mut self,
        event: EncryptionEvent,
    ) -> StoredEncryptionAction {
        if self.terminal || !self.awaiting_change {
            return StoredEncryptionAction::Ignored;
        }

        match event {
            EncryptionEvent::Change {
                status,
                connection_handle,
                encryption_enabled,
                encryption_key_size,
            } => {
                if connection_handle != self.connection_handle {
                    return StoredEncryptionAction::Ignored;
                }

                self.terminal = true;
                if status != 0 {
                    return StoredEncryptionAction::Failed(
                        StoredEncryptionFailure {
                            connection_handle,
                            kind:
                                StoredEncryptionFailureKind::EncryptionChangeStatus {
                                    status,
                                },
                        },
                    );
                }

                if encryption_enabled == 0 {
                    return StoredEncryptionAction::Failed(
                        StoredEncryptionFailure {
                            connection_handle,
                            kind:
                                StoredEncryptionFailureKind::EncryptionNotEnabled,
                        },
                    );
                }

                StoredEncryptionAction::Encrypted {
                    connection_handle,
                    encryption_key_size,
                }
            }
            EncryptionEvent::Disconnected {
                connection_handle,
                reason,
            } => {
                if connection_handle != self.connection_handle {
                    return StoredEncryptionAction::Ignored;
                }

                self.terminal = true;
                StoredEncryptionAction::Failed(StoredEncryptionFailure {
                    connection_handle,
                    kind: StoredEncryptionFailureKind::ConnectionLost {
                        reason,
                    },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_connections_key_uses_zero_ediv_and_rand() {
        let key = StoredLeKeyMaterial::secure_connections([0x5a; 16]);
        assert_eq!(key.encrypted_diversifier, 0);
        assert_eq!(key.random_number, [0; 8]);
        assert_eq!(key.long_term_key, [0x5a; 16]);
    }

    #[test]
    fn command_preserves_stored_legacy_or_sc_parameters() {
        let request = StoredEncryptionRequest {
            connection_handle: 0x0800,
            key: StoredLeKeyMaterial {
                long_term_key: [0x11; 16],
                encrypted_diversifier: 0x2233,
                random_number: [1, 2, 3, 4, 5, 6, 7, 8],
            },
        };

        let (_tx, command) =
            StoredEncryptionTransaction::begin(request).unwrap();

        match command {
            Command::LeEnableEncryption {
                connection_handle,
                random_number,
                encrypted_diversifier,
                long_term_key,
            } => {
                assert_eq!(connection_handle, 0x0800);
                assert_eq!(
                    random_number,
                    [1, 2, 3, 4, 5, 6, 7, 8]
                );
                assert_eq!(encrypted_diversifier, 0x2233);
                assert_eq!(long_term_key, [0x11; 16]);
            }
            other => {
                panic!("expected LE Enable Encryption, got {other:?}")
            }
        }
    }

    #[test]
    fn command_status_then_encryption_change_completes() {
        let (mut tx, _command) =
            StoredEncryptionTransaction::begin(
                StoredEncryptionRequest {
                    connection_handle: 0x0800,
                    key: StoredLeKeyMaterial::secure_connections([0x22; 16]),
                },
            )
            .unwrap();

        assert!(matches!(
            tx.on_command_status(
                bumble_hci::codes::HCI_LE_ENABLE_ENCRYPTION_COMMAND,
                0,
            ),
            StoredEncryptionAction::AwaitChange
        ));

        assert!(matches!(
            tx.on_event(EncryptionEvent::Change {
                status: 0,
                connection_handle: 0x0800,
                encryption_enabled: 1,
                encryption_key_size: 16,
            }),
            StoredEncryptionAction::Encrypted {
                connection_handle: 0x0800,
                encryption_key_size: 16,
            }
        ));
    }

    #[test]
    fn disconnect_while_waiting_for_encryption_is_terminal() {
        let (mut tx, _command) =
            StoredEncryptionTransaction::begin(
                StoredEncryptionRequest {
                    connection_handle: 0x0800,
                    key: StoredLeKeyMaterial::secure_connections([0x66; 16]),
                },
            )
            .unwrap();
        let _ = tx.on_command_status(
            bumble_hci::codes::HCI_LE_ENABLE_ENCRYPTION_COMMAND,
            0,
        );

        match tx.on_event(EncryptionEvent::Disconnected {
            connection_handle: 0x0800,
            reason: 0x08,
        }) {
            StoredEncryptionAction::Failed(failure) => {
                assert_eq!(
                    failure.kind,
                    StoredEncryptionFailureKind::ConnectionLost {
                        reason: 0x08,
                    }
                );
            }
            other => {
                panic!("expected connection-lost failure, got {other:?}")
            }
        }
    }

    #[test]
    fn failure_paths_do_not_claim_encryption() {
        let (mut tx, _command) =
            StoredEncryptionTransaction::begin(
                StoredEncryptionRequest {
                    connection_handle: 0x0800,
                    key: StoredLeKeyMaterial::secure_connections([0x33; 16]),
                },
            )
            .unwrap();

        match tx.on_command_status(
            bumble_hci::codes::HCI_LE_ENABLE_ENCRYPTION_COMMAND,
            0x0c,
        ) {
            StoredEncryptionAction::Failed(failure) => {
                assert_eq!(
                    failure.kind,
                    StoredEncryptionFailureKind::CommandStatus {
                        status: 0x0c,
                    }
                );
            }
            other => panic!("expected command failure, got {other:?}"),
        }

        let (mut tx, _command) =
            StoredEncryptionTransaction::begin(
                StoredEncryptionRequest {
                    connection_handle: 0x0801,
                    key: StoredLeKeyMaterial::secure_connections([0x44; 16]),
                },
            )
            .unwrap();
        let _ = tx.on_command_status(
            bumble_hci::codes::HCI_LE_ENABLE_ENCRYPTION_COMMAND,
            0,
        );

        match tx.on_event(EncryptionEvent::Change {
            status: 0,
            connection_handle: 0x0801,
            encryption_enabled: 0,
            encryption_key_size: 0,
        }) {
            StoredEncryptionAction::Failed(failure) => {
                assert_eq!(
                    failure.kind,
                    StoredEncryptionFailureKind::EncryptionNotEnabled
                );
            }
            other => {
                panic!("expected encryption-disabled failure, got {other:?}")
            }
        }
    }
}
