use std::error::Error;
use std::fmt;

use crate::model::{
    BondRequest, BondRole, ExpectedSetMember, MemberIdentity, PeerAddress, SetDescriptor,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingStage {
    Idle,
    PrimaryDiscovery,
    PrimaryBond,
    PrimaryCsisDiscovery,
    SetMemberSearch,
    SetMemberBond,
    SetMemberVerification,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingState {
    Idle,
    DiscoveringPrimary {
        target_name: String,
    },
    BondingPrimary {
        target_name: String,
        address: PeerAddress,
    },
    DiscoveringPrimaryCsis {
        target_name: String,
        primary: MemberIdentity,
    },
    SearchingSetMembers {
        target_name: String,
        set: SetDescriptor,
        members: Vec<MemberIdentity>,
    },
    BondingSetMember {
        target_name: String,
        set: SetDescriptor,
        members: Vec<MemberIdentity>,
        expected: ExpectedSetMember,
    },
    VerifyingSetMember {
        target_name: String,
        set: SetDescriptor,
        members: Vec<MemberIdentity>,
        expected: ExpectedSetMember,
        provisional_identity: MemberIdentity,
    },
    Complete {
        target_name: String,
        set: Option<SetDescriptor>,
        members: Vec<MemberIdentity>,
    },
    Failed {
        stage: PairingStage,
        reason: String,
    },
}

impl PairingState {
    pub fn stage(&self) -> PairingStage {
        match self {
            Self::Idle => PairingStage::Idle,
            Self::DiscoveringPrimary { .. } => PairingStage::PrimaryDiscovery,
            Self::BondingPrimary { .. } => PairingStage::PrimaryBond,
            Self::DiscoveringPrimaryCsis { .. } => PairingStage::PrimaryCsisDiscovery,
            Self::SearchingSetMembers { .. } => PairingStage::SetMemberSearch,
            Self::BondingSetMember { .. } => PairingStage::SetMemberBond,
            Self::VerifyingSetMember { .. } => PairingStage::SetMemberVerification,
            Self::Complete { .. } => PairingStage::Complete,
            Self::Failed { stage, .. } => *stage,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingEvent {
    Start {
        target_name: String,
    },
    PrimaryCandidateFound {
        address: PeerAddress,
    },
    BondSucceeded {
        identity: MemberIdentity,
    },
    BondFailed {
        reason: String,
    },
    PrimaryCsisDiscovered {
        set: SetDescriptor,
    },
    PrimaryCsisUnavailable,
    ExpectedSetMemberFound {
        expected: ExpectedSetMember,
    },
    SetMemberSirkVerified,
    SetMemberSirkRejected {
        reason: String,
    },
    SearchExhausted,
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingAction {
    StartPrimaryDiscovery {
        target_name: String,
    },
    StopDiscovery,
    BeginLeBond(BondRequest),
    CommitBond {
        role: BondRole,
        identity: MemberIdentity,
    },
    DiscoverPrimaryCsis {
        primary: MemberIdentity,
    },
    StartSetMemberSearch {
        set: SetDescriptor,
        known_members: Vec<MemberIdentity>,
    },
    VerifySetMemberSirk {
        expected: ExpectedSetMember,
        provisional_identity: MemberIdentity,
    },
    DiscardProvisionalBond {
        identity: MemberIdentity,
    },
    SurfaceRecoverable {
        stage: PairingStage,
        reason: String,
    },
    SurfaceTerminal {
        stage: PairingStage,
        reason: String,
    },
    ReportComplete {
        members: Vec<MemberIdentity>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub state: PairingState,
    pub actions: Vec<PairingAction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidTransition {
    pub state: PairingStage,
    pub event: &'static str,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "event {} is invalid while pairing state is {:?}",
            self.event, self.state
        )
    }
}

impl Error for InvalidTransition {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingMachine {
    state: PairingState,
}

impl Default for PairingMachine {
    fn default() -> Self {
        Self {
            state: PairingState::Idle,
        }
    }
}

impl PairingMachine {
    pub fn state(&self) -> &PairingState {
        &self.state
    }

    pub fn apply(&mut self, event: PairingEvent) -> Result<Vec<PairingAction>, InvalidTransition> {
        let transition = reduce(self.state.clone(), event)?;
        self.state = transition.state;
        Ok(transition.actions)
    }
}

pub fn reduce(
    state: PairingState,
    event: PairingEvent,
) -> Result<Transition, InvalidTransition> {
    if matches!(event, PairingEvent::Reset) {
        return Ok(Transition {
            state: PairingState::Idle,
            actions: Vec::new(),
        });
    }

    let stage = state.stage();
    let event_name = event_name(&event);

    match (state, event) {
        (PairingState::Idle, PairingEvent::Start { target_name }) => Ok(Transition {
            state: PairingState::DiscoveringPrimary {
                target_name: target_name.clone(),
            },
            actions: vec![PairingAction::StartPrimaryDiscovery { target_name }],
        }),

        (
            PairingState::DiscoveringPrimary { target_name },
            PairingEvent::PrimaryCandidateFound { address },
        ) => Ok(Transition {
            state: PairingState::BondingPrimary {
                target_name,
                address: address.clone(),
            },
            actions: vec![
                PairingAction::StopDiscovery,
                PairingAction::BeginLeBond(BondRequest::primary(address)),
            ],
        }),

        (
            PairingState::BondingPrimary {
                target_name,
                address: _,
            },
            PairingEvent::BondSucceeded { identity },
        ) => Ok(Transition {
            state: PairingState::DiscoveringPrimaryCsis {
                target_name,
                primary: identity.clone(),
            },
            actions: vec![
                PairingAction::CommitBond {
                    role: BondRole::Primary,
                    identity: identity.clone(),
                },
                PairingAction::DiscoverPrimaryCsis { primary: identity },
            ],
        }),

        (
            PairingState::BondingPrimary { .. },
            PairingEvent::BondFailed { reason },
        ) => terminal(PairingStage::PrimaryBond, reason),

        (
            PairingState::DiscoveringPrimaryCsis {
                target_name,
                primary,
            },
            PairingEvent::PrimaryCsisUnavailable,
        ) => {
            let members = vec![primary];
            Ok(Transition {
                state: PairingState::Complete {
                    target_name,
                    set: None,
                    members: members.clone(),
                },
                actions: vec![PairingAction::ReportComplete { members }],
            })
        }

        (
            PairingState::DiscoveringPrimaryCsis {
                target_name,
                primary,
            },
            PairingEvent::PrimaryCsisDiscovered { set },
        ) => {
            let members = vec![primary];
            if set.size <= 1 {
                Ok(Transition {
                    state: PairingState::Complete {
                        target_name,
                        set: Some(set),
                        members: members.clone(),
                    },
                    actions: vec![PairingAction::ReportComplete { members }],
                })
            } else {
                Ok(Transition {
                    state: PairingState::SearchingSetMembers {
                        target_name,
                        set: set.clone(),
                        members: members.clone(),
                    },
                    actions: vec![PairingAction::StartSetMemberSearch {
                        set,
                        known_members: members,
                    }],
                })
            }
        }

        (
            PairingState::SearchingSetMembers {
                target_name,
                set,
                members,
            },
            PairingEvent::ExpectedSetMemberFound { expected },
        ) if expected.set.handle == set.handle => Ok(Transition {
            state: PairingState::BondingSetMember {
                target_name,
                set,
                members,
                expected: expected.clone(),
            },
            actions: vec![
                PairingAction::StopDiscovery,
                PairingAction::BeginLeBond(BondRequest::set_member(&expected)),
            ],
        }),

        (
            PairingState::SearchingSetMembers {
                target_name,
                set,
                members,
            },
            PairingEvent::SearchExhausted,
        ) => Ok(Transition {
            state: PairingState::SearchingSetMembers {
                target_name,
                set,
                members,
            },
            actions: vec![PairingAction::SurfaceRecoverable {
                stage: PairingStage::SetMemberSearch,
                reason: "no additional coordinated-set member was visible".into(),
            }],
        }),

        (
            PairingState::BondingSetMember {
                target_name,
                set,
                members,
                expected,
            },
            PairingEvent::BondSucceeded { identity },
        ) => Ok(Transition {
            state: PairingState::VerifyingSetMember {
                target_name,
                set,
                members,
                expected: expected.clone(),
                provisional_identity: identity.clone(),
            },
            actions: vec![PairingAction::VerifySetMemberSirk {
                expected,
                provisional_identity: identity,
            }],
        }),

        (
            PairingState::BondingSetMember {
                target_name,
                set,
                members,
                expected: _,
            },
            PairingEvent::BondFailed { reason },
        ) => Ok(Transition {
            state: PairingState::SearchingSetMembers {
                target_name,
                set: set.clone(),
                members: members.clone(),
            },
            actions: vec![
                PairingAction::SurfaceRecoverable {
                    stage: PairingStage::SetMemberBond,
                    reason,
                },
                PairingAction::StartSetMemberSearch {
                    set,
                    known_members: members,
                },
            ],
        }),

        (
            PairingState::VerifyingSetMember {
                target_name,
                set,
                mut members,
                expected: _,
                provisional_identity,
            },
            PairingEvent::SetMemberSirkVerified,
        ) => {
            members.push(provisional_identity.clone());
            let mut actions = vec![PairingAction::CommitBond {
                role: BondRole::SetMember,
                identity: provisional_identity,
            }];

            if members.len() >= usize::from(set.size) {
                actions.push(PairingAction::ReportComplete {
                    members: members.clone(),
                });
                Ok(Transition {
                    state: PairingState::Complete {
                        target_name,
                        set: Some(set),
                        members,
                    },
                    actions,
                })
            } else {
                actions.push(PairingAction::StartSetMemberSearch {
                    set: set.clone(),
                    known_members: members.clone(),
                });
                Ok(Transition {
                    state: PairingState::SearchingSetMembers {
                        target_name,
                        set,
                        members,
                    },
                    actions,
                })
            }
        }

        (
            PairingState::VerifyingSetMember {
                target_name,
                set,
                members,
                expected: _,
                provisional_identity,
            },
            PairingEvent::SetMemberSirkRejected { reason },
        ) => Ok(Transition {
            state: PairingState::SearchingSetMembers {
                target_name,
                set: set.clone(),
                members: members.clone(),
            },
            actions: vec![
                PairingAction::DiscardProvisionalBond {
                    identity: provisional_identity,
                },
                PairingAction::SurfaceRecoverable {
                    stage: PairingStage::SetMemberVerification,
                    reason,
                },
                PairingAction::StartSetMemberSearch {
                    set,
                    known_members: members,
                },
            ],
        }),

        _ => Err(InvalidTransition {
            state: stage,
            event: event_name,
        }),
    }
}

fn terminal(stage: PairingStage, reason: String) -> Result<Transition, InvalidTransition> {
    Ok(Transition {
        state: PairingState::Failed {
            stage,
            reason: reason.clone(),
        },
        actions: vec![PairingAction::SurfaceTerminal { stage, reason }],
    })
}

fn event_name(event: &PairingEvent) -> &'static str {
    match event {
        PairingEvent::Start { .. } => "Start",
        PairingEvent::PrimaryCandidateFound { .. } => "PrimaryCandidateFound",
        PairingEvent::BondSucceeded { .. } => "BondSucceeded",
        PairingEvent::BondFailed { .. } => "BondFailed",
        PairingEvent::PrimaryCsisDiscovered { .. } => "PrimaryCsisDiscovered",
        PairingEvent::PrimaryCsisUnavailable => "PrimaryCsisUnavailable",
        PairingEvent::ExpectedSetMemberFound { .. } => "ExpectedSetMemberFound",
        PairingEvent::SetMemberSirkVerified => "SetMemberSirkVerified",
        PairingEvent::SetMemberSirkRejected { .. } => "SetMemberSirkRejected",
        PairingEvent::SearchExhausted => "SearchExhausted",
        PairingEvent::Reset => "Reset",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AddressKind, SetHandle};

    fn address(value: &str) -> PeerAddress {
        PeerAddress::new(value, AddressKind::Random)
    }

    fn identity(value: &str) -> MemberIdentity {
        MemberIdentity::new(value)
    }

    fn stereo_set() -> SetDescriptor {
        SetDescriptor::new(SetHandle::new("buds3-pro-set"), 2)
    }

    #[test]
    fn primary_then_second_member_completes_serially() {
        let mut machine = PairingMachine::default();
        assert_eq!(
            machine
                .apply(PairingEvent::Start {
                    target_name: "Galaxy Buds3 Pro".into(),
                })
                .unwrap(),
            vec![PairingAction::StartPrimaryDiscovery {
                target_name: "Galaxy Buds3 Pro".into(),
            }]
        );

        machine
            .apply(PairingEvent::PrimaryCandidateFound {
                address: address("primary-rpa"),
            })
            .unwrap();
        machine
            .apply(PairingEvent::BondSucceeded {
                identity: identity("primary-id"),
            })
            .unwrap();
        machine
            .apply(PairingEvent::PrimaryCsisDiscovered { set: stereo_set() })
            .unwrap();

        let expected = ExpectedSetMember {
            advertised_address: address("secondary-rpa"),
            set: stereo_set(),
            rsi: [1, 2, 3, 4, 5, 6],
        };
        machine
            .apply(PairingEvent::ExpectedSetMemberFound {
                expected: expected.clone(),
            })
            .unwrap();
        let actions = machine
            .apply(PairingEvent::BondSucceeded {
                identity: identity("secondary-id"),
            })
            .unwrap();
        assert_eq!(
            actions,
            vec![PairingAction::VerifySetMemberSirk {
                expected,
                provisional_identity: identity("secondary-id"),
            }]
        );

        let actions = machine
            .apply(PairingEvent::SetMemberSirkVerified)
            .unwrap();
        assert!(matches!(machine.state(), PairingState::Complete { .. }));
        assert_eq!(
            actions,
            vec![
                PairingAction::CommitBond {
                    role: BondRole::SetMember,
                    identity: identity("secondary-id"),
                },
                PairingAction::ReportComplete {
                    members: vec![identity("primary-id"), identity("secondary-id")],
                },
            ]
        );
    }

    #[test]
    fn rejected_member_is_discarded_and_search_resumes() {
        let state = PairingState::VerifyingSetMember {
            target_name: "Buds".into(),
            set: stereo_set(),
            members: vec![identity("primary")],
            expected: ExpectedSetMember {
                advertised_address: address("candidate"),
                set: stereo_set(),
                rsi: [0; 6],
            },
            provisional_identity: identity("wrong"),
        };

        let transition = reduce(
            state,
            PairingEvent::SetMemberSirkRejected {
                reason: "SIRK mismatch".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            transition.state,
            PairingState::SearchingSetMembers { .. }
        ));
        assert!(matches!(
            transition.actions.first(),
            Some(PairingAction::DiscardProvisionalBond { identity }) if identity == &MemberIdentity::new("wrong")
        ));
    }

    #[test]
    fn only_one_set_member_bond_may_be_in_flight() {
        let expected = ExpectedSetMember {
            advertised_address: address("candidate-a"),
            set: stereo_set(),
            rsi: [0; 6],
        };
        let state = PairingState::BondingSetMember {
            target_name: "Buds".into(),
            set: stereo_set(),
            members: vec![identity("primary")],
            expected,
        };

        let error = reduce(
            state,
            PairingEvent::ExpectedSetMemberFound {
                expected: ExpectedSetMember {
                    advertised_address: address("candidate-b"),
                    set: stereo_set(),
                    rsi: [1; 6],
                },
            },
        )
        .unwrap_err();
        assert_eq!(error.state, PairingStage::SetMemberBond);
    }

    #[test]
    fn search_exhaustion_is_recoverable_and_does_not_lose_set_state() {
        let state = PairingState::SearchingSetMembers {
            target_name: "Buds".into(),
            set: stereo_set(),
            members: vec![identity("primary")],
        };
        let transition = reduce(state.clone(), PairingEvent::SearchExhausted).unwrap();
        assert_eq!(transition.state, state);
        assert!(matches!(
            transition.actions.as_slice(),
            [PairingAction::SurfaceRecoverable {
                stage: PairingStage::SetMemberSearch,
                ..
            }]
        ));
    }

    #[test]
    fn primary_bond_failure_is_terminal() {
        let transition = reduce(
            PairingState::BondingPrimary {
                target_name: "Buds".into(),
                address: address("primary"),
            },
            PairingEvent::BondFailed {
                reason: "timeout".into(),
            },
        )
        .unwrap();
        assert!(matches!(transition.state, PairingState::Failed { .. }));
    }
}
