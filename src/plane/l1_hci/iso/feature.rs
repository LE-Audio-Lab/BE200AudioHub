use bumble_hci::{
    codes::{
        HCI_LE_READ_LOCAL_SUPPORTED_FEATURES_COMMAND,
        HCI_LE_SET_HOST_FEATURE_COMMAND,
    },
    Command,
};

pub const CIS_HOST_SUPPORT_BIT: u8 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CisHostFeatureFailure {
    MissingFeatureCache,
    FeatureCacheTooShort,
    SetStatus(u8),
    ReadStatus(u8),
    MalformedReadback,
    BitStillClear,
    UnexpectedCommandStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CisHostFeatureState {
    Idle,
    Setting,
    ReadingBack,
    Ready {
        initially_enabled: bool,
        set_command_sent: bool,
    },
    Failed(CisHostFeatureFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CisHostFeatureAction {
    Send(Command),
    Ready {
        initially_enabled: bool,
        set_command_sent: bool,
    },
    Failed(CisHostFeatureFailure),
    Ignored,
}

pub fn cis_host_feature_enabled(features: &[u8]) -> bool {
    features
        .get(usize::from(CIS_HOST_SUPPORT_BIT / 8))
        .is_some_and(|octet| {
            octet & (1u8 << (CIS_HOST_SUPPORT_BIT % 8)) != 0
        })
}

pub struct CisHostFeatureTransaction {
    initially_enabled: bool,
    set_command_sent: bool,
    phase: CisHostFeatureState,
}

impl CisHostFeatureTransaction {
    pub fn begin(
        cached_features: Option<&[u8]>,
    ) -> Result<(Option<Self>, CisHostFeatureAction), CisHostFeatureFailure> {
        let features =
            cached_features.ok_or(CisHostFeatureFailure::MissingFeatureCache)?;
        if features.len() < 5 {
            return Err(CisHostFeatureFailure::FeatureCacheTooShort);
        }
        if cis_host_feature_enabled(features) {
            return Ok((
                None,
                CisHostFeatureAction::Ready {
                    initially_enabled: true,
                    set_command_sent: false,
                },
            ));
        }

        Ok((
            Some(Self {
                initially_enabled: false,
                set_command_sent: true,
                phase: CisHostFeatureState::Setting,
            }),
            CisHostFeatureAction::Send(Command::LeSetHostFeature {
                bit_number: CIS_HOST_SUPPORT_BIT,
                bit_value: 1,
            }),
        ))
    }

    pub fn on_command_complete(
        &mut self,
        opcode: u16,
        status: Option<u8>,
        parameters_after_status: &[u8],
    ) -> CisHostFeatureAction {
        match self.phase {
            CisHostFeatureState::Setting
                if opcode == HCI_LE_SET_HOST_FEATURE_COMMAND =>
            {
                let Some(status) = status else {
                    return self.fail(
                        CisHostFeatureFailure::MalformedReadback,
                    );
                };
                if status != 0 {
                    return self.fail(
                        CisHostFeatureFailure::SetStatus(status),
                    );
                }
                self.phase = CisHostFeatureState::ReadingBack;
                CisHostFeatureAction::Send(
                    Command::LeReadLocalSupportedFeatures,
                )
            }
            CisHostFeatureState::ReadingBack
                if opcode == HCI_LE_READ_LOCAL_SUPPORTED_FEATURES_COMMAND =>
            {
                let Some(status) = status else {
                    return self.fail(
                        CisHostFeatureFailure::MalformedReadback,
                    );
                };
                if status != 0 {
                    return self.fail(
                        CisHostFeatureFailure::ReadStatus(status),
                    );
                }
                if parameters_after_status.len() != 8 {
                    return self.fail(
                        CisHostFeatureFailure::MalformedReadback,
                    );
                }
                if !cis_host_feature_enabled(parameters_after_status) {
                    return self.fail(
                        CisHostFeatureFailure::BitStillClear,
                    );
                }
                self.phase = CisHostFeatureState::Ready {
                    initially_enabled: self.initially_enabled,
                    set_command_sent: self.set_command_sent,
                };
                CisHostFeatureAction::Ready {
                    initially_enabled: self.initially_enabled,
                    set_command_sent: self.set_command_sent,
                }
            }
            _ => CisHostFeatureAction::Ignored,
        }
    }

    pub fn on_command_status(
        &mut self,
        opcode: u16,
    ) -> CisHostFeatureAction {
        if matches!(
            (self.phase, opcode),
            (
                CisHostFeatureState::Setting,
                HCI_LE_SET_HOST_FEATURE_COMMAND
            ) | (
                CisHostFeatureState::ReadingBack,
                HCI_LE_READ_LOCAL_SUPPORTED_FEATURES_COMMAND
            )
        ) {
            self.fail(CisHostFeatureFailure::UnexpectedCommandStatus)
        } else {
            CisHostFeatureAction::Ignored
        }
    }

    fn fail(
        &mut self,
        failure: CisHostFeatureFailure,
    ) -> CisHostFeatureAction {
        self.phase = CisHostFeatureState::Failed(failure);
        CisHostFeatureAction::Failed(failure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_feature_bit_32() {
        assert!(!cis_host_feature_enabled(&[]));
        assert!(!cis_host_feature_enabled(&[0; 8]));
        let mut features = [0u8; 8];
        features[4] = 1;
        assert!(cis_host_feature_enabled(&features));
        features[4] = 2;
        assert!(!cis_host_feature_enabled(&features));
    }

    #[test]
    fn already_enabled_is_a_noop_ready_state() {
        let mut features = [0u8; 8];
        features[4] = 1;
        let (transaction, action) =
            CisHostFeatureTransaction::begin(Some(&features)).unwrap();
        assert!(transaction.is_none());
        assert_eq!(
            action,
            CisHostFeatureAction::Ready {
                initially_enabled: true,
                set_command_sent: false,
            }
        );
    }

    #[test]
    fn set_requires_successful_feature_readback() {
        let features = [0u8; 8];
        let (transaction, action) =
            CisHostFeatureTransaction::begin(Some(&features)).unwrap();
        let mut transaction = transaction.unwrap();
        assert!(matches!(
            action,
            CisHostFeatureAction::Send(Command::LeSetHostFeature {
                bit_number: 32,
                bit_value: 1,
            })
        ));

        assert!(matches!(
            transaction.on_command_complete(
                HCI_LE_SET_HOST_FEATURE_COMMAND,
                Some(0),
                &[],
            ),
            CisHostFeatureAction::Send(
                Command::LeReadLocalSupportedFeatures
            )
        ));

        let mut after = [0u8; 8];
        after[4] = 1;
        assert_eq!(
            transaction.on_command_complete(
                HCI_LE_READ_LOCAL_SUPPORTED_FEATURES_COMMAND,
                Some(0),
                &after,
            ),
            CisHostFeatureAction::Ready {
                initially_enabled: false,
                set_command_sent: true,
            }
        );
    }

    #[test]
    fn set_wire_matches_sealed_r5_probe() {
        let bytes = Command::LeSetHostFeature {
            bit_number: 32,
            bit_value: 1,
        }
        .to_bytes();
        assert_eq!(bytes, [0x01, 0x74, 0x20, 0x02, 0x20, 0x01]);
    }
}
