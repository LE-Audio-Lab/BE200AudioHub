use std::collections::BTreeSet;

use bumble_hci::{
    codes::{
        HCI_LE_CREATE_CIS_COMMAND,
        HCI_LE_SET_CIG_PARAMETERS_COMMAND,
    },
    Command,
};

use super::connection::ConnectionToken;

const MAX_U24: u32 = 0x00ff_ffff;
const MAX_CONNECTION_HANDLE: u16 = 0x0eff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CisConfig {
    pub cis_id: u8,
    pub max_sdu_c_to_p: u16,
    pub max_sdu_p_to_c: u16,
    pub phy_c_to_p: u8,
    pub phy_p_to_c: u8,
    pub rtn_c_to_p: u8,
    pub rtn_p_to_c: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CigConfig {
    pub cig_id: u8,
    pub sdu_interval_c_to_p: u32,
    pub sdu_interval_p_to_c: u32,
    pub worst_case_sca: u8,
    pub packing: u8,
    pub framing: u8,
    pub max_transport_latency_c_to_p: u16,
    pub max_transport_latency_p_to_c: u16,
    pub cis: Vec<CisConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CigConfigError {
    EmptyCisSet,
    TooManyCis,
    DuplicateCisId(u8),
    SduIntervalTooLarge { direction: &'static str, value: u32 },
    InvalidPacking(u8),
    InvalidFraming(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CisAllocation {
    pub cis_id: u8,
    pub connection_handle: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CigAllocation {
    pub cig_id: u8,
    pub cis: Vec<CisAllocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CigConfigureFailureKind {
    CommandCompleteStatus { status: u8 },
    MalformedResponse,
    CigIdMismatch { expected: u8, actual: u8 },
    CisCountMismatch { expected: usize, actual: usize },
    InvalidConnectionHandle(u16),
    DuplicateConnectionHandle(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CigConfigureFailure {
    pub cig_id: u8,
    /// A successful controller status means allocation may already exist even
    /// if the returned payload is malformed and cannot be trusted.
    pub may_have_allocated: bool,
    pub kind: CigConfigureFailureKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CigConfigureState {
    Idle,
    Configuring,
    Ready(CigAllocation),
    Failed(CigConfigureFailure),
}

#[derive(Debug)]
pub enum CigConfigureAction {
    Complete(CigAllocation),
    Failed(CigConfigureFailure),
    Ignored,
}

pub struct CigConfigureTransaction {
    config: CigConfig,
    terminal: bool,
}

impl CigConfigureTransaction {
    pub fn begin(
        config: CigConfig,
    ) -> Result<(Self, Command), CigConfigError> {
        validate_cig_config(&config)?;
        let command = set_cig_command(&config);
        Ok((
            Self {
                config,
                terminal: false,
            },
            command,
        ))
    }

    pub fn on_command_complete(
        &mut self,
        opcode: u16,
        status: u8,
        parameters_after_status: &[u8],
    ) -> CigConfigureAction {
        if self.terminal || opcode != HCI_LE_SET_CIG_PARAMETERS_COMMAND {
            return CigConfigureAction::Ignored;
        }

        if status != 0 {
            self.terminal = true;
            return CigConfigureAction::Failed(CigConfigureFailure {
                cig_id: self.config.cig_id,
                may_have_allocated: false,
                kind: CigConfigureFailureKind::CommandCompleteStatus {
                    status,
                },
            });
        }

        let result = parse_allocation(&self.config, parameters_after_status);
        self.terminal = true;
        match result {
            Ok(allocation) => CigConfigureAction::Complete(allocation),
            Err(kind) => CigConfigureAction::Failed(CigConfigureFailure {
                cig_id: self.config.cig_id,
                may_have_allocated: true,
                kind,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CisAclBinding {
    pub cis_id: u8,
    pub acl: ConnectionToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CisCreateConfigError {
    EmptyAllocation,
    BindingCountMismatch { expected: usize, actual: usize },
    DuplicateBinding(u8),
    MissingBinding(u8),
    UnknownBinding(u8),
    DuplicateCisHandle(u16),
    CisHandleEqualsAclHandle(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CisCreateFailureKind {
    CommandStatus { status: u8 },
    DuplicateCommandStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CisCreateFailure {
    pub cig_id: u8,
    pub kind: CisCreateFailureKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CisCreateAction {
    AwaitingEstablished,
    Failed(CisCreateFailure),
    Ignored,
}

pub struct CisCreateTransaction {
    allocation: CigAllocation,
    bindings: Vec<CisAclBinding>,
    command_accepted: bool,
    terminal: bool,
}

impl CisCreateTransaction {
    pub fn begin(
        allocation: CigAllocation,
        bindings: Vec<CisAclBinding>,
    ) -> Result<(Self, Command), CisCreateConfigError> {
        validate_cis_create(&allocation, &bindings)?;
        let command = create_cis_command(&allocation, &bindings);
        Ok((
            Self {
                allocation,
                bindings,
                command_accepted: false,
                terminal: false,
            },
            command,
        ))
    }

    pub fn on_command_status(
        &mut self,
        opcode: u16,
        status: u8,
    ) -> CisCreateAction {
        if self.terminal || opcode != HCI_LE_CREATE_CIS_COMMAND {
            return CisCreateAction::Ignored;
        }
        if self.command_accepted {
            self.terminal = true;
            return CisCreateAction::Failed(CisCreateFailure {
                cig_id: self.allocation.cig_id,
                kind: CisCreateFailureKind::DuplicateCommandStatus,
            });
        }
        if status != 0 {
            self.terminal = true;
            return CisCreateAction::Failed(CisCreateFailure {
                cig_id: self.allocation.cig_id,
                kind: CisCreateFailureKind::CommandStatus { status },
            });
        }

        self.command_accepted = true;
        CisCreateAction::AwaitingEstablished
    }

    pub fn allocation(&self) -> &CigAllocation {
        &self.allocation
    }

    pub fn bindings(&self) -> &[CisAclBinding] {
        &self.bindings
    }
}

fn validate_cis_create(
    allocation: &CigAllocation,
    bindings: &[CisAclBinding],
) -> Result<(), CisCreateConfigError> {
    if allocation.cis.is_empty() {
        return Err(CisCreateConfigError::EmptyAllocation);
    }
    if bindings.len() != allocation.cis.len() {
        return Err(CisCreateConfigError::BindingCountMismatch {
            expected: allocation.cis.len(),
            actual: bindings.len(),
        });
    }

    let allocation_ids =
        allocation.cis.iter().map(|cis| cis.cis_id).collect::<BTreeSet<_>>();
    let mut binding_ids = BTreeSet::new();
    for binding in bindings {
        if !binding_ids.insert(binding.cis_id) {
            return Err(CisCreateConfigError::DuplicateBinding(
                binding.cis_id,
            ));
        }
        if !allocation_ids.contains(&binding.cis_id) {
            return Err(CisCreateConfigError::UnknownBinding(
                binding.cis_id,
            ));
        }
    }
    for cis in &allocation.cis {
        if !binding_ids.contains(&cis.cis_id) {
            return Err(CisCreateConfigError::MissingBinding(cis.cis_id));
        }
    }

    let mut cis_handles = BTreeSet::new();
    for cis in &allocation.cis {
        if !cis_handles.insert(cis.connection_handle) {
            return Err(CisCreateConfigError::DuplicateCisHandle(
                cis.connection_handle,
            ));
        }
        let acl = bindings
            .iter()
            .find(|binding| binding.cis_id == cis.cis_id)
            .expect("binding completeness validated above")
            .acl
            .connection_handle;
        if cis.connection_handle == acl {
            return Err(CisCreateConfigError::CisHandleEqualsAclHandle(
                cis.connection_handle,
            ));
        }
    }

    Ok(())
}

fn create_cis_command(
    allocation: &CigAllocation,
    bindings: &[CisAclBinding],
) -> Command {
    let mut cis_connection_handle =
        Vec::with_capacity(allocation.cis.len());
    let mut acl_connection_handle =
        Vec::with_capacity(allocation.cis.len());

    for cis in &allocation.cis {
        let binding = bindings
            .iter()
            .find(|binding| binding.cis_id == cis.cis_id)
            .expect("binding completeness validated before command build");
        cis_connection_handle.push(cis.connection_handle);
        acl_connection_handle.push(binding.acl.connection_handle);
    }

    Command::LeCreateCis {
        cis_connection_handle,
        acl_connection_handle,
    }
}

fn validate_cig_config(config: &CigConfig) -> Result<(), CigConfigError> {
    if config.cis.is_empty() {
        return Err(CigConfigError::EmptyCisSet);
    }
    if config.cis.len() > usize::from(u8::MAX) {
        return Err(CigConfigError::TooManyCis);
    }
    if config.sdu_interval_c_to_p > MAX_U24 {
        return Err(CigConfigError::SduIntervalTooLarge {
            direction: "c_to_p",
            value: config.sdu_interval_c_to_p,
        });
    }
    if config.sdu_interval_p_to_c > MAX_U24 {
        return Err(CigConfigError::SduIntervalTooLarge {
            direction: "p_to_c",
            value: config.sdu_interval_p_to_c,
        });
    }
    if config.packing > 1 {
        return Err(CigConfigError::InvalidPacking(config.packing));
    }
    if config.framing > 1 {
        return Err(CigConfigError::InvalidFraming(config.framing));
    }

    let mut cis_ids = BTreeSet::new();
    for cis in &config.cis {
        if !cis_ids.insert(cis.cis_id) {
            return Err(CigConfigError::DuplicateCisId(cis.cis_id));
        }
    }
    Ok(())
}

fn set_cig_command(config: &CigConfig) -> Command {
    Command::LeSetCigParameters {
        cig_id: config.cig_id,
        sdu_interval_c_to_p: config.sdu_interval_c_to_p,
        sdu_interval_p_to_c: config.sdu_interval_p_to_c,
        worst_case_sca: config.worst_case_sca,
        packing: config.packing,
        framing: config.framing,
        max_transport_latency_c_to_p: config.max_transport_latency_c_to_p,
        max_transport_latency_p_to_c: config.max_transport_latency_p_to_c,
        cis_id: config.cis.iter().map(|cis| cis.cis_id).collect(),
        max_sdu_c_to_p: config
            .cis
            .iter()
            .map(|cis| cis.max_sdu_c_to_p)
            .collect(),
        max_sdu_p_to_c: config
            .cis
            .iter()
            .map(|cis| cis.max_sdu_p_to_c)
            .collect(),
        phy_c_to_p: config.cis.iter().map(|cis| cis.phy_c_to_p).collect(),
        phy_p_to_c: config.cis.iter().map(|cis| cis.phy_p_to_c).collect(),
        rtn_c_to_p: config.cis.iter().map(|cis| cis.rtn_c_to_p).collect(),
        rtn_p_to_c: config.cis.iter().map(|cis| cis.rtn_p_to_c).collect(),
    }
}

fn parse_allocation(
    config: &CigConfig,
    parameters_after_status: &[u8],
) -> Result<CigAllocation, CigConfigureFailureKind> {
    if parameters_after_status.len() < 2 {
        return Err(CigConfigureFailureKind::MalformedResponse);
    }

    let cig_id = parameters_after_status[0];
    if cig_id != config.cig_id {
        return Err(CigConfigureFailureKind::CigIdMismatch {
            expected: config.cig_id,
            actual: cig_id,
        });
    }

    let count = usize::from(parameters_after_status[1]);
    if parameters_after_status.len() != 2 + count * 2 {
        return Err(CigConfigureFailureKind::MalformedResponse);
    }
    if count != config.cis.len() {
        return Err(CigConfigureFailureKind::CisCountMismatch {
            expected: config.cis.len(),
            actual: count,
        });
    }

    let mut handles = BTreeSet::new();
    let mut allocations = Vec::with_capacity(count);
    for (cis, bytes) in config
        .cis
        .iter()
        .zip(parameters_after_status[2..].chunks_exact(2))
    {
        let connection_handle = u16::from_le_bytes([bytes[0], bytes[1]]);
        if connection_handle > MAX_CONNECTION_HANDLE {
            return Err(CigConfigureFailureKind::InvalidConnectionHandle(
                connection_handle,
            ));
        }
        if !handles.insert(connection_handle) {
            return Err(CigConfigureFailureKind::DuplicateConnectionHandle(
                connection_handle,
            ));
        }
        allocations.push(CisAllocation {
            cis_id: cis.cis_id,
            connection_handle,
        });
    }

    Ok(CigAllocation {
        cig_id,
        cis: allocations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CigConfig {
        let cis = |cis_id| CisConfig {
            cis_id,
            max_sdu_c_to_p: 100,
            max_sdu_p_to_c: 0,
            phy_c_to_p: 2,
            phy_p_to_c: 1,
            rtn_c_to_p: 13,
            rtn_p_to_c: 0,
        };
        CigConfig {
            cig_id: 1,
            sdu_interval_c_to_p: 10_000,
            sdu_interval_p_to_c: 10_000,
            worst_case_sca: 0,
            packing: 0,
            framing: 0,
            max_transport_latency_c_to_p: 95,
            max_transport_latency_p_to_c: 95,
            cis: vec![cis(1), cis(2)],
        }
    }

    fn allocation() -> CigAllocation {
        CigAllocation {
            cig_id: 1,
            cis: vec![
                CisAllocation {
                    cis_id: 1,
                    connection_handle: 0x0900,
                },
                CisAllocation {
                    cis_id: 2,
                    connection_handle: 0x0901,
                },
            ],
        }
    }

    fn token(handle: u16, generation: u64) -> ConnectionToken {
        ConnectionToken {
            connection_handle: handle,
            generation,
        }
    }

    #[test]
    fn create_cis_command_status_must_succeed_once() {
        let bindings = vec![
            CisAclBinding {
                cis_id: 1,
                acl: token(0x0800, 7),
            },
            CisAclBinding {
                cis_id: 2,
                acl: token(0x0801, 8),
            },
        ];
        let (mut transaction, _) =
            CisCreateTransaction::begin(allocation(), bindings).unwrap();

        assert_eq!(
            transaction.on_command_status(HCI_LE_CREATE_CIS_COMMAND, 0),
            CisCreateAction::AwaitingEstablished
        );
        assert_eq!(
            transaction.on_command_status(HCI_LE_CREATE_CIS_COMMAND, 0),
            CisCreateAction::Failed(CisCreateFailure {
                cig_id: 1,
                kind: CisCreateFailureKind::DuplicateCommandStatus,
            })
        );
    }

    #[test]
    fn create_cis_rejected_status_is_terminal() {
        let bindings = vec![
            CisAclBinding {
                cis_id: 1,
                acl: token(0x0800, 7),
            },
            CisAclBinding {
                cis_id: 2,
                acl: token(0x0801, 8),
            },
        ];
        let (mut transaction, _) =
            CisCreateTransaction::begin(allocation(), bindings).unwrap();

        assert_eq!(
            transaction.on_command_status(HCI_LE_CREATE_CIS_COMMAND, 0x0c),
            CisCreateAction::Failed(CisCreateFailure {
                cig_id: 1,
                kind: CisCreateFailureKind::CommandStatus { status: 0x0c },
            })
        );
        assert_eq!(
            transaction.on_command_status(HCI_LE_CREATE_CIS_COMMAND, 0),
            CisCreateAction::Ignored
        );
    }

    #[test]
    fn create_cis_command_follows_allocation_order_not_binding_order() {
        let bindings = vec![
            CisAclBinding {
                cis_id: 2,
                acl: token(0x0800, 7),
            },
            CisAclBinding {
                cis_id: 1,
                acl: token(0x0801, 8),
            },
        ];
        let (_transaction, command) =
            CisCreateTransaction::begin(allocation(), bindings).unwrap();
        let Command::LeCreateCis {
            cis_connection_handle,
            acl_connection_handle,
        } = command
        else {
            panic!("wrong command");
        };
        assert_eq!(cis_connection_handle, vec![0x0900, 0x0901]);
        assert_eq!(acl_connection_handle, vec![0x0801, 0x0800]);
    }

    #[test]
    fn create_cis_allows_multiple_cis_on_one_acl() {
        let bindings = vec![
            CisAclBinding {
                cis_id: 1,
                acl: token(0x0800, 7),
            },
            CisAclBinding {
                cis_id: 2,
                acl: token(0x0800, 7),
            },
        ];
        let (_transaction, command) =
            CisCreateTransaction::begin(allocation(), bindings).unwrap();
        assert_eq!(command.op_code(), HCI_LE_CREATE_CIS_COMMAND);
    }

    #[test]
    fn create_cis_rejects_incomplete_or_unknown_bindings() {
        assert_eq!(
            CisCreateTransaction::begin(
                allocation(),
                vec![CisAclBinding {
                    cis_id: 1,
                    acl: token(0x0800, 7),
                }],
            )
            .err(),
            Some(CisCreateConfigError::BindingCountMismatch {
                expected: 2,
                actual: 1,
            })
        );

        assert_eq!(
            CisCreateTransaction::begin(
                allocation(),
                vec![
                    CisAclBinding {
                        cis_id: 1,
                        acl: token(0x0800, 7),
                    },
                    CisAclBinding {
                        cis_id: 3,
                        acl: token(0x0801, 8),
                    },
                ],
            )
            .err(),
            Some(CisCreateConfigError::UnknownBinding(3))
        );
    }

    #[test]
    fn set_cig_command_preserves_explicit_unidirectional_parameters() {
        let (_transaction, command) =
            CigConfigureTransaction::begin(config()).unwrap();
        let Command::LeSetCigParameters {
            max_sdu_p_to_c,
            phy_p_to_c,
            rtn_p_to_c,
            ..
        } = command
        else {
            panic!("wrong command");
        };
        assert_eq!(max_sdu_p_to_c, vec![0, 0]);
        assert_eq!(phy_p_to_c, vec![1, 1]);
        assert_eq!(rtn_p_to_c, vec![0, 0]);
    }

    #[test]
    fn successful_complete_maps_controller_handles_to_cis_ids() {
        let (mut transaction, _) =
            CigConfigureTransaction::begin(config()).unwrap();
        let action = transaction.on_command_complete(
            HCI_LE_SET_CIG_PARAMETERS_COMMAND,
            0,
            &[1, 2, 0x40, 0x00, 0x41, 0x00],
        );
        assert!(matches!(
            action,
            CigConfigureAction::Complete(CigAllocation {
                cig_id: 1,
                cis
            }) if cis == vec![
                CisAllocation { cis_id: 1, connection_handle: 0x0040 },
                CisAllocation { cis_id: 2, connection_handle: 0x0041 },
            ]
        ));
    }

    #[test]
    fn duplicate_cis_id_is_rejected_before_controller_io() {
        let mut invalid = config();
        invalid.cis[1].cis_id = invalid.cis[0].cis_id;
        assert_eq!(
            CigConfigureTransaction::begin(invalid).err(),
            Some(CigConfigError::DuplicateCisId(1))
        );
    }

    #[test]
    fn controller_status_failure_is_terminal_and_typed() {
        let (mut transaction, _) =
            CigConfigureTransaction::begin(config()).unwrap();
        let action = transaction.on_command_complete(
            HCI_LE_SET_CIG_PARAMETERS_COMMAND,
            0x0c,
            &[],
        );
        assert!(matches!(
            action,
            CigConfigureAction::Failed(CigConfigureFailure {
                cig_id: 1,
                may_have_allocated: false,
                kind: CigConfigureFailureKind::CommandCompleteStatus {
                    status: 0x0c
                },
            })
        ));
        assert!(matches!(
            transaction.on_command_complete(
                HCI_LE_SET_CIG_PARAMETERS_COMMAND,
                0,
                &[1, 2, 0x40, 0, 0x41, 0],
            ),
            CigConfigureAction::Ignored
        ));
    }

    #[test]
    fn malformed_or_duplicate_controller_handles_are_rejected() {
        let (mut wrong_count, _) =
            CigConfigureTransaction::begin(config()).unwrap();
        assert!(matches!(
            wrong_count.on_command_complete(
                HCI_LE_SET_CIG_PARAMETERS_COMMAND,
                0,
                &[1, 1, 0x40, 0],
            ),
            CigConfigureAction::Failed(CigConfigureFailure {
                may_have_allocated: true,
                kind: CigConfigureFailureKind::CisCountMismatch {
                    expected: 2,
                    actual: 1
                },
                ..
            })
        ));

        let (mut duplicate, _) =
            CigConfigureTransaction::begin(config()).unwrap();
        assert!(matches!(
            duplicate.on_command_complete(
                HCI_LE_SET_CIG_PARAMETERS_COMMAND,
                0,
                &[1, 2, 0x40, 0, 0x40, 0],
            ),
            CigConfigureAction::Failed(CigConfigureFailure {
                may_have_allocated: true,
                kind: CigConfigureFailureKind::DuplicateConnectionHandle(
                    0x0040
                ),
                ..
            })
        ));
    }
}
