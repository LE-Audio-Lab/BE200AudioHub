use std::collections::BTreeMap;

use bumble::{Address, AddressType};
use bumble_smp::privacy::verify_resolvable_private_address;

use crate::plane::l1_hci::acl::scan::{
    ScanObservation, ScanSummary,
};

use super::plan::ControlSetPlan;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedMemberPresence {
    pub member_index: usize,
    pub current_address: String,
    pub current_address_type: u8,
    pub rssi: i8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlDiscoveryError {
    InvalidScanAddress {
        address: String,
        address_type: u8,
    },
    AmbiguousObservation {
        address: String,
    },
    MemberNotVisible {
        member_index: usize,
    },
    DuplicateTransportAddress {
        first_member_index: usize,
        second_member_index: usize,
        address: String,
        address_type: u8,
    },
}

pub fn resolve_connectable_members(
    plan: &ControlSetPlan,
    scan: &ScanSummary,
) -> Result<Vec<ResolvedMemberPresence>, ControlDiscoveryError> {
    let mut strongest =
        BTreeMap::<usize, &ScanObservation>::new();

    for observation in
        scan.observations.iter().filter(|value| value.is_connectable)
    {
        if !matches!(observation.address_type, 0 | 1) {
            continue;
        }

        let address = Address::parse(
            &observation.address,
            AddressType(observation.address_type),
        )
        .map_err(|_| ControlDiscoveryError::InvalidScanAddress {
            address: observation.address.clone(),
            address_type: observation.address_type,
        })?;

        let mut matched_member = None;
        for (member_index, member) in plan.members.iter().enumerate() {
            let identity = Address::from_bytes(
                member.identity_address_le,
                AddressType(member.identity_address_type),
            );
            let direct_identity = observation
                .address
                .eq_ignore_ascii_case(&identity.to_string(true));
            let resolved_rpa =
                verify_resolvable_private_address(
                    &member.peer_irk,
                    &address,
                );

            if !(direct_identity || resolved_rpa) {
                continue;
            }

            if matched_member.replace(member_index).is_some() {
                return Err(
                    ControlDiscoveryError::AmbiguousObservation {
                        address: observation.address.clone(),
                    },
                );
            }
        }

        let Some(member_index) = matched_member else {
            continue;
        };

        strongest
            .entry(member_index)
            .and_modify(|current| {
                if observation.rssi > current.rssi {
                    *current = observation;
                }
            })
            .or_insert(observation);
    }

    let mut result = Vec::with_capacity(plan.members.len());
    for member_index in 0..plan.members.len() {
        let observation = strongest
            .remove(&member_index)
            .ok_or(
                ControlDiscoveryError::MemberNotVisible {
                    member_index,
                },
            )?;
        result.push(ResolvedMemberPresence {
            member_index,
            current_address: observation.address.clone(),
            current_address_type: observation.address_type,
            rssi: observation.rssi,
        });
    }

    for first in 0..result.len() {
        for second in first + 1..result.len() {
            if result[first]
                .current_address
                .eq_ignore_ascii_case(&result[second].current_address)
                && result[first].current_address_type
                    == result[second].current_address_type
            {
                return Err(
                    ControlDiscoveryError::DuplicateTransportAddress {
                        first_member_index:
                            result[first].member_index,
                        second_member_index:
                            result[second].member_index,
                        address:
                            result[first].current_address.clone(),
                        address_type:
                            result[first].current_address_type,
                    },
                );
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use bumble_smp::privacy::resolvable_private_address;

    use crate::{
        control::plan::{
            ControlMemberPlan, ControlSetPlan,
        },
        plane::l1_hci::acl::{
            scan::ScanObservation,
            security::StoredLeKeyMaterial,
        },
    };

    use super::*;

    fn member(last: u8, irk: [u8; 16]) -> ControlMemberPlan {
        ControlMemberPlan {
            identity_address_le: [last, 5, 4, 3, 2, 1],
            identity_address_type: 1,
            peer_irk: irk,
            stored_key: StoredLeKeyMaterial::secure_connections(
                [last; 16],
            ),
        }
    }

    fn plan() -> ControlSetPlan {
        ControlSetPlan {
            controller_identity_le: [1, 2, 3, 4, 5, 6],
            local_irk: [0x55; 16],
            members: vec![
                member(0x10, [0x21; 16]),
                member(0x20, [0x31; 16]),
            ],
        }
    }

    fn observation(
        address: Address,
        rssi: i8,
        connectable: bool,
    ) -> ScanObservation {
        ScanObservation {
            address: address.to_string(true),
            address_type: address.address_type().0,
            name: None,
            rsi: None,
            rssi,
            primary_phy: 1,
            secondary_phy: 0,
            sid: 0,
            is_legacy: false,
            is_connectable: connectable,
            is_scannable: false,
            raw_data: vec![],
        }
    }

    #[test]
    fn resolves_both_members_and_keeps_strongest_rpa_per_identity() {
        let value = plan();
        let weak_left = resolvable_private_address(
            &value.members[0].peer_irk,
            [1, 2, 3],
        );
        let strong_left = resolvable_private_address(
            &value.members[0].peer_irk,
            [4, 5, 6],
        );
        let right = resolvable_private_address(
            &value.members[1].peer_irk,
            [7, 8, 9],
        );

        let scan = ScanSummary {
            reports_seen: 3,
            observations: vec![
                observation(weak_left, -70, true),
                observation(strong_left.clone(), -35, true),
                observation(right.clone(), -40, true),
            ],
            truncated: false,
        };

        let resolved =
            resolve_connectable_members(&value, &scan).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(
            resolved[0].current_address,
            strong_left.to_string(true)
        );
        assert_eq!(resolved[0].rssi, -35);
        assert_eq!(
            resolved[1].current_address,
            right.to_string(true)
        );
    }

    #[test]
    fn nonconnectable_member_does_not_satisfy_discovery_gate() {
        let value = plan();
        let left = resolvable_private_address(
            &value.members[0].peer_irk,
            [1, 2, 3],
        );
        let right = resolvable_private_address(
            &value.members[1].peer_irk,
            [4, 5, 6],
        );

        let scan = ScanSummary {
            reports_seen: 2,
            observations: vec![
                observation(left, -30, true),
                observation(right, -30, false),
            ],
            truncated: false,
        };

        assert_eq!(
            resolve_connectable_members(&value, &scan),
            Err(ControlDiscoveryError::MemberNotVisible {
                member_index: 1,
            })
        );
    }
}
