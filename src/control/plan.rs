use std::{
    collections::BTreeSet,
    fmt,
};

use crate::hci_host::{
    connection::{
        ConnectionToken, FalConnectRequest, LeInitiationParameters,
        LePeerIdentity,
    },
    privacy::{
        ResolvingListConfig, ResolvingPeer, DEFAULT_RPA_TIMEOUT_SECONDS,
    },
    security::{
        StoredEncryptionRequest, StoredLeKeyMaterial,
    },
};

pub const PRODUCT_MEMBER_COUNT: usize = 2;

#[derive(Clone, PartialEq, Eq)]
pub struct ControlMemberPlan {
    pub identity_address_le: [u8; 6],
    pub identity_address_type: u8,
    pub peer_irk: [u8; 16],
    pub stored_key: StoredLeKeyMaterial,
}

impl fmt::Debug for ControlMemberPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlMemberPlan")
            .field("identity_address_le", &self.identity_address_le)
            .field("identity_address_type", &self.identity_address_type)
            .field("peer_irk", &"<redacted>")
            .field("stored_key", &self.stored_key)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ControlSetPlan {
    pub controller_identity_le: [u8; 6],
    pub local_irk: [u8; 16],
    pub members: Vec<ControlMemberPlan>,
}

impl fmt::Debug for ControlSetPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlSetPlan")
            .field(
                "controller_identity_le",
                &self.controller_identity_le,
            )
            .field("local_irk", &"<redacted>")
            .field("members", &self.members)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlPlanError {
    WrongMemberCount {
        actual: usize,
    },
    InvalidAddressType {
        member_index: usize,
        address_type: u8,
    },
    DuplicateIdentity,
    MemberIndexOutOfRange {
        member_index: usize,
    },
}

impl ControlSetPlan {
    pub fn validate(&self) -> Result<(), ControlPlanError> {
        if self.members.len() != PRODUCT_MEMBER_COUNT {
            return Err(ControlPlanError::WrongMemberCount {
                actual: self.members.len(),
            });
        }

        let mut identities = BTreeSet::new();
        for (member_index, member) in self.members.iter().enumerate() {
            if !matches!(member.identity_address_type, 0 | 1) {
                return Err(ControlPlanError::InvalidAddressType {
                    member_index,
                    address_type: member.identity_address_type,
                });
            }

            if !identities.insert((
                member.identity_address_type,
                member.identity_address_le,
            )) {
                return Err(ControlPlanError::DuplicateIdentity);
            }
        }

        Ok(())
    }

    pub fn privacy_config(
        &self,
    ) -> Result<ResolvingListConfig, ControlPlanError> {
        self.validate()?;
        Ok(ResolvingListConfig {
            peers: self
                .members
                .iter()
                .map(|member| ResolvingPeer {
                    identity_address_le: member.identity_address_le,
                    identity_address_type: member.identity_address_type,
                    peer_irk: member.peer_irk,
                })
                .collect(),
            local_irk: self.local_irk,
            rpa_timeout_seconds: DEFAULT_RPA_TIMEOUT_SECONDS,
        })
    }

    pub fn connect_request(
        &self,
        member_index: usize,
    ) -> Result<FalConnectRequest, ControlPlanError> {
        self.validate()?;
        let member = self
            .members
            .get(member_index)
            .ok_or(ControlPlanError::MemberIndexOutOfRange {
                member_index,
            })?;

        Ok(FalConnectRequest {
            peer: LePeerIdentity {
                address_le: member.identity_address_le,
                address_type: member.identity_address_type,
            },
            parameters: LeInitiationParameters::default(),
        })
    }

    pub fn encryption_request(
        &self,
        member_index: usize,
        connection: ConnectionToken,
    ) -> Result<StoredEncryptionRequest, ControlPlanError> {
        self.validate()?;
        let member = self
            .members
            .get(member_index)
            .ok_or(ControlPlanError::MemberIndexOutOfRange {
                member_index,
            })?;

        Ok(StoredEncryptionRequest {
            connection,
            key: member.stored_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(last: u8) -> ControlMemberPlan {
        ControlMemberPlan {
            identity_address_le: [last, 5, 4, 3, 2, 1],
            identity_address_type: 1,
            peer_irk: [last; 16],
            stored_key: StoredLeKeyMaterial {
                long_term_key: [last.wrapping_add(1); 16],
                encrypted_diversifier: u16::from(last),
                random_number: [last.wrapping_add(2); 8],
            },
        }
    }

    fn plan() -> ControlSetPlan {
        ControlSetPlan {
            controller_identity_le: [1, 2, 3, 4, 5, 6],
            local_irk: [0x44; 16],
            members: vec![member(0x10), member(0x20)],
        }
    }

    #[test]
    fn product_plan_requires_exactly_two_distinct_members() {
        let mut value = plan();
        value.validate().unwrap();

        value.members.pop();
        assert_eq!(
            value.validate(),
            Err(ControlPlanError::WrongMemberCount { actual: 1 })
        );

        let mut value = plan();
        value.members[1].identity_address_le =
            value.members[0].identity_address_le;
        assert_eq!(
            value.validate(),
            Err(ControlPlanError::DuplicateIdentity)
        );
    }

    #[test]
    fn plan_maps_to_existing_hci_privacy_connect_and_security_types() {
        let value = plan();

        let privacy = value.privacy_config().unwrap();
        assert_eq!(privacy.peers.len(), 2);
        assert_eq!(
            privacy.peers[0].identity_address_le,
            value.members[0].identity_address_le
        );

        let connect = value.connect_request(1).unwrap();
        assert_eq!(
            connect.peer.address_le,
            value.members[1].identity_address_le
        );
        assert_eq!(
            connect.parameters,
            LeInitiationParameters::default()
        );

        let token = ConnectionToken {
            connection_handle: 0x0801,
            generation: 7,
        };
        let encryption =
            value.encryption_request(1, token).unwrap();
        assert_eq!(encryption.connection, token);
        assert_eq!(
            encryption.key.encrypted_diversifier,
            u16::from(0x20)
        );
    }

    #[test]
    fn debug_output_redacts_control_keys() {
        let debug = format!("{:?}", plan());
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("[68, 68, 68"));
    }
}
