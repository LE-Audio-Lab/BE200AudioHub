use std::collections::BTreeSet;

use bumble::{Address, AddressType};
use bumble_hci::{
    codes::{
        HCI_LE_ADD_DEVICE_TO_RESOLVING_LIST_COMMAND,
        HCI_LE_CLEAR_RESOLVING_LIST_COMMAND,
        HCI_LE_READ_RESOLVING_LIST_SIZE_COMMAND,
        HCI_LE_SET_ADDRESS_RESOLUTION_ENABLE_COMMAND,
        HCI_LE_SET_RESOLVABLE_PRIVATE_ADDRESS_TIMEOUT_COMMAND,
    },
    Command,
};

pub const DEFAULT_RPA_TIMEOUT_SECONDS: u16 = 900;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvingPeer {
    /// Bluetooth address bytes in HCI/wire order (little-endian).
    pub identity_address_le: [u8; 6],
    /// Public=0, Random=1. Identity semantics are carried by this request,
    /// not inferred from an advertisement string.
    pub identity_address_type: u8,
    pub peer_irk: [u8; 16],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvingListConfig {
    pub peers: Vec<ResolvingPeer>,
    pub local_irk: [u8; 16],
    pub rpa_timeout_seconds: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivacyConfigError {
    EmptyPeerSet,
    TooManyPeers,
    InvalidAddressType(u8),
    DuplicatePeer,
    ZeroRpaTimeout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrivacyFailureKind {
    CommandStatus { opcode: u16, status: u8 },
    MalformedCapacityResponse,
    CapacityTooSmall { capacity: u8, required: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivacyFailure {
    pub kind: PrivacyFailureKind,
    pub rollback_disable_status: Option<u8>,
    pub rollback_clear_status: Option<u8>,
}

#[derive(Debug)]
pub enum PrivacyAction {
    Send(Command),
    Complete,
    Failed(PrivacyFailure),
    Ignored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Disabling,
    Clearing,
    ReadingCapacity,
    Adding(usize),
    SettingTimeout,
    Enabling,
    RollbackDisable,
    RollbackClear,
    Complete,
    Failed,
}

pub struct PrivacyTransaction {
    config: ResolvingListConfig,
    phase: Phase,
    pending_opcode: u16,
    failure: Option<PrivacyFailure>,
}

impl PrivacyTransaction {
    pub fn begin(
        config: ResolvingListConfig,
    ) -> Result<(Self, Command), PrivacyConfigError> {
        validate_config(&config)?;

        let command = set_resolution(false);
        Ok((
            Self {
                config,
                phase: Phase::Disabling,
                pending_opcode: command.op_code(),
                failure: None,
            },
            command,
        ))
    }

    pub fn pending_opcode(&self) -> u16 {
        self.pending_opcode
    }

    pub fn on_command_complete(
        &mut self,
        opcode: u16,
        status: u8,
        parameters_after_status: &[u8],
    ) -> PrivacyAction {
        if opcode != self.pending_opcode
            || matches!(self.phase, Phase::Complete | Phase::Failed)
        {
            return PrivacyAction::Ignored;
        }

        match self.phase {
            Phase::RollbackDisable => {
                if status != 0 {
                    self.failure
                        .as_mut()
                        .expect("rollback always has a primary failure")
                        .rollback_disable_status = Some(status);
                }
                self.phase = Phase::RollbackClear;
                let command = Command::LeClearResolvingList;
                self.pending_opcode = command.op_code();
                PrivacyAction::Send(command)
            }
            Phase::RollbackClear => {
                if status != 0 {
                    self.failure
                        .as_mut()
                        .expect("rollback always has a primary failure")
                        .rollback_clear_status = Some(status);
                }
                self.phase = Phase::Failed;
                PrivacyAction::Failed(
                    self.failure
                        .take()
                        .expect("rollback always has a primary failure"),
                )
            }
            _ if status != 0 => self.start_rollback(
                PrivacyFailureKind::CommandStatus { opcode, status },
            ),
            Phase::Disabling => {
                self.phase = Phase::Clearing;
                self.send(Command::LeClearResolvingList)
            }
            Phase::Clearing => {
                self.phase = Phase::ReadingCapacity;
                self.send(Command::LeReadResolvingListSize)
            }
            Phase::ReadingCapacity => {
                if parameters_after_status.len() != 1 {
                    return self.start_rollback(
                        PrivacyFailureKind::MalformedCapacityResponse,
                    );
                }

                let capacity = parameters_after_status[0];
                if usize::from(capacity) < self.config.peers.len() {
                    return self.start_rollback(
                        PrivacyFailureKind::CapacityTooSmall {
                            capacity,
                            required: self.config.peers.len(),
                        },
                    );
                }

                self.phase = Phase::Adding(0);
                self.send(self.add_peer_command(0))
            }
            Phase::Adding(index) => {
                let next = index + 1;
                if next < self.config.peers.len() {
                    self.phase = Phase::Adding(next);
                    self.send(self.add_peer_command(next))
                } else {
                    self.phase = Phase::SettingTimeout;
                    self.send(Command::LeSetResolvablePrivateAddressTimeout {
                        rpa_timeout: self.config.rpa_timeout_seconds,
                    })
                }
            }
            Phase::SettingTimeout => {
                self.phase = Phase::Enabling;
                self.send(set_resolution(true))
            }
            Phase::Enabling => {
                self.phase = Phase::Complete;
                self.pending_opcode = 0;
                PrivacyAction::Complete
            }
            Phase::Complete
            | Phase::Failed
            | Phase::RollbackDisable
            | Phase::RollbackClear => PrivacyAction::Ignored,
        }
    }

    fn add_peer_command(&self, index: usize) -> Command {
        let peer = &self.config.peers[index];
        Command::LeAddDeviceToResolvingList {
            peer_identity_address_type: peer.identity_address_type,
            peer_identity_address: Address::from_bytes(
                peer.identity_address_le,
                AddressType(peer.identity_address_type),
            ),
            peer_irk: peer.peer_irk,
            local_irk: self.config.local_irk,
        }
    }

    fn start_rollback(&mut self, kind: PrivacyFailureKind) -> PrivacyAction {
        self.failure = Some(PrivacyFailure {
            kind,
            rollback_disable_status: None,
            rollback_clear_status: None,
        });
        self.phase = Phase::RollbackDisable;
        self.send(set_resolution(false))
    }

    fn send(&mut self, command: Command) -> PrivacyAction {
        self.pending_opcode = command.op_code();
        PrivacyAction::Send(command)
    }
}

fn set_resolution(enabled: bool) -> Command {
    Command::LeSetAddressResolutionEnable {
        address_resolution_enable: u8::from(enabled),
    }
}

fn validate_config(config: &ResolvingListConfig) -> Result<(), PrivacyConfigError> {
    if config.peers.is_empty() {
        return Err(PrivacyConfigError::EmptyPeerSet);
    }
    if config.peers.len() > usize::from(u8::MAX) {
        return Err(PrivacyConfigError::TooManyPeers);
    }
    if config.rpa_timeout_seconds == 0 {
        return Err(PrivacyConfigError::ZeroRpaTimeout);
    }

    let mut identities = BTreeSet::new();
    for peer in &config.peers {
        if !matches!(peer.identity_address_type, 0 | 1) {
            return Err(PrivacyConfigError::InvalidAddressType(
                peer.identity_address_type,
            ));
        }
        if !identities.insert((
            peer.identity_address_type,
            peer.identity_address_le,
        )) {
            return Err(PrivacyConfigError::DuplicatePeer);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(address_last_byte: u8) -> ResolvingPeer {
        ResolvingPeer {
            identity_address_le: [address_last_byte, 2, 3, 4, 5, 6],
            identity_address_type: 0,
            peer_irk: [address_last_byte; 16],
        }
    }

    fn config(peers: Vec<ResolvingPeer>) -> ResolvingListConfig {
        ResolvingListConfig {
            peers,
            local_irk: [0x44; 16],
            rpa_timeout_seconds: DEFAULT_RPA_TIMEOUT_SECONDS,
        }
    }

    fn expect_send(action: PrivacyAction, opcode: u16) -> Command {
        match action {
            PrivacyAction::Send(command) => {
                assert_eq!(command.op_code(), opcode);
                command
            }
            other => panic!("expected Send({opcode:#06x}), got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_config_before_any_controller_command() {
        assert_eq!(
            PrivacyTransaction::begin(config(Vec::new())).err(),
            Some(PrivacyConfigError::EmptyPeerSet)
        );

        let mut invalid = peer(1);
        invalid.identity_address_type = 2;
        assert_eq!(
            PrivacyTransaction::begin(config(vec![invalid])).err(),
            Some(PrivacyConfigError::InvalidAddressType(2))
        );

        assert_eq!(
            PrivacyTransaction::begin(config(vec![peer(1), peer(1)])).err(),
            Some(PrivacyConfigError::DuplicatePeer)
        );
    }

    #[test]
    fn successful_transaction_preserves_controller_privacy_order() {
        let (mut tx, first) =
            PrivacyTransaction::begin(config(vec![peer(1), peer(2)])).unwrap();
        assert_eq!(
            first.op_code(),
            HCI_LE_SET_ADDRESS_RESOLUTION_ENABLE_COMMAND
        );

        expect_send(
            tx.on_command_complete(first.op_code(), 0, &[]),
            HCI_LE_CLEAR_RESOLVING_LIST_COMMAND,
        );
        expect_send(
            tx.on_command_complete(HCI_LE_CLEAR_RESOLVING_LIST_COMMAND, 0, &[]),
            HCI_LE_READ_RESOLVING_LIST_SIZE_COMMAND,
        );

        let first_add = expect_send(
            tx.on_command_complete(
                HCI_LE_READ_RESOLVING_LIST_SIZE_COMMAND,
                0,
                &[2],
            ),
            HCI_LE_ADD_DEVICE_TO_RESOLVING_LIST_COMMAND,
        );
        match first_add {
            Command::LeAddDeviceToResolvingList {
                peer_identity_address_type,
                peer_identity_address,
                peer_irk,
                local_irk,
            } => {
                assert_eq!(peer_identity_address_type, 0);
                assert_eq!(
                    peer_identity_address.address_bytes(),
                    &[1, 2, 3, 4, 5, 6]
                );
                assert_eq!(peer_irk, [1; 16]);
                assert_eq!(local_irk, [0x44; 16]);
            }
            other => panic!("unexpected first add command: {other:?}"),
        }

        expect_send(
            tx.on_command_complete(
                HCI_LE_ADD_DEVICE_TO_RESOLVING_LIST_COMMAND,
                0,
                &[],
            ),
            HCI_LE_ADD_DEVICE_TO_RESOLVING_LIST_COMMAND,
        );
        expect_send(
            tx.on_command_complete(
                HCI_LE_ADD_DEVICE_TO_RESOLVING_LIST_COMMAND,
                0,
                &[],
            ),
            HCI_LE_SET_RESOLVABLE_PRIVATE_ADDRESS_TIMEOUT_COMMAND,
        );
        expect_send(
            tx.on_command_complete(
                HCI_LE_SET_RESOLVABLE_PRIVATE_ADDRESS_TIMEOUT_COMMAND,
                0,
                &[],
            ),
            HCI_LE_SET_ADDRESS_RESOLUTION_ENABLE_COMMAND,
        );

        assert!(matches!(
            tx.on_command_complete(
                HCI_LE_SET_ADDRESS_RESOLUTION_ENABLE_COMMAND,
                0,
                &[],
            ),
            PrivacyAction::Complete
        ));
    }

    #[test]
    fn capacity_failure_rolls_back_disable_then_clear() {
        let (mut tx, first) =
            PrivacyTransaction::begin(config(vec![peer(1), peer(2)])).unwrap();
        let _ = tx.on_command_complete(first.op_code(), 0, &[]);
        let _ = tx.on_command_complete(
            HCI_LE_CLEAR_RESOLVING_LIST_COMMAND,
            0,
            &[],
        );

        expect_send(
            tx.on_command_complete(
                HCI_LE_READ_RESOLVING_LIST_SIZE_COMMAND,
                0,
                &[1],
            ),
            HCI_LE_SET_ADDRESS_RESOLUTION_ENABLE_COMMAND,
        );
        expect_send(
            tx.on_command_complete(
                HCI_LE_SET_ADDRESS_RESOLUTION_ENABLE_COMMAND,
                0,
                &[],
            ),
            HCI_LE_CLEAR_RESOLVING_LIST_COMMAND,
        );

        let failure = match tx.on_command_complete(
            HCI_LE_CLEAR_RESOLVING_LIST_COMMAND,
            0,
            &[],
        ) {
            PrivacyAction::Failed(failure) => failure,
            other => panic!("expected failure, got {other:?}"),
        };
        assert_eq!(
            failure.kind,
            PrivacyFailureKind::CapacityTooSmall {
                capacity: 1,
                required: 2,
            }
        );
        assert_eq!(failure.rollback_disable_status, None);
        assert_eq!(failure.rollback_clear_status, None);
    }

    #[test]
    fn command_failure_rolls_back_and_records_cleanup_failures() {
        let (mut tx, first) =
            PrivacyTransaction::begin(config(vec![peer(1)])).unwrap();
        let _ = tx.on_command_complete(first.op_code(), 0, &[]);
        let _ = tx.on_command_complete(
            HCI_LE_CLEAR_RESOLVING_LIST_COMMAND,
            0,
            &[],
        );
        let _ = tx.on_command_complete(
            HCI_LE_READ_RESOLVING_LIST_SIZE_COMMAND,
            0,
            &[1],
        );

        expect_send(
            tx.on_command_complete(
                HCI_LE_ADD_DEVICE_TO_RESOLVING_LIST_COMMAND,
                0x0c,
                &[],
            ),
            HCI_LE_SET_ADDRESS_RESOLUTION_ENABLE_COMMAND,
        );
        expect_send(
            tx.on_command_complete(
                HCI_LE_SET_ADDRESS_RESOLUTION_ENABLE_COMMAND,
                0x12,
                &[],
            ),
            HCI_LE_CLEAR_RESOLVING_LIST_COMMAND,
        );

        let failure = match tx.on_command_complete(
            HCI_LE_CLEAR_RESOLVING_LIST_COMMAND,
            0x0c,
            &[],
        ) {
            PrivacyAction::Failed(failure) => failure,
            other => panic!("expected failure, got {other:?}"),
        };
        assert_eq!(
            failure.kind,
            PrivacyFailureKind::CommandStatus {
                opcode: HCI_LE_ADD_DEVICE_TO_RESOLVING_LIST_COMMAND,
                status: 0x0c,
            }
        );
        assert_eq!(failure.rollback_disable_status, Some(0x12));
        assert_eq!(failure.rollback_clear_status, Some(0x0c));
    }

    #[test]
    fn unrelated_completion_is_ignored_without_advancing() {
        let (mut tx, first) =
            PrivacyTransaction::begin(config(vec![peer(1)])).unwrap();

        assert!(matches!(
            tx.on_command_complete(0xffff, 0, &[]),
            PrivacyAction::Ignored
        ));
        assert_eq!(tx.pending_opcode(), first.op_code());
    }
}
