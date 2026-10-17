use std::collections::BTreeSet;

use bumble_hci::{
    codes::HCI_LE_SET_CIG_PARAMETERS_COMMAND,
    Command,
};

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
    pub kind: CigConfigureFailureKind,
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
                kind,
            }),
        }
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
                kind: CigConfigureFailureKind::DuplicateConnectionHandle(
                    0x0040
                ),
                ..
            })
        ));
    }
}
