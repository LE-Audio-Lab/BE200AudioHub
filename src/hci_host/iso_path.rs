use bumble_hci::{
    codes::{
        HCI_LE_REMOVE_ISO_DATA_PATH_COMMAND,
        HCI_LE_SETUP_ISO_DATA_PATH_COMMAND,
    },
    CodingFormat, Command,
};

const MAX_CONNECTION_HANDLE: u16 = 0x0eff;
const MAX_U24: u32 = 0x00ff_ffff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IsoDataPathDirection {
    /// Bluetooth Core "Input": Host -> Controller.
    Input = 0,
    /// Bluetooth Core "Output": Controller -> Host.
    Output = 1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoDataPathConfig {
    pub connection_handle: u16,
    pub direction: IsoDataPathDirection,
    pub data_path_id: u8,
    pub codec_id: CodingFormat,
    pub controller_delay: u32,
    pub codec_configuration: Vec<u8>,
}

impl IsoDataPathConfig {
    /// Exact path used by the sealed R5 First Sound implementation.
    pub fn hci_input(connection_handle: u16) -> Self {
        Self {
            connection_handle,
            direction: IsoDataPathDirection::Input,
            data_path_id: 0,
            codec_id: CodingFormat::TRANSPARENT,
            controller_delay: 0,
            codec_configuration: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoPathConfigError {
    InvalidConnectionHandle(u16),
    ControllerDelayTooLarge(u32),
    CodecConfigurationTooLong(usize),
    InvalidRemoveMask(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsoPathFailureKind {
    CommandCompleteStatus { status: u8 },
    MalformedResponse,
    ConnectionHandleMismatch { expected: u16, actual: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoPathFailure {
    pub connection_handle: u16,
    pub may_have_configured: bool,
    pub kind: IsoPathFailureKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsoPathSetupAction {
    Complete {
        connection_handle: u16,
        direction: IsoDataPathDirection,
    },
    Failed(IsoPathFailure),
    Ignored,
}

pub struct IsoPathSetupTransaction {
    config: IsoDataPathConfig,
    terminal: bool,
}

impl IsoPathSetupTransaction {
    pub fn begin(
        config: IsoDataPathConfig,
    ) -> Result<(Self, Command), IsoPathConfigError> {
        validate_setup(&config)?;
        let command = Command::LeSetupIsoDataPath {
            connection_handle: config.connection_handle,
            data_path_direction: config.direction as u8,
            data_path_id: config.data_path_id,
            codec_id: config.codec_id,
            controller_delay: config.controller_delay,
            codec_configuration: config.codec_configuration.clone(),
        };
        Ok((Self { config, terminal: false }, command))
    }

    pub fn on_command_complete(
        &mut self,
        opcode: u16,
        status: u8,
        parameters_after_status: &[u8],
    ) -> IsoPathSetupAction {
        if self.terminal || opcode != HCI_LE_SETUP_ISO_DATA_PATH_COMMAND {
            return IsoPathSetupAction::Ignored;
        }
        self.terminal = true;

        if status != 0 {
            return IsoPathSetupAction::Failed(IsoPathFailure {
                connection_handle: self.config.connection_handle,
                may_have_configured: false,
                kind: IsoPathFailureKind::CommandCompleteStatus { status },
            });
        }

        match parse_echoed_handle(
            self.config.connection_handle,
            parameters_after_status,
        ) {
            Ok(connection_handle) => IsoPathSetupAction::Complete {
                connection_handle,
                direction: self.config.direction,
            },
            Err(kind) => IsoPathSetupAction::Failed(IsoPathFailure {
                connection_handle: self.config.connection_handle,
                may_have_configured: true,
                kind,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsoPathRemoveAction {
    Complete {
        connection_handle: u16,
        directions: u8,
    },
    Failed(IsoPathFailure),
    Ignored,
}

pub struct IsoPathRemoveTransaction {
    connection_handle: u16,
    directions: u8,
    terminal: bool,
}

impl IsoPathRemoveTransaction {
    pub fn begin(
        connection_handle: u16,
        directions: u8,
    ) -> Result<(Self, Command), IsoPathConfigError> {
        validate_handle(connection_handle)?;
        if directions == 0 || directions & !0x03 != 0 {
            return Err(IsoPathConfigError::InvalidRemoveMask(directions));
        }

        Ok((
            Self {
                connection_handle,
                directions,
                terminal: false,
            },
            Command::LeRemoveIsoDataPath {
                connection_handle,
                data_path_direction: directions,
            },
        ))
    }

    pub fn on_command_complete(
        &mut self,
        opcode: u16,
        status: u8,
        parameters_after_status: &[u8],
    ) -> IsoPathRemoveAction {
        if self.terminal || opcode != HCI_LE_REMOVE_ISO_DATA_PATH_COMMAND {
            return IsoPathRemoveAction::Ignored;
        }
        self.terminal = true;

        if status != 0 {
            return IsoPathRemoveAction::Failed(IsoPathFailure {
                connection_handle: self.connection_handle,
                may_have_configured: true,
                kind: IsoPathFailureKind::CommandCompleteStatus { status },
            });
        }

        match parse_echoed_handle(
            self.connection_handle,
            parameters_after_status,
        ) {
            Ok(connection_handle) => IsoPathRemoveAction::Complete {
                connection_handle,
                directions: self.directions,
            },
            Err(kind) => IsoPathRemoveAction::Failed(IsoPathFailure {
                connection_handle: self.connection_handle,
                may_have_configured: true,
                kind,
            }),
        }
    }
}

fn validate_setup(config: &IsoDataPathConfig) -> Result<(), IsoPathConfigError> {
    validate_handle(config.connection_handle)?;
    if config.controller_delay > MAX_U24 {
        return Err(IsoPathConfigError::ControllerDelayTooLarge(
            config.controller_delay,
        ));
    }
    if config.codec_configuration.len() > usize::from(u8::MAX) {
        return Err(IsoPathConfigError::CodecConfigurationTooLong(
            config.codec_configuration.len(),
        ));
    }
    Ok(())
}

fn validate_handle(connection_handle: u16) -> Result<(), IsoPathConfigError> {
    if connection_handle > MAX_CONNECTION_HANDLE {
        Err(IsoPathConfigError::InvalidConnectionHandle(connection_handle))
    } else {
        Ok(())
    }
}

fn parse_echoed_handle(
    expected: u16,
    parameters_after_status: &[u8],
) -> Result<u16, IsoPathFailureKind> {
    let [lo, hi] = parameters_after_status else {
        return Err(IsoPathFailureKind::MalformedResponse);
    };
    let actual = u16::from_le_bytes([*lo, *hi]);
    if actual != expected {
        return Err(IsoPathFailureKind::ConnectionHandleMismatch {
            expected,
            actual,
        });
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hci_input_matches_sealed_first_sound_path() {
        let config = IsoDataPathConfig::hci_input(0x0900);
        let (_transaction, command) =
            IsoPathSetupTransaction::begin(config).unwrap();

        assert_eq!(
            command,
            Command::LeSetupIsoDataPath {
                connection_handle: 0x0900,
                data_path_direction: 0,
                data_path_id: 0,
                codec_id: CodingFormat::TRANSPARENT,
                controller_delay: 0,
                codec_configuration: vec![],
            }
        );
    }

    #[test]
    fn setup_requires_success_and_matching_echoed_handle() {
        let (mut transaction, _) = IsoPathSetupTransaction::begin(
            IsoDataPathConfig::hci_input(0x0900),
        )
        .unwrap();

        assert_eq!(
            transaction.on_command_complete(
                HCI_LE_SETUP_ISO_DATA_PATH_COMMAND,
                0,
                &[0x00, 0x09],
            ),
            IsoPathSetupAction::Complete {
                connection_handle: 0x0900,
                direction: IsoDataPathDirection::Input,
            }
        );
    }

    #[test]
    fn malformed_setup_success_is_uncertain_not_clean_failure() {
        let (mut transaction, _) = IsoPathSetupTransaction::begin(
            IsoDataPathConfig::hci_input(0x0900),
        )
        .unwrap();

        assert!(matches!(
            transaction.on_command_complete(
                HCI_LE_SETUP_ISO_DATA_PATH_COMMAND,
                0,
                &[],
            ),
            IsoPathSetupAction::Failed(IsoPathFailure {
                connection_handle: 0x0900,
                may_have_configured: true,
                kind: IsoPathFailureKind::MalformedResponse,
            })
        ));
    }

    #[test]
    fn remove_uses_direction_bitmask_not_setup_enum() {
        let (_transaction, command) =
            IsoPathRemoveTransaction::begin(0x0900, 0x01).unwrap();
        assert_eq!(
            command,
            Command::LeRemoveIsoDataPath {
                connection_handle: 0x0900,
                data_path_direction: 0x01,
            }
        );
        assert_eq!(
            IsoPathRemoveTransaction::begin(0x0900, 0).err(),
            Some(IsoPathConfigError::InvalidRemoveMask(0))
        );
        assert_eq!(
            IsoPathRemoveTransaction::begin(0x0900, 0x04).err(),
            Some(IsoPathConfigError::InvalidRemoveMask(0x04))
        );
    }

    #[test]
    fn remove_failure_keeps_path_state_uncertain() {
        let (mut transaction, _) =
            IsoPathRemoveTransaction::begin(0x0900, 0x01).unwrap();

        assert!(matches!(
            transaction.on_command_complete(
                HCI_LE_REMOVE_ISO_DATA_PATH_COMMAND,
                0x0c,
                &[0x00, 0x09],
            ),
            IsoPathRemoveAction::Failed(IsoPathFailure {
                connection_handle: 0x0900,
                may_have_configured: true,
                kind: IsoPathFailureKind::CommandCompleteStatus {
                    status: 0x0c,
                },
            })
        ));
    }
}
