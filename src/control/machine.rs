use crate::hci_host::{
    connection::{
        ConnectionToken, FalConnectFailure, FalConnectRequest,
        FalConnectionState, LeConnectionSetSnapshot,
    },
    engine::{
        ControllerIdentityFailure, OperationalAttachFailure,
        OperationalAttachState, PassiveScanState,
    },
    iso_host_feature::{
        CisHostFeatureFailure, CisHostFeatureState,
    },
    mailbox::HciReport,
    privacy::{
        ControllerPrivacyState, PrivacyFailure, ResolvingListConfig,
    },
    security::{
        StoredEncryptionFailure, StoredEncryptionRequest,
        StoredEncryptionState,
    },
};

use super::{
    discovery::{
        resolve_connectable_members, ControlDiscoveryError,
        ResolvedMemberPresence,
    },
    plan::{ControlPlanError, ControlSetPlan},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlPhase {
    Idle,
    Attaching,
    ReadingControllerIdentity,
    EnsuringCisHostFeature,
    StartingDiscovery,
    DiscoveringMembers,
    StoppingDiscovery,
    ConfiguringPrivacy,
    ConnectingMember { member_index: usize },
    SecuringMember { member_index: usize },
    AwaitingSetConnected,
    SetConnected,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlTimer {
    Attach,
    ControllerIdentity,
    CisHostFeature,
    ScanStart,
    DiscoveryWindow,
    ScanStop,
    Privacy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlAction {
    BeginOperationalAttach,
    ReadControllerIdentity,
    EnsureCisHostFeature,
    StartPassiveExtendedScan,
    StopPassiveExtendedScan,
    ConfigureControllerPrivacy(ResolvingListConfig),
    BeginFalConnection {
        member_index: usize,
        request: FalConnectRequest,
    },
    BeginStoredEncryption {
        member_index: usize,
        request: StoredEncryptionRequest,
    },
    ArmTimer(ControlTimer),
    DisarmTimer(ControlTimer),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlFailure {
    InvalidStartState(ControlPhase),
    Plan(ControlPlanError),
    Attach(OperationalAttachFailure),
    ControllerIdentity(ControllerIdentityFailure),
    ControllerIdentityMismatch {
        expected: [u8; 6],
        actual: [u8; 6],
    },
    CisHostFeature(CisHostFeatureFailure),
    Scan {
        opcode: u16,
        status: u8,
    },
    Discovery(ControlDiscoveryError),
    Privacy(PrivacyFailure),
    Connection {
        member_index: usize,
        failure: FalConnectFailure,
    },
    Encryption {
        member_index: usize,
        failure: StoredEncryptionFailure,
    },
    ConnectionTokenMismatch {
        member_index: usize,
        expected: ConnectionToken,
        actual: ConnectionToken,
    },
    ConnectionSetMismatch,
    Timeout(ControlTimer),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    DiscoveryWindowElapsed,
    TimerExpired(ControlTimer),
}

pub struct ControlMachine {
    plan: ControlSetPlan,
    phase: ControlPhase,
    resolved_members: Vec<ResolvedMemberPresence>,
    connections: [Option<ConnectionToken>; 2],
    encryption_key_sizes: [Option<u8>; 2],
    failure: Option<ControlFailure>,
}

impl ControlMachine {
    pub fn new(plan: ControlSetPlan) -> Result<Self, ControlPlanError> {
        plan.validate()?;
        Ok(Self {
            plan,
            phase: ControlPhase::Idle,
            resolved_members: Vec::new(),
            connections: [None, None],
            encryption_key_sizes: [None, None],
            failure: None,
        })
    }

    pub fn phase(&self) -> ControlPhase {
        self.phase
    }

    pub fn resolved_members(&self) -> &[ResolvedMemberPresence] {
        &self.resolved_members
    }

    pub fn failure(&self) -> Option<&ControlFailure> {
        self.failure.as_ref()
    }

    pub fn connections(&self) -> [Option<ConnectionToken>; 2] {
        self.connections
    }

    pub fn start(&mut self) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::Idle {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.plan.validate().map_err(ControlFailure::Plan)?;
        self.phase = ControlPhase::Attaching;
        Ok(vec![
            ControlAction::BeginOperationalAttach,
            ControlAction::ArmTimer(ControlTimer::Attach),
        ])
    }

    pub fn on_event(
        &mut self,
        event: ControlEvent,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        match event {
            ControlEvent::DiscoveryWindowElapsed
                if self.phase == ControlPhase::DiscoveringMembers =>
            {
                self.phase = ControlPhase::StoppingDiscovery;
                Ok(vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::DiscoveryWindow,
                    ),
                    ControlAction::StopPassiveExtendedScan,
                    ControlAction::ArmTimer(ControlTimer::ScanStop),
                ])
            }
            ControlEvent::TimerExpired(timer)
                if self.timer_matches_phase(timer) =>
            {
                self.fail(ControlFailure::Timeout(timer))
            }
            _ => Ok(Vec::new()),
        }
    }

    pub fn on_hci_report(
        &mut self,
        report: &HciReport,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        match (self.phase, report) {
            (
                ControlPhase::Attaching,
                HciReport::CapabilitiesReady(_),
            ) => {
                self.phase = ControlPhase::ReadingControllerIdentity;
                Ok(vec![
                    ControlAction::DisarmTimer(ControlTimer::Attach),
                    ControlAction::ReadControllerIdentity,
                    ControlAction::ArmTimer(
                        ControlTimer::ControllerIdentity,
                    ),
                ])
            }
            (
                ControlPhase::Attaching,
                HciReport::AttachState(
                    OperationalAttachState::Failed(failure),
                ),
            ) => self.fail(ControlFailure::Attach(*failure)),

            (
                ControlPhase::ReadingControllerIdentity,
                HciReport::ControllerIdentity(Ok(identity)),
            ) => {
                if identity.public_address_le
                    != self.plan.controller_identity_le
                {
                    return self.fail(
                        ControlFailure::ControllerIdentityMismatch {
                            expected: self.plan.controller_identity_le,
                            actual: identity.public_address_le,
                        },
                    );
                }
                self.phase = ControlPhase::EnsuringCisHostFeature;
                Ok(vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::ControllerIdentity,
                    ),
                    ControlAction::EnsureCisHostFeature,
                    ControlAction::ArmTimer(
                        ControlTimer::CisHostFeature,
                    ),
                ])
            }
            (
                ControlPhase::ReadingControllerIdentity,
                HciReport::ControllerIdentity(Err(failure)),
            ) => self.fail(ControlFailure::ControllerIdentity(*failure)),

            (
                ControlPhase::EnsuringCisHostFeature,
                HciReport::CisHostFeatureState(
                    CisHostFeatureState::Ready { .. },
                ),
            ) => {
                self.phase = ControlPhase::StartingDiscovery;
                Ok(vec![
                    ControlAction::DisarmTimer(
                        ControlTimer::CisHostFeature,
                    ),
                    ControlAction::StartPassiveExtendedScan,
                    ControlAction::ArmTimer(ControlTimer::ScanStart),
                ])
            }
            (
                ControlPhase::EnsuringCisHostFeature,
                HciReport::CisHostFeatureState(
                    CisHostFeatureState::Failed(failure),
                ),
            ) => self.fail(ControlFailure::CisHostFeature(*failure)),

            (
                ControlPhase::StartingDiscovery,
                HciReport::PassiveScanState(
                    PassiveScanState::Scanning,
                ),
            ) => {
                self.phase = ControlPhase::DiscoveringMembers;
                Ok(vec![
                    ControlAction::DisarmTimer(ControlTimer::ScanStart),
                    ControlAction::ArmTimer(
                        ControlTimer::DiscoveryWindow,
                    ),
                ])
            }
            (
                ControlPhase::StartingDiscovery
                | ControlPhase::DiscoveringMembers
                | ControlPhase::StoppingDiscovery,
                HciReport::PassiveScanState(
                    PassiveScanState::Failed { opcode, status },
                ),
            ) => self.fail(ControlFailure::Scan {
                opcode: *opcode,
                status: *status,
            }),

            (
                ControlPhase::StoppingDiscovery,
                HciReport::ScanComplete(scan),
            ) => {
                let resolved =
                    resolve_connectable_members(&self.plan, scan)
                        .map_err(ControlFailure::Discovery);
                let resolved = match resolved {
                    Ok(resolved) => resolved,
                    Err(failure) => return self.fail(failure),
                };
                let privacy = self
                    .plan
                    .privacy_config()
                    .map_err(ControlFailure::Plan);
                let privacy = match privacy {
                    Ok(privacy) => privacy,
                    Err(failure) => return self.fail(failure),
                };

                self.resolved_members = resolved;
                self.phase = ControlPhase::ConfiguringPrivacy;
                Ok(vec![
                    ControlAction::DisarmTimer(ControlTimer::ScanStop),
                    ControlAction::ConfigureControllerPrivacy(privacy),
                    ControlAction::ArmTimer(ControlTimer::Privacy),
                ])
            }
            (
                ControlPhase::ConfiguringPrivacy,
                HciReport::ControllerPrivacyState(
                    ControllerPrivacyState::Ready,
                ),
            ) => self.begin_member_connection(0),
            (
                ControlPhase::ConfiguringPrivacy,
                HciReport::ControllerPrivacyState(
                    ControllerPrivacyState::Failed(failure),
                ),
            ) => self.fail(ControlFailure::Privacy(failure.clone())),

            (
                ControlPhase::ConnectingMember { member_index },
                HciReport::FalConnectionState(
                    FalConnectionState::Connected(connection),
                ),
            ) => {
                let token = connection.token();
                self.connections[member_index] = Some(token);
                self.phase = ControlPhase::SecuringMember {
                    member_index,
                };
                let request = self
                    .plan
                    .encryption_request(member_index, token)
                    .map_err(ControlFailure::Plan)?;
                Ok(vec![ControlAction::BeginStoredEncryption {
                    member_index,
                    request,
                }])
            }
            (
                ControlPhase::ConnectingMember { member_index },
                HciReport::FalConnectionState(
                    FalConnectionState::Failed(failure),
                ),
            ) => self.fail(ControlFailure::Connection {
                member_index,
                failure: failure.clone(),
            }),

            (
                ControlPhase::SecuringMember { member_index },
                HciReport::StoredEncryptionState(
                    StoredEncryptionState::Encrypted {
                        connection,
                        encryption_key_size,
                    },
                ),
            ) => {
                let expected = self.connections[member_index]
                    .expect("securing phase requires a connection token");
                if *connection != expected {
                    return self.fail(
                        ControlFailure::ConnectionTokenMismatch {
                            member_index,
                            expected,
                            actual: *connection,
                        },
                    );
                }
                self.encryption_key_sizes[member_index] =
                    Some(*encryption_key_size);

                if member_index + 1 < self.plan.members.len() {
                    self.begin_member_connection(member_index + 1)
                } else {
                    self.phase = ControlPhase::AwaitingSetConnected;
                    Ok(Vec::new())
                }
            }
            (
                ControlPhase::SecuringMember { member_index },
                HciReport::StoredEncryptionState(
                    StoredEncryptionState::Failed(failure),
                ),
            ) => self.fail(ControlFailure::Encryption {
                member_index,
                failure: *failure,
            }),

            (
                ControlPhase::AwaitingSetConnected,
                HciReport::ConnectionSet(snapshot),
            ) => {
                if !self.connection_set_matches(snapshot) {
                    return self.fail(
                        ControlFailure::ConnectionSetMismatch,
                    );
                }
                self.phase = ControlPhase::SetConnected;
                Ok(Vec::new())
            }

            _ => Ok(Vec::new()),
        }
    }

    fn begin_member_connection(
        &mut self,
        member_index: usize,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let request = self
            .plan
            .connect_request(member_index)
            .map_err(ControlFailure::Plan)?;
        self.phase = ControlPhase::ConnectingMember {
            member_index,
        };
        Ok(vec![ControlAction::BeginFalConnection {
            member_index,
            request,
        }])
    }

    fn connection_set_matches(
        &self,
        snapshot: &LeConnectionSetSnapshot,
    ) -> bool {
        if snapshot.connections.len() != self.plan.members.len() {
            return false;
        }

        self.connections.iter().all(|expected| {
            let Some(expected) = expected else {
                return false;
            };
            snapshot.connections.iter().any(|actual| {
                actual.token() == *expected
                    && actual.encryption_enabled != 0
            })
        })
    }

    fn timer_matches_phase(&self, timer: ControlTimer) -> bool {
        matches!(
            (self.phase, timer),
            (ControlPhase::Attaching, ControlTimer::Attach)
                | (
                    ControlPhase::ReadingControllerIdentity,
                    ControlTimer::ControllerIdentity
                )
                | (
                    ControlPhase::EnsuringCisHostFeature,
                    ControlTimer::CisHostFeature
                )
                | (
                    ControlPhase::StartingDiscovery,
                    ControlTimer::ScanStart
                )
                | (
                    ControlPhase::DiscoveringMembers,
                    ControlTimer::DiscoveryWindow
                )
                | (
                    ControlPhase::StoppingDiscovery,
                    ControlTimer::ScanStop
                )
                | (
                    ControlPhase::ConfiguringPrivacy,
                    ControlTimer::Privacy
                )
        )
    }

    fn fail(
        &mut self,
        failure: ControlFailure,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        self.phase = ControlPhase::Faulted;
        self.failure = Some(failure.clone());
        Err(failure)
    }
}

#[cfg(test)]
mod tests {
    use bumble::Address;
    use bumble_smp::privacy::resolvable_private_address;

    use crate::{
        control::plan::ControlMemberPlan,
        hci_host::{
            engine::{
                ControllerCapabilities, ControllerIdentity,
                PacketPoolSnapshot,
            },
            scan::{ScanObservation, ScanSummary},
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
            local_irk: [0x44; 16],
            members: vec![
                member(0x10, [0x21; 16]),
                member(0x20, [0x31; 16]),
            ],
        }
    }

    fn capabilities() -> ControllerCapabilities {
        ControllerCapabilities {
            supported_commands: [0; 64],
            hci_version: Some(13),
            hci_subversion: Some(1),
            manufacturer: Some(2),
            local_le_features: vec![0; 8],
            local_le_features_max_page: None,
            le_acl_pool: Some(PacketPoolSnapshot {
                data_packet_length: 251,
                total_num_data_packets: 8,
            }),
            iso_pool: Some(PacketPoolSnapshot {
                data_packet_length: 248,
                total_num_data_packets: 8,
            }),
            number_of_supported_advertising_sets: 1,
            maximum_advertising_data_length: 1650,
        }
    }

    fn observation(address: Address, rssi: i8) -> ScanObservation {
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
            is_connectable: true,
            is_scannable: false,
            raw_data: vec![],
        }
    }

    #[test]
    fn happy_path_reaches_privacy_configuration_without_blocking_waits() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();

        assert_eq!(
            machine.start().unwrap(),
            vec![
                ControlAction::BeginOperationalAttach,
                ControlAction::ArmTimer(ControlTimer::Attach),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::CapabilitiesReady(
                capabilities(),
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::Attach),
                ControlAction::ReadControllerIdentity,
                ControlAction::ArmTimer(
                    ControlTimer::ControllerIdentity
                ),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::ControllerIdentity(Ok(
                ControllerIdentity {
                    public_address_le:
                        value.controller_identity_le,
                },
            )))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::DisarmTimer(
                    ControlTimer::ControllerIdentity
                ),
                ControlAction::EnsureCisHostFeature,
                ControlAction::ArmTimer(
                    ControlTimer::CisHostFeature
                )
            ]
        ));

        let actions = machine
            .on_hci_report(&HciReport::CisHostFeatureState(
                CisHostFeatureState::Ready {
                    initially_enabled: false,
                    set_command_sent: true,
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::CisHostFeature
                ),
                ControlAction::StartPassiveExtendedScan,
                ControlAction::ArmTimer(ControlTimer::ScanStart),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::PassiveScanState(
                PassiveScanState::Scanning,
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::ScanStart),
                ControlAction::ArmTimer(
                    ControlTimer::DiscoveryWindow
                ),
            ]
        );

        let actions = machine
            .on_event(ControlEvent::DiscoveryWindowElapsed)
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::DiscoveryWindow
                ),
                ControlAction::StopPassiveExtendedScan,
                ControlAction::ArmTimer(ControlTimer::ScanStop),
            ]
        );

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
                observation(left, -40),
                observation(right, -35),
            ],
            truncated: false,
        };

        let actions = machine
            .on_hci_report(&HciReport::ScanComplete(scan))
            .unwrap();
        assert_eq!(machine.phase(), ControlPhase::ConfiguringPrivacy);
        assert_eq!(machine.resolved_members().len(), 2);
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::DisarmTimer(ControlTimer::ScanStop),
                ControlAction::ConfigureControllerPrivacy(_),
                ControlAction::ArmTimer(ControlTimer::Privacy),
            ]
        ));
    }

    fn drive_to_configuring_privacy(
        machine: &mut ControlMachine,
        value: &ControlSetPlan,
    ) {
        machine.start().unwrap();
        machine
            .on_hci_report(&HciReport::CapabilitiesReady(
                capabilities(),
            ))
            .unwrap();
        machine
            .on_hci_report(&HciReport::ControllerIdentity(Ok(
                ControllerIdentity {
                    public_address_le:
                        value.controller_identity_le,
                },
            )))
            .unwrap();
        machine
            .on_hci_report(&HciReport::CisHostFeatureState(
                CisHostFeatureState::Ready {
                    initially_enabled: true,
                    set_command_sent: false,
                },
            ))
            .unwrap();
        machine
            .on_hci_report(&HciReport::PassiveScanState(
                PassiveScanState::Scanning,
            ))
            .unwrap();
        machine
            .on_event(ControlEvent::DiscoveryWindowElapsed)
            .unwrap();

        let left = resolvable_private_address(
            &value.members[0].peer_irk,
            [1, 2, 3],
        );
        let right = resolvable_private_address(
            &value.members[1].peer_irk,
            [4, 5, 6],
        );
        machine
            .on_hci_report(&HciReport::ScanComplete(ScanSummary {
                reports_seen: 2,
                observations: vec![
                    observation(left, -40),
                    observation(right, -35),
                ],
                truncated: false,
            }))
            .unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringPrivacy
        );
    }

    fn connection(
        handle: u16,
        generation: u64,
        encrypted: u8,
    ) -> crate::hci_host::connection::LeConnectionSnapshot {
        crate::hci_host::connection::LeConnectionSnapshot {
            connection_handle: handle,
            generation,
            role: 0,
            peer_address_le: [0; 6],
            peer_address_type: 1,
            connection_interval: 24,
            peripheral_latency: 0,
            supervision_timeout: 42,
            encryption_enabled: encrypted,
            encryption_key_size: 0,
        }
    }

    #[test]
    fn privacy_dual_acl_and_stored_ltk_reach_set_connected() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();
        drive_to_configuring_privacy(&mut machine, &value);

        let actions = machine
            .on_hci_report(&HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            ))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [ControlAction::BeginFalConnection {
                member_index: 0,
                ..
            }]
        ));

        let first = connection(0x0800, 1, 0);
        let first_token = first.token();
        let actions = machine
            .on_hci_report(&HciReport::FalConnectionState(
                FalConnectionState::Connected(first),
            ))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [ControlAction::BeginStoredEncryption {
                member_index: 0,
                request,
            }] if request.connection == first_token
        ));

        let actions = machine
            .on_hci_report(&HciReport::StoredEncryptionState(
                StoredEncryptionState::Encrypted {
                    connection: first_token,
                    encryption_key_size: 0,
                },
            ))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [ControlAction::BeginFalConnection {
                member_index: 1,
                ..
            }]
        ));

        let second = connection(0x0801, 2, 0);
        let second_token = second.token();
        let actions = machine
            .on_hci_report(&HciReport::FalConnectionState(
                FalConnectionState::Connected(second),
            ))
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [ControlAction::BeginStoredEncryption {
                member_index: 1,
                request,
            }] if request.connection == second_token
        ));

        assert!(machine
            .on_hci_report(&HciReport::StoredEncryptionState(
                StoredEncryptionState::Encrypted {
                    connection: second_token,
                    encryption_key_size: 0,
                },
            ))
            .unwrap()
            .is_empty());
        assert_eq!(
            machine.phase(),
            ControlPhase::AwaitingSetConnected
        );

        let snapshot = LeConnectionSetSnapshot {
            connections: vec![
                connection(0x0800, 1, 1),
                connection(0x0801, 2, 1),
            ],
        };
        assert!(machine
            .on_hci_report(&HciReport::ConnectionSet(snapshot))
            .unwrap()
            .is_empty());
        assert_eq!(machine.phase(), ControlPhase::SetConnected);
        assert_eq!(
            machine.connections(),
            [Some(first_token), Some(second_token)]
        );
    }

    #[test]
    fn controller_identity_mismatch_faults_before_radio_session_work() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();
        machine.start().unwrap();
        machine
            .on_hci_report(&HciReport::CapabilitiesReady(
                capabilities(),
            ))
            .unwrap();

        let error = machine
            .on_hci_report(&HciReport::ControllerIdentity(Ok(
                ControllerIdentity {
                    public_address_le: [9; 6],
                },
            )))
            .unwrap_err();

        assert_eq!(
            error,
            ControlFailure::ControllerIdentityMismatch {
                expected: value.controller_identity_le,
                actual: [9; 6],
            }
        );
        assert_eq!(machine.phase(), ControlPhase::Faulted);
    }

    #[test]
    fn stale_timer_event_is_ignored_after_phase_advance() {
        let mut machine = ControlMachine::new(plan()).unwrap();
        machine.start().unwrap();
        machine
            .on_hci_report(&HciReport::CapabilitiesReady(
                capabilities(),
            ))
            .unwrap();

        assert_eq!(
            machine
                .on_event(ControlEvent::TimerExpired(
                    ControlTimer::Attach
                ))
                .unwrap(),
            Vec::<ControlAction>::new()
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::ReadingControllerIdentity
        );
    }

    #[test]
    fn active_stage_timeout_is_explicit_fault() {
        let mut machine = ControlMachine::new(plan()).unwrap();
        machine.start().unwrap();

        let error = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::Attach,
            ))
            .unwrap_err();
        assert_eq!(
            error,
            ControlFailure::Timeout(ControlTimer::Attach)
        );
        assert_eq!(machine.phase(), ControlPhase::Faulted);
    }

}
