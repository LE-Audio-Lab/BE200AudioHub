    use bumble::{Address, Uuid};
    use bumble_smp::privacy::resolvable_private_address;

    use crate::{
        plane::l1_control::{
            ascs::operation::AseOperationPhase,
            discovery::snapshot::AudioMemberSnapshot,
            model::set_plan::ControlMemberPlan,
        },
        plane::l1_hci::{
            host::engine::{
                ControllerCapabilities, ControllerIdentity, PacketPoolSnapshot,
            },
            acl::{
            scan::{ScanObservation, ScanSummary},
            security::StoredLeKeyMaterial,
            },
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
    ) -> crate::plane::l1_hci::acl::connection::LeConnectionSnapshot {
        crate::plane::l1_hci::acl::connection::LeConnectionSnapshot {
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
            [
                ControlAction::DisarmTimer(ControlTimer::Privacy),
                ControlAction::BeginFalConnection {
                    member_index: 0,
                    ..
                },
                ControlAction::ArmTimer(
                    ControlTimer::Connect { member_index: 0 }
                )
            ]
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
            [
                ControlAction::DisarmTimer(
                    ControlTimer::Connect { member_index: 0 }
                ),
                ControlAction::BeginStoredEncryption {
                    member_index: 0,
                    request,
                },
                ControlAction::ArmTimer(
                    ControlTimer::Encryption { member_index: 0 }
                )
            ] if request.connection == first_token
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
            [
                ControlAction::DisarmTimer(
                    ControlTimer::Encryption { member_index: 0 }
                ),
                ControlAction::BeginFalConnection {
                    member_index: 1,
                    ..
                },
                ControlAction::ArmTimer(
                    ControlTimer::Connect { member_index: 1 }
                )
            ]
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
            [
                ControlAction::DisarmTimer(
                    ControlTimer::Connect { member_index: 1 }
                ),
                ControlAction::BeginStoredEncryption {
                    member_index: 1,
                    request,
                },
                ControlAction::ArmTimer(
                    ControlTimer::Encryption { member_index: 1 }
                )
            ] if request.connection == second_token
        ));

        assert_eq!(
            machine
                .on_hci_report(&HciReport::StoredEncryptionState(
                    StoredEncryptionState::Encrypted {
                        connection: second_token,
                        encryption_key_size: 0,
                    },
                ))
                .unwrap(),
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::Encryption { member_index: 1 },
                ),
                ControlAction::ArmTimer(
                    ControlTimer::SetProjection,
                ),
            ]
        );
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
        assert_eq!(
            machine
                .on_hci_report(&HciReport::ConnectionSet(snapshot))
                .unwrap(),
            vec![ControlAction::DisarmTimer(
                ControlTimer::SetProjection,
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::SetConnected);
        assert_eq!(
            machine.connections(),
            [Some(first_token), Some(second_token)]
        );
    }

    #[test]
    fn second_member_encryption_failure_disconnects_known_acls_reverse_then_clears_privacy() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();
        drive_to_configuring_privacy(&mut machine, &value);
        machine
            .on_hci_report(&HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            ))
            .unwrap();

        let first = connection(0x0800, 1, 0);
        let first_token = first.token();
        machine
            .on_hci_report(&HciReport::FalConnectionState(
                FalConnectionState::Connected(first),
            ))
            .unwrap();
        machine
            .on_hci_report(&HciReport::StoredEncryptionState(
                StoredEncryptionState::Encrypted {
                    connection: first_token,
                    encryption_key_size: 0,
                },
            ))
            .unwrap();

        let second = connection(0x0801, 2, 0);
        let second_token = second.token();
        machine
            .on_hci_report(&HciReport::FalConnectionState(
                FalConnectionState::Connected(second),
            ))
            .unwrap();

        let failure = StoredEncryptionFailure {
            connection: second_token,
            kind: crate::plane::l1_hci::acl::security::
                StoredEncryptionFailureKind::EncryptionNotEnabled,
        };
        let actions = machine
            .on_hci_report(&HciReport::StoredEncryptionState(
                StoredEncryptionState::Failed(failure),
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisconnectAcl {
                    connection: second_token,
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupDisconnect(second_token),
                ),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::DisconnectState(
                DisconnectState::Disconnected {
                    connection: second_token,
                    event_reason: 0x16,
                },
            ))
            .unwrap();
        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupDisconnect(second_token),
            )
        );
        assert!(matches!(
            actions[1],
            ControlAction::DisconnectAcl {
                connection
            } if connection == first_token
        ));

        let actions = machine
            .on_hci_report(&HciReport::DisconnectState(
                DisconnectState::Disconnected {
                    connection: first_token,
                    event_reason: 0x16,
                },
            ))
            .unwrap();
        assert!(actions.contains(
            &ControlAction::ClearControllerPrivacy
        ));

        let final_error = machine
            .on_hci_report(&HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Idle,
            ))
            .unwrap_err();
        assert_eq!(
            final_error,
            ControlFailure::Encryption {
                member_index: 1,
                failure,
            }
        );
        assert_eq!(machine.phase(), ControlPhase::Faulted);
        assert_eq!(machine.connections(), [None, None]);
    }

    #[test]
    fn connect_timeout_cancels_before_acl_cleanup() {
        let value = plan();
        let mut machine = ControlMachine::new(value.clone()).unwrap();
        drive_to_configuring_privacy(&mut machine, &value);
        machine
            .on_hci_report(&HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            ))
            .unwrap();

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::Connect { member_index: 0 },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::CancelFalConnection,
                ControlAction::ArmTimer(
                    ControlTimer::CancelConnect {
                        member_index: 0,
                    },
                ),
            ]
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::CancellingConnection {
                member_index: 0,
            }
        );
    }

    fn machine_at_set_connected() -> ControlMachine {
        let mut machine = ControlMachine::new(plan()).unwrap();
        machine.phase = ControlPhase::SetConnected;
        machine.confirmed.privacy_active = true;
        machine.confirmed.connections = [
            Some(ConnectionToken {
                connection_handle: 0x0800,
                generation: 1,
            }),
            Some(ConnectionToken {
                connection_handle: 0x0801,
                generation: 2,
            }),
        ];
        machine
    }

    fn integration_planned_sink(
        side: AudioSide,
        member_index: usize,
        connection_handle: u16,
        generation: u64,
        location: bumble_profiles::bap::AudioLocation,
    ) -> crate::plane::l1_control::planning::codec::PlannedSink {
        use bumble_hci::CodingFormat;
        use bumble_profiles::bap::{
            CodecSpecificConfiguration, FrameDuration,
            SamplingFrequency,
        };
        use crate::plane::l1_control::discovery::snapshot::GattCharacteristicRange;

        crate::plane::l1_control::planning::codec::PlannedSink {
            side,
            member_index,
            connection: ConnectionToken {
                connection_handle,
                generation,
            },
            ase_id: 1,
            ase_value_handle: 0x0046,
            ase_end_group_handle: 0x0048,
            ascs_control_point: GattCharacteristicRange {
                declaration_handle: 0x0042,
                value_handle: 0x0043,
                end_group_handle: 0x0045,
                properties: 0x12,
            },
            audio_location: location,
            codec_id: CodingFormat::LC3,
            codec_specific_configuration:
                CodecSpecificConfiguration {
                    sampling_frequency: Some(
                        SamplingFrequency::FREQ_48000,
                    ),
                    frame_duration: Some(
                        FrameDuration::DURATION_10000_US,
                    ),
                    audio_channel_allocation: Some(location),
                    octets_per_codec_frame: Some(100),
                    codec_frames_per_sdu: Some(1),
                },
        }
    }

    fn machine_at_codec_planned() -> ControlMachine {
        use bumble_profiles::bap::{
            AudioLocation, ContextType,
        };
        use crate::plane::l1_control::interface::stream::LeAudioFormat;

        let mut machine = machine_at_set_connected();
        machine.phase = ControlPhase::CodecPlanned;
        machine.planned.codec_plan = Some(StereoCodecPlan {
            format: LeAudioFormat {
                sample_rate_hz: 48_000,
                frame_duration_us: 10_000,
                octets_per_sdu: 100,
            },
            context: ContextType::MEDIA,
            left: integration_planned_sink(
                AudioSide::Left,
                1,
                0x0801,
                2,
                AudioLocation::FRONT_LEFT,
            ),
            right: integration_planned_sink(
                AudioSide::Right,
                0,
                0x0800,
                1,
                AudioLocation::FRONT_RIGHT,
            ),
        });
        machine
    }

    fn only_begin_gatt(actions: &[ControlAction]) -> GattRequest {
        let requests = actions
            .iter()
            .filter_map(|action| match action {
                ControlAction::BeginGatt(request) => {
                    Some(request.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1, "expected one BeginGatt: {actions:?}");
        requests.into_iter().next().unwrap()
    }

    fn complete_gatt(
        machine: &mut ControlMachine,
        request: &GattRequest,
        result: crate::plane::l1_hci::gatt::transaction::GattResult,
    ) -> Vec<ControlAction> {
        machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Complete {
                    request_id: request.request_id,
                    connection: request.connection,
                    result,
                },
            ))
            .unwrap()
    }

    fn codec_status_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseEndpoint, AudioRole,
        };

        let planned = machine.planned_sink(side).unwrap();
        let mut endpoint =
            AseEndpoint::new(planned.ase_id, AudioRole::Sink);
        endpoint.state = AseState::CODEC_CONFIGURED;
        endpoint.codec_id = planned.codec_id;
        endpoint.codec_specific_configuration =
            planned.codec_specific_configuration.to_bytes();
        endpoint.value().unwrap()
    }

    fn codec_cp_success_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        AseControlResponse {
            opcode: AseOpcode::ConfigCodec,
            responses: vec![AseResponse {
                ase_id: machine.planned_sink(side).unwrap().ase_id,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    fn drive_ase_prepare(
        machine: &mut ControlMachine,
        mut actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::plane::l1_hci::gatt::transaction::{
            GattDescriptor, GattOperation, GattResult,
        };

        let request = only_begin_gatt(&actions);
        let ase_cccd = match request.operation {
            GattOperation::DiscoverDescriptors {
                ending_handle, ..
            } => ending_handle,
            ref other => panic!(
                "expected ASE descriptor discovery, got {other:?}"
            ),
        };
        actions = complete_gatt(
            machine,
            &request,
            GattResult::Descriptors(vec![GattDescriptor {
                handle: ase_cccd,
                uuid: Uuid::from_16_bits(0x2902),
            }]),
        );

        let request = only_begin_gatt(&actions);
        let cp_cccd = match request.operation {
            GattOperation::DiscoverDescriptors {
                ending_handle, ..
            } => ending_handle,
            ref other => panic!(
                "expected CP descriptor discovery, got {other:?}"
            ),
        };
        actions = complete_gatt(
            machine,
            &request,
            GattResult::Descriptors(vec![GattDescriptor {
                handle: cp_cccd,
                uuid: Uuid::from_16_bits(0x2902),
            }]),
        );

        let request = only_begin_gatt(&actions);
        let (value_handle, cccd_handle, indicate) =
            match request.operation {
                GattOperation::Subscribe {
                    value_handle,
                    cccd_handle,
                    indicate,
                } => (value_handle, cccd_handle, indicate),
                ref other => panic!(
                    "expected ASE subscribe, got {other:?}"
                ),
            };
        actions = complete_gatt(
            machine,
            &request,
            GattResult::Subscription {
                value_handle,
                cccd_handle,
                indicate,
                enabled: true,
            },
        );

        let request = only_begin_gatt(&actions);
        let (value_handle, cccd_handle, indicate) =
            match request.operation {
                GattOperation::Subscribe {
                    value_handle,
                    cccd_handle,
                    indicate,
                } => (value_handle, cccd_handle, indicate),
                ref other => panic!(
                    "expected CP subscribe, got {other:?}"
                ),
            };
        complete_gatt(
            machine,
            &request,
            GattResult::Subscription {
                value_handle,
                cccd_handle,
                indicate,
                enabled: true,
            },
        )
    }

    fn drive_config_codec(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::plane::l1_hci::gatt::transaction::{
            GattOperation, GattResult, GattValueEvent,
        };

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteCommand {
                attribute_handle: 0x0043,
                ..
            }
        ));

        let _ = complete_gatt(
            machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: false,
            },
        );

        assert!(machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0043,
                        value: codec_cp_success_bytes(machine, side),
                    },
                ),
            ))
            .unwrap()
            .is_empty());

        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0046,
                        value: codec_status_bytes(machine, side),
                    },
                ),
            ))
            .unwrap();

        let readback = only_begin_gatt(&actions);
        assert_eq!(
            readback.operation,
            GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        );
        complete_gatt(
            machine,
            &readback,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: codec_status_bytes(machine, side),
            },
        )
    }

    #[test]
    fn codec_configuration_runs_left_then_right_to_codec_configured() {
        let mut machine = machine_at_codec_planned();

        let actions = machine.begin_codec_configuration().unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::PreparingCodecAse {
                side: AudioSide::Left,
            }
        );

        let actions = drive_ase_prepare(&mut machine, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringCodec {
                side: AudioSide::Left,
            }
        );
        assert!(machine.prepared_ase(AudioSide::Left).is_some());

        let actions =
            drive_config_codec(&mut machine, AudioSide::Left, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::PreparingCodecAse {
                side: AudioSide::Right,
            }
        );
        assert!(machine.codec_status(AudioSide::Left).is_some());

        let actions = drive_ase_prepare(&mut machine, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringCodec {
                side: AudioSide::Right,
            }
        );

        let actions =
            drive_config_codec(&mut machine, AudioSide::Right, actions);
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(
                ControlTimer::AseOperation {
                    side: AudioSide::Right
                }
            )
        )));
        assert_eq!(machine.phase(), ControlPhase::CodecConfigured);
        assert!(machine.codec_status(AudioSide::Left).is_some());
        assert!(machine.codec_status(AudioSide::Right).is_some());
    }

    #[test]
    fn codec_semantic_timeout_after_write_command_rolls_back() {
        use crate::plane::l1_hci::gatt::transaction::{
            GattOperation, GattResult,
        };

        let mut machine = machine_at_codec_planned();
        let actions = machine.begin_codec_configuration().unwrap();
        let actions = drive_ase_prepare(&mut machine, actions);

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteCommand { .. }
        ));
        let actions = complete_gatt(
            &mut machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: false,
            },
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(ControlTimer::Gatt { .. })
        )));
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringCodec {
                side: AudioSide::Left,
            }
        );

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::AseOperation {
                    side: AudioSide::Left,
                },
            ))
            .unwrap();
        let disconnect = actions
            .iter()
            .position(|action| matches!(
                action,
                ControlAction::DisconnectAcl { .. }
            ))
            .expect("semantic timeout must begin ACL rollback");
        assert!(actions[..disconnect].iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(
                ControlTimer::AseOperation {
                    side: AudioSide::Left
                }
            )
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::CancelGatt { .. }
        )));
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::AseOperationTimeout {
                side: AudioSide::Left,
            })
        ));
    }

    fn machine_at_codec_configured() -> ControlMachine {
        let mut machine = machine_at_codec_planned();
        machine.phase = ControlPhase::CodecConfigured;
        machine
    }

    fn drive_to_qos_planned() -> ControlMachine {
        let mut machine = machine_at_codec_planned();

        let actions = machine.begin_codec_configuration().unwrap();
        let actions = drive_ase_prepare(&mut machine, actions);
        let actions =
            drive_config_codec(&mut machine, AudioSide::Left, actions);
        let actions = drive_ase_prepare(&mut machine, actions);
        let _actions =
            drive_config_codec(&mut machine, AudioSide::Right, actions);
        assert_eq!(machine.phase(), ControlPhase::CodecConfigured);

        machine.plan_iso().unwrap();
        let _ = machine.begin_cig_configuration().unwrap();
        machine
            .on_hci_report(&HciReport::CigConfigureState(
                CigConfigureState::Ready(CigAllocation {
                    cig_id: 1,
                    cis: vec![
                        crate::plane::l1_hci::iso::cig_cis::CisAllocation {
                            cis_id: 1,
                            connection_handle: 0x0900,
                        },
                        crate::plane::l1_hci::iso::cig_cis::CisAllocation {
                            cis_id: 2,
                            connection_handle: 0x0901,
                        },
                    ],
                }),
            ))
            .unwrap();
        assert_eq!(machine.phase(), ControlPhase::CigConfigured);

        machine.plan_qos().unwrap();
        assert_eq!(machine.phase(), ControlPhase::QosPlanned);
        machine
    }

    fn qos_status_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseEndpoint, AudioRole,
        };

        let planned = match side {
            AudioSide::Left => &machine.planned.qos_plan().unwrap().left,
            AudioSide::Right => &machine.planned.qos_plan().unwrap().right,
        };
        let mut endpoint =
            AseEndpoint::new(planned.parameters.ase_id, AudioRole::Sink);
        endpoint.state = AseState::QOS_CONFIGURED;
        endpoint.qos = planned.expected_readback();
        endpoint.value().unwrap()
    }

    fn qos_cp_success_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        let ase_id = match side {
            AudioSide::Left => {
                machine.planned.qos_plan().unwrap().left.parameters.ase_id
            }
            AudioSide::Right => {
                machine.planned.qos_plan().unwrap().right.parameters.ase_id
            }
        };
        AseControlResponse {
            opcode: AseOpcode::ConfigQos,
            responses: vec![AseResponse {
                ase_id,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    fn drive_config_qos(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::plane::l1_hci::gatt::transaction::{
            GattOperation, GattResult, GattValueEvent,
        };

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteCommand {
                attribute_handle: 0x0043,
                ..
            }
        ));

        let actions = complete_gatt(
            machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: false,
            },
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(ControlTimer::Gatt { .. })
        )));

        assert!(machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0043,
                        value: qos_cp_success_bytes(machine, side),
                    },
                ),
            ))
            .unwrap()
            .is_empty());

        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0046,
                        value: qos_status_bytes(machine, side),
                    },
                ),
            ))
            .unwrap();
        let readback = only_begin_gatt(&actions);
        assert_eq!(
            readback.operation,
            GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        );

        complete_gatt(
            machine,
            &readback,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: qos_status_bytes(machine, side),
            },
        )
    }

    #[test]
    fn qos_configuration_runs_left_then_right_to_qos_configured() {
        let mut machine = drive_to_qos_planned();

        let actions = machine.begin_qos_configuration().unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringQos {
                side: AudioSide::Left,
            }
        );

        let actions =
            drive_config_qos(&mut machine, AudioSide::Left, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::ConfiguringQos {
                side: AudioSide::Right,
            }
        );
        assert!(machine.qos_status(AudioSide::Left).is_some());

        let actions =
            drive_config_qos(&mut machine, AudioSide::Right, actions);
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(
                ControlTimer::AseOperation {
                    side: AudioSide::Right,
                }
            )
        )));
        assert_eq!(machine.phase(), ControlPhase::QosConfigured);
        assert!(machine.qos_status(AudioSide::Left).is_some());
        assert!(machine.qos_status(AudioSide::Right).is_some());
    }

    fn drive_to_enable_planned() -> ControlMachine {
        let mut machine = drive_to_qos_planned();

        let actions = machine.begin_qos_configuration().unwrap();
        let actions =
            drive_config_qos(&mut machine, AudioSide::Left, actions);
        let _actions =
            drive_config_qos(&mut machine, AudioSide::Right, actions);
        assert_eq!(machine.phase(), ControlPhase::QosConfigured);

        machine.plan_enable().unwrap();
        assert_eq!(machine.phase(), ControlPhase::EnablePlanned);
        machine
    }

    fn enable_status_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::{
            ascs::{AseEndpoint, AudioRole},
            le_audio::Metadata,
        };

        let planned = match side {
            AudioSide::Left => &machine.planned.enable_plan().unwrap().left,
            AudioSide::Right => &machine.planned.enable_plan().unwrap().right,
        };
        let qos = match side {
            AudioSide::Left => {
                &machine.planned.qos_plan().unwrap().left
            }
            AudioSide::Right => {
                &machine.planned.qos_plan().unwrap().right
            }
        };

        let mut endpoint =
            AseEndpoint::new(planned.ase_id, AudioRole::Sink);
        endpoint.state = AseState::ENABLING;
        endpoint.qos = qos.expected_readback();
        endpoint.metadata =
            Metadata::from_bytes(&planned.metadata).unwrap();
        endpoint.value().unwrap()
    }

    fn enable_cp_success_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };

        let ase_id = match side {
            AudioSide::Left => {
                machine.planned.enable_plan().unwrap().left.ase_id
            }
            AudioSide::Right => {
                machine.planned.enable_plan().unwrap().right.ase_id
            }
        };

        AseControlResponse {
            opcode: AseOpcode::Enable,
            responses: vec![AseResponse {
                ase_id,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    fn drive_enable(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::plane::l1_hci::gatt::transaction::{
            GattOperation, GattResult, GattValueEvent,
        };

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteCommand {
                attribute_handle: 0x0043,
                ..
            }
        ));

        let actions = complete_gatt(
            machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: false,
            },
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(ControlTimer::Gatt { .. })
        )));

        assert!(machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0043,
                        value: enable_cp_success_bytes(machine, side),
                    },
                ),
            ))
            .unwrap()
            .is_empty());

        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0046,
                        value: enable_status_bytes(machine, side),
                    },
                ),
            ))
            .unwrap();

        let readback = only_begin_gatt(&actions);
        assert_eq!(
            readback.operation,
            GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        );

        complete_gatt(
            machine,
            &readback,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: enable_status_bytes(machine, side),
            },
        )
    }

    #[test]
    fn enable_runs_left_then_right_to_enabled() {
        use bumble_profiles::{
            bap::ContextType,
            le_audio::MetadataTag,
        };

        let mut machine = drive_to_enable_planned();

        let actions = machine.begin_enable().unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::Enabling {
                side: AudioSide::Left,
            }
        );

        let actions =
            drive_enable(&mut machine, AudioSide::Left, actions);
        assert_eq!(
            machine.phase(),
            ControlPhase::Enabling {
                side: AudioSide::Right,
            }
        );
        assert!(machine.enable_status(AudioSide::Left).is_some());

        let actions =
            drive_enable(&mut machine, AudioSide::Right, actions);
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisarmTimer(
                ControlTimer::AseOperation {
                    side: AudioSide::Right,
                }
            )
        )));
        assert_eq!(machine.phase(), ControlPhase::Enabled);
        assert!(machine.enable_status(AudioSide::Left).is_some());
        assert!(machine.enable_status(AudioSide::Right).is_some());

        for side in [AudioSide::Left, AudioSide::Right] {
            let status = machine.enable_status(side).unwrap();
            let AseStatusDetails::Metadata {
                metadata, ..
            } = &status.details
            else {
                panic!("expected Enable metadata readback");
            };
            assert!(metadata.entries.iter().any(|entry| {
                entry.tag == MetadataTag::STREAMING_AUDIO_CONTEXTS
                    && entry.data
                        == ContextType::MEDIA.0.to_le_bytes()
            }));
        }
    }

    fn drive_to_enabled() -> ControlMachine {
        let mut machine = drive_to_enable_planned();
        let actions = machine.begin_enable().unwrap();
        let actions =
            drive_enable(&mut machine, AudioSide::Left, actions);
        let _actions =
            drive_enable(&mut machine, AudioSide::Right, actions);
        assert_eq!(machine.phase(), ControlPhase::Enabled);
        machine
    }

    fn release_cp_success_bytes(
        machine: &ControlMachine,
        side: AudioSide,
    ) -> Vec<u8> {
        use bumble_profiles::ascs::{
            AseControlResponse, AseOpcode, AseReasonCode,
            AseResponse, AseResponseCode,
        };
        AseControlResponse {
            opcode: AseOpcode::Release,
            responses: vec![AseResponse {
                ase_id: machine.planned_sink(side).unwrap().ase_id,
                code: AseResponseCode::SUCCESS,
                reason: AseReasonCode::NONE,
            }],
        }
        .to_bytes()
        .unwrap()
    }

    fn drive_rollback_release(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::plane::l1_hci::gatt::transaction::{
            GattOperation, GattResult, GattValueEvent,
        };

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteCommand {
                attribute_handle: 0x0043,
                ..
            }
        ));

        let _ = complete_gatt(
            machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: false,
            },
        );

        assert!(machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0043,
                        value: release_cp_success_bytes(machine, side),
                    },
                ),
            ))
            .unwrap()
            .is_empty());

        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0046,
                        value: vec![
                            machine.planned_sink(side).unwrap().ase_id,
                            AseState::RELEASING.0,
                        ],
                    },
                ),
            ))
            .unwrap();

        let readback = only_begin_gatt(&actions);
        assert_eq!(
            readback.operation,
            GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        );

        complete_gatt(
            machine,
            &readback,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: vec![
                    machine.planned_sink(side).unwrap().ase_id,
                    AseState::IDLE.0,
                ],
            },
        )
    }

    fn drive_graceful_release_gate(
        machine: &mut ControlMachine,
        side: AudioSide,
        actions: Vec<ControlAction>,
    ) -> Vec<ControlAction> {
        use crate::plane::l1_hci::gatt::transaction::{
            GattOperation, GattResult, GattValueEvent,
        };

        let write = only_begin_gatt(&actions);
        assert!(matches!(
            write.operation,
            GattOperation::WriteCommand {
                attribute_handle: 0x0043,
                ..
            }
        ));

        let _ = complete_gatt(
            machine,
            &write,
            GattResult::WriteComplete {
                attribute_handle: 0x0043,
                with_response: false,
            },
        );

        assert!(machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0043,
                        value: release_cp_success_bytes(machine, side),
                    },
                ),
            ))
            .unwrap()
            .is_empty());

        machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Value(
                    GattValueEvent::Notification {
                        connection: write.connection,
                        attribute_handle: 0x0046,
                        value: vec![
                            machine.planned_sink(side).unwrap().ase_id,
                            AseState::RELEASING.0,
                        ],
                    },
                ),
            ))
            .unwrap()
    }

    #[test]
    fn create_cis_success_uses_only_semantic_cis_refs() {
        let mut machine = drive_to_enabled();
        let plan = machine.planned.iso_plan().unwrap().clone();

        let actions = machine.begin_cis_creation().unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::CreateCis(
                    plan.cis_create_request(),
                ),
                ControlAction::ArmTimer(ControlTimer::CisCreate),
            ]
        );
        assert_eq!(machine.phase(), ControlPhase::CreatingCis);

        assert!(machine
            .on_hci_report(&HciReport::SemanticCisCreateState(
                SemanticCisCreateState::AwaitingEstablished {
                    cig_id: 1,
                    established_cis_ids: vec![2],
                },
            ))
            .unwrap()
            .is_empty());
        assert_eq!(
            machine.live_cis(),
            &BTreeSet::from([CisRef {
                cig_id: 1,
                cis_id: 2,
            }])
        );

        let actions = machine
            .on_hci_report(&HciReport::SemanticCisCreateState(
                SemanticCisCreateState::Ready {
                    cig_id: 1,
                    established_cis_ids: vec![2, 1],
                },
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::CisCreate,
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::CisEstablished);
        assert_eq!(
            machine.live_cis(),
            &BTreeSet::from([
                CisRef {
                    cig_id: 1,
                    cis_id: 1,
                },
                CisRef {
                    cig_id: 1,
                    cis_id: 2,
                },
            ])
        );
    }

    fn drive_to_cis_established() -> ControlMachine {
        let mut machine = drive_to_enabled();
        machine.begin_cis_creation().unwrap();
        machine
            .on_hci_report(&HciReport::SemanticCisCreateState(
                SemanticCisCreateState::Ready {
                    cig_id: 1,
                    established_cis_ids: vec![1, 2],
                },
            ))
            .unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::CisEstablished
        );
        machine
    }

    #[test]
    fn iso_path_setup_runs_left_then_right_to_ready() {
        let mut machine = drive_to_cis_established();
        let left = CisRef {
            cig_id: 1,
            cis_id: 1,
        };
        let right = CisRef {
            cig_id: 1,
            cis_id: 2,
        };

        let actions = machine.begin_iso_path_setup().unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::SetupIsoInputPathByRef {
                    cis: left,
                },
                ControlAction::ArmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: left,
                    },
                ),
            ]
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::SettingUpIsoPath {
                side: AudioSide::Left,
            }
        );

        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: left,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: left,
                    },
                ),
                ControlAction::SetupIsoInputPathByRef {
                    cis: right,
                },
                ControlAction::ArmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: right,
                    },
                ),
            ]
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::SettingUpIsoPath {
                side: AudioSide::Right,
            }
        );

        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: right,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::IsoPathSetup {
                    cis: right,
                },
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::IsoPathsReady);
        assert_eq!(
            machine.iso_paths_ready(),
            &BTreeSet::from([left, right])
        );
    }

    #[test]
    fn uncertain_right_path_failure_removes_paths_before_any_cis() {
        use crate::plane::l1_hci::iso::path::{
            SemanticIsoPathFailureKind,
        };

        let mut machine = drive_to_cis_established();
        let left = CisRef {
            cig_id: 1,
            cis_id: 1,
        };
        let right = CisRef {
            cig_id: 1,
            cis_id: 2,
        };

        machine.begin_iso_path_setup().unwrap();
        machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: left,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();

        let failure = SemanticIsoPathFailure {
            cis: right,
            may_have_configured: true,
            kind: SemanticIsoPathFailureKind::
                ConnectionHandleMismatch,
        };
        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::Failed(
                    failure.clone(),
                ),
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::IsoPathSetup {
                        cis: right,
                    },
                ),
                ControlAction::RemoveIsoPathByRef {
                    cis: right,
                    directions: 0x01,
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupIsoPath {
                        cis: right,
                    },
                ),
            ]
        );
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::IsoPathSetup {
                cis,
                failure: actual,
            }) if *cis == right && actual == &failure
        ));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::DisconnectCisByRef { .. }
        )));

        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::RemoveComplete {
                    cis: right,
                    directions: 0x01,
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::CleanupIsoPath {
                        cis: right,
                    },
                ),
                ControlAction::RemoveIsoPathByRef {
                    cis: left,
                    directions: 0x01,
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupIsoPath {
                        cis: left,
                    },
                ),
            ]
        );

        let actions = machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::RemoveComplete {
                    cis: left,
                    directions: 0x01,
                },
            ))
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupIsoPath {
                    cis: left,
                },
            )
        );
        assert_eq!(
            actions[1],
            ControlAction::DisconnectCisByRef {
                cis: right,
            }
        );
        assert_eq!(
            actions[2],
            ControlAction::ArmTimer(
                ControlTimer::CleanupCis {
                    cis: right,
                },
            )
        );
        assert!(machine.iso_paths_ready().is_empty());
    }

    fn drive_to_iso_paths_ready() -> ControlMachine {
        let mut machine = drive_to_cis_established();
        let left = CisRef {
            cig_id: 1,
            cis_id: 1,
        };
        let right = CisRef {
            cig_id: 1,
            cis_id: 2,
        };

        machine.begin_iso_path_setup().unwrap();
        machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: left,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();
        machine
            .on_hci_report(&HciReport::SemanticIsoPathState(
                SemanticIsoPathState::SetupComplete {
                    cis: right,
                    direction: IsoDataPathDirection::Input,
                },
            ))
            .unwrap();
        assert_eq!(machine.phase(), ControlPhase::IsoPathsReady);
        machine
    }

    fn streaming_status_bytes(
        machine: &ControlMachine,
        side: AudioSide,
        state: AseState,
    ) -> Vec<u8> {
        use bumble_profiles::{
            ascs::{AseEndpoint, AudioRole},
            le_audio::Metadata,
        };

        let enable = match side {
            AudioSide::Left => &machine.planned.enable_plan().unwrap().left,
            AudioSide::Right => &machine.planned.enable_plan().unwrap().right,
        };
        let qos = match side {
            AudioSide::Left => &machine.planned.qos_plan().unwrap().left,
            AudioSide::Right => &machine.planned.qos_plan().unwrap().right,
        };

        let mut endpoint =
            AseEndpoint::new(enable.ase_id, AudioRole::Sink);
        endpoint.state = state;
        endpoint.qos = qos.expected_readback();
        endpoint.metadata =
            Metadata::from_bytes(&enable.metadata).unwrap();
        endpoint.value().unwrap()
    }

    fn only_read_gatt(actions: &[ControlAction]) -> GattRequest {
        let request = only_begin_gatt(actions);
        assert!(matches!(
            request.operation,
            crate::plane::l1_hci::gatt::transaction::GattOperation::ReadValue {
                attribute_handle: 0x0046,
            }
        ));
        request
    }

    fn drive_to_streaming_ready() -> ControlMachine {
        use crate::plane::l1_hci::gatt::transaction::GattResult;

        let mut machine = drive_to_iso_paths_ready();
        let actions = machine.begin_streaming_gate().unwrap();
        let left_read = only_read_gatt(&actions);
        let left_streaming = streaming_status_bytes(
            &machine,
            AudioSide::Left,
            AseState::STREAMING,
        );
        let actions = complete_gatt(
            &mut machine,
            &left_read,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: left_streaming,
            },
        );

        let right_read = only_read_gatt(&actions);
        let right_streaming = streaming_status_bytes(
            &machine,
            AudioSide::Right,
            AseState::STREAMING,
        );
        complete_gatt(
            &mut machine,
            &right_read,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: right_streaming,
            },
        );

        assert_eq!(machine.phase(), ControlPhase::StreamingReady);
        machine
    }

    #[test]
    fn stream_binding_requires_successful_hci_route_ack() {
        let machine = drive_to_iso_paths_ready();
        assert_eq!(
            machine.stream_binding(),
            Err(ControlFailure::InvalidStartState(
                ControlPhase::IsoPathsReady,
            ))
        );

        let machine = drive_to_streaming_ready();
        assert_eq!(
            machine.stream_binding(),
            Err(ControlFailure::InvalidStartState(
                ControlPhase::StreamingReady,
            ))
        );
    }

    #[test]
    fn route_ack_unlocks_transport_neutral_stream_binding() {
        let mut machine = drive_to_streaming_ready();

        let actions = machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::BindingStreamRoute {
                session_id: SessionId(7),
                stream_id: StreamId(11),
            }
        );
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::BindStreamRoute(
                    StereoStreamRouteBinding {
                        session_id: SessionId(7),
                        stream_id: StreamId(11),
                        left: CisRef {
                            cig_id: 1,
                            cis_id: 1,
                        },
                        right: CisRef {
                            cig_id: 1,
                            cis_id: 2,
                        },
                    }
                ),
                ControlAction::ArmTimer(
                    ControlTimer::StreamRouteBind {
                        session_id: SessionId(7),
                        stream_id: StreamId(11),
                    }
                )
            ]
        ));
        assert!(matches!(
            machine.stream_binding(),
            Err(ControlFailure::InvalidStartState(
                ControlPhase::BindingStreamRoute {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                }
            ))
        ));

        let actions = machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Bound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();
        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::StreamRouteBind {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::StreamBoundReady);

        let binding = machine.stream_binding().unwrap();
        assert_eq!(binding.session_id, SessionId(7));
        assert_eq!(binding.stream_id, StreamId(11));
        assert_eq!(
            binding.format,
            crate::plane::l1_control::interface::stream::LeAudioFormat {
                sample_rate_hz: 48_000,
                frame_duration_us: 10_000,
                octets_per_sdu: 100,
            }
        );
        assert_eq!(binding.presentation_delay_us, 40_000);
        assert_eq!(
            binding.topology,
            StreamTopology::StereoSplit
        );
    }

    #[test]
    fn route_bind_timeout_rolls_back_before_media_handoff() {
        let mut machine = drive_to_streaming_ready();
        machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::StreamRouteBind {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();

        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::StreamRouteTimeout {
                session_id: SessionId(7),
                stream_id: StreamId(11),
            })
        ));
        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(
                    ControlTimer::StreamRouteBind {
                        session_id: SessionId(7),
                        stream_id: StreamId(11),
                    },
                ),
                ControlAction::UnbindStreamRoute {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupStreamRoute {
                        session_id: SessionId(7),
                        stream_id: StreamId(11),
                    },
                ),
            ]
        );
        assert!(machine.stream_binding().is_err());

        let actions = machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Unbound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                    was_bound: true,
                },
            ))
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupStreamRoute {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            )
        );
        assert_eq!(
            actions[1],
            ControlAction::RemoveIsoPathByRef {
                cis: CisRef {
                    cig_id: 1,
                    cis_id: 2,
                },
                directions: 0x01,
            }
        );
        assert_eq!(
            actions[2],
            ControlAction::ArmTimer(
                ControlTimer::CleanupIsoPath {
                    cis: CisRef {
                        cig_id: 1,
                        cis_id: 2,
                    },
                },
            )
        );
        assert!(!actions[..1].iter().any(|action| matches!(
            action,
            ControlAction::DisconnectCisByRef { .. }
                | ControlAction::RemoveCig { .. }
                | ControlAction::DisconnectAcl { .. }
        )));
    }

    #[test]
    fn route_cleanup_timeout_records_failure_then_starts_path_cleanup() {
        let mut machine = drive_to_streaming_ready();
        machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();
        machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::StreamRouteBind {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();

        let cleanup_timer = ControlTimer::CleanupStreamRoute {
            session_id: SessionId(7),
            stream_id: StreamId(11),
        };
        let actions = machine
            .on_event(ControlEvent::TimerExpired(cleanup_timer))
            .unwrap();

        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(machine.rollback.cleanup_failures.contains(
            &ControlCleanupFailure::Timeout(cleanup_timer)
        ));
        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(cleanup_timer)
        );
        assert_eq!(
            actions[1],
            ControlAction::RemoveIsoPathByRef {
                cis: CisRef {
                    cig_id: 1,
                    cis_id: 2,
                },
                directions: 0x01,
            }
        );
        assert_eq!(
            actions[2],
            ControlAction::ArmTimer(
                ControlTimer::CleanupIsoPath {
                    cis: CisRef {
                        cig_id: 1,
                        cis_id: 2,
                    },
                },
            )
        );
    }

    #[test]
    fn streaming_gate_retries_left_then_confirms_both_ears() {
        use crate::plane::l1_hci::gatt::transaction::GattResult;

        let mut machine = drive_to_iso_paths_ready();

        let actions = machine.begin_streaming_gate().unwrap();
        assert_eq!(
            machine.phase(),
            ControlPhase::AwaitingStreaming {
                side: AudioSide::Left,
            }
        );
        assert!(actions.contains(&ControlAction::ArmTimer(
            ControlTimer::StreamingDeadline {
                side: AudioSide::Left,
            },
        )));
        let left_read = only_read_gatt(&actions);

        let left_enabling = streaming_status_bytes(
            &machine,
            AudioSide::Left,
            AseState::ENABLING,
        );
        let actions = complete_gatt(
            &mut machine,
            &left_read,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: left_enabling,
            },
        );
        assert!(actions.contains(&ControlAction::ArmTimer(
            ControlTimer::StreamingProbe {
                side: AudioSide::Left,
            },
        )));

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::StreamingProbe {
                    side: AudioSide::Left,
                },
            ))
            .unwrap();
        let left_confirm = only_read_gatt(&actions);

        let left_streaming = streaming_status_bytes(
            &machine,
            AudioSide::Left,
            AseState::STREAMING,
        );
        let actions = complete_gatt(
            &mut machine,
            &left_confirm,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: left_streaming,
            },
        );
        assert_eq!(
            machine.phase(),
            ControlPhase::AwaitingStreaming {
                side: AudioSide::Right,
            }
        );
        assert!(machine.streaming_status(AudioSide::Left).is_some());
        assert!(actions.contains(&ControlAction::DisarmTimer(
            ControlTimer::StreamingDeadline {
                side: AudioSide::Left,
            },
        )));
        assert!(actions.contains(&ControlAction::ArmTimer(
            ControlTimer::StreamingDeadline {
                side: AudioSide::Right,
            },
        )));

        let right_read = only_read_gatt(&actions);
        let right_streaming = streaming_status_bytes(
            &machine,
            AudioSide::Right,
            AseState::STREAMING,
        );
        let actions = complete_gatt(
            &mut machine,
            &right_read,
            GattResult::Value {
                attribute_handle: 0x0046,
                value: right_streaming,
            },
        );

        assert_eq!(machine.phase(), ControlPhase::StreamingReady);
        assert!(machine.streaming_status(AudioSide::Left).is_some());
        assert!(machine.streaming_status(AudioSide::Right).is_some());
        assert!(actions.contains(&ControlAction::DisarmTimer(
            ControlTimer::StreamingDeadline {
                side: AudioSide::Right,
            },
        )));
    }

    #[test]
    fn streaming_deadline_cancels_read_then_rolls_back_paths_first() {
        let mut machine = drive_to_iso_paths_ready();
        let actions = machine.begin_streaming_gate().unwrap();
        let read = only_read_gatt(&actions);

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::StreamingDeadline {
                    side: AudioSide::Left,
                },
            ))
            .unwrap();

        let cancel_index = actions
            .iter()
            .position(|action| {
                *action == ControlAction::CancelGatt {
                    connection: read.connection,
                    request_id: read.request_id,
                }
            })
            .expect("deadline must cancel the active GATT read");
        let remove_index = actions
            .iter()
            .position(|action| matches!(
                action,
                ControlAction::RemoveIsoPathByRef {
                    cis: CisRef {
                        cig_id: 1,
                        cis_id: 2,
                    },
                    directions: 0x01,
                }
            ))
            .expect("stream rollback must begin with right ISO path");

        assert!(cancel_index < remove_index);
        assert!(!actions[..remove_index].iter().any(|action| matches!(
            action,
            ControlAction::DisconnectCisByRef { .. }
                | ControlAction::RemoveCig { .. }
                | ControlAction::DisconnectAcl { .. }
        )));
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::StreamingDeadline {
                side: AudioSide::Left,
            })
        ));
    }

    #[test]
    fn partial_cis_failure_disconnects_cis_releases_ases_then_removes_cig() {
        use crate::plane::l1_hci::iso::cig_cis::{
            SemanticCisCreateFailureKind,
            SemanticCisEstablishedFailure,
        };

        let mut machine = drive_to_enabled();
        machine.begin_cis_creation().unwrap();

        let failure = SemanticCisCreateFailure {
            cig_id: 1,
            established_cis_ids: vec![2],
            failed: vec![SemanticCisEstablishedFailure {
                cis_id: 1,
                status: 0x3E,
            }],
            kind: SemanticCisCreateFailureKind::
                CisEstablishmentFailed,
        };
        let actions = machine
            .on_hci_report(&HciReport::SemanticCisCreateState(
                SemanticCisCreateState::Failed(failure.clone()),
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::CisCreate),
                ControlAction::DisconnectCisByRef {
                    cis: CisRef {
                        cig_id: 1,
                        cis_id: 2,
                    },
                },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupCis {
                        cis: CisRef {
                            cig_id: 1,
                            cis_id: 2,
                        },
                    },
                ),
            ]
        );
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::CisCreate(actual))
                if actual == &failure
        ));

        let cis = CisRef {
            cig_id: 1,
            cis_id: 2,
        };
        let actions = machine
            .on_hci_report(
                &HciReport::SemanticCisDisconnectState(
                    SemanticCisDisconnectState::Disconnected {
                        cis,
                        event_reason: 0x16,
                    },
                ),
            )
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupCis { cis },
            )
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::ArmTimer(
                ControlTimer::CleanupAse {
                    side: AudioSide::Right,
                }
            )
        )));
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::BeginGatt(_)
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveCig { .. }
        )));

        let actions = drive_rollback_release(
            &mut machine,
            AudioSide::Right,
            actions,
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::ArmTimer(
                ControlTimer::CleanupAse {
                    side: AudioSide::Left,
                }
            )
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveCig { .. }
        )));

        let actions = drive_rollback_release(
            &mut machine,
            AudioSide::Left,
            actions,
        );
        assert!(actions.iter().any(|action| {
            *action == ControlAction::RemoveCig { cig_id: 1 }
        }));
        let remove_index = actions
            .iter()
            .position(|action| {
                *action == ControlAction::RemoveCig { cig_id: 1 }
            })
            .unwrap();
        assert!(!actions[..remove_index].iter().any(|action| matches!(
            action,
            ControlAction::DisconnectAcl { .. }
        )));
        assert!(machine.live_cis().is_empty());
    }

    #[test]
    fn create_cis_timeout_fails_closed_without_guessing_controller_cleanup() {
        let mut machine = drive_to_enabled();
        machine.begin_cis_creation().unwrap();

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::CisCreate,
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::CisCreate,
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::Faulted);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::CisCreateTimeout {
                cig_id: 1,
            })
        ));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveCig { .. }
                | ControlAction::DisconnectAcl { .. }
        )));
    }

    #[test]
    fn set_cig_success_reaches_cig_configured_without_retaining_raw_handles() {
        let mut machine = machine_at_codec_configured();

        machine.plan_iso().unwrap();
        assert_eq!(machine.phase(), ControlPhase::IsoPlanned);
        let plan = machine.planned.iso_plan().unwrap().clone();
        assert_eq!(plan.cig_id, 1);
        assert_eq!(plan.left.cis_id, 1);
        assert_eq!(plan.right.cis_id, 2);

        let actions = machine.begin_cig_configuration().unwrap();
        assert_eq!(machine.phase(), ControlPhase::ConfiguringCig);
        assert_eq!(
            actions,
            vec![
                ControlAction::ConfigureCig(plan.cig_config()),
                ControlAction::ArmTimer(ControlTimer::CigConfig),
            ]
        );

        let allocation = CigAllocation {
            cig_id: 1,
            cis: vec![
                crate::plane::l1_hci::iso::cig_cis::CisAllocation {
                    cis_id: 1,
                    connection_handle: 0x0900,
                },
                crate::plane::l1_hci::iso::cig_cis::CisAllocation {
                    cis_id: 2,
                    connection_handle: 0x0901,
                },
            ],
        };
        let actions = machine
            .on_hci_report(&HciReport::CigConfigureState(
                CigConfigureState::Ready(allocation),
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![ControlAction::DisarmTimer(
                ControlTimer::CigConfig,
            )]
        );
        assert_eq!(machine.phase(), ControlPhase::CigConfigured);
        assert!(machine.cleanup_knowledge.cig_may_exist);
        // Control retains only the semantic plan. The controller-assigned
        // 0x0900/0x0901 handles from CigAllocation are intentionally not
        // represented in ControlMachine state.
        assert_eq!(machine.planned.iso_plan().unwrap(), &plan);
    }

    #[test]
    fn set_cig_uncertain_failure_removes_cig_before_acl_rollback() {
        let mut machine = machine_at_codec_configured();
        machine.plan_iso().unwrap();
        machine.begin_cig_configuration().unwrap();

        let failure = CigConfigureFailure {
            cig_id: 1,
            may_have_allocated: true,
            kind: crate::plane::l1_hci::iso::cig_cis::CigConfigureFailureKind::
                MalformedResponse,
        };
        let actions = machine
            .on_hci_report(&HciReport::CigConfigureState(
                CigConfigureState::Failed(failure.clone()),
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::CigConfig),
                ControlAction::RemoveCig { cig_id: 1 },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupCig { cig_id: 1 },
                ),
            ]
        );
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::CigConfigure(actual))
                if actual == &failure
        ));

        let actions = machine
            .on_hci_report(&HciReport::CigRemoveState(
                CigRemoveState::Removed { cig_id: 1 },
            ))
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::DisarmTimer(
                ControlTimer::CleanupCig { cig_id: 1 },
            )
        );
        assert_eq!(
            actions[1],
            ControlAction::DisconnectAcl {
                connection: ConnectionToken {
                    connection_handle: 0x0801,
                    generation: 2,
                },
            }
        );
        assert_eq!(
            actions[2],
            ControlAction::ArmTimer(
                ControlTimer::CleanupDisconnect(ConnectionToken {
                    connection_handle: 0x0801,
                    generation: 2,
                }),
            )
        );
        assert!(!machine.cleanup_knowledge.cig_may_exist);
    }

    #[test]
    fn set_cig_timeout_assumes_possible_allocation_and_removes_cig_first() {
        let mut machine = machine_at_codec_configured();
        machine.plan_iso().unwrap();
        machine.begin_cig_configuration().unwrap();

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::CigConfig,
            ))
            .unwrap();

        assert_eq!(
            actions,
            vec![
                ControlAction::DisarmTimer(ControlTimer::CigConfig),
                ControlAction::RemoveCig { cig_id: 1 },
                ControlAction::ArmTimer(
                    ControlTimer::CleanupCig { cig_id: 1 },
                ),
            ]
        );
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(machine.cleanup_knowledge.cig_may_exist);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::CigConfigureTimeout {
                cig_id: 1,
            })
        ));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::DisconnectAcl { .. }
        )));
    }

    #[test]
    fn audio_set_checkpoint_plans_first_stereo_stream() {
        use bumble_hci::CodingFormat;
        use bumble_profiles::{
            ascs::{AseState, AseStatus, AseStatusDetails},
            bap::{
                AudioLocation, CodecSpecificCapabilities, ContextType,
                SupportedFrameDuration, SupportedSamplingFrequency,
            },
            le_audio::Metadata,
            pacs::{AudioContexts, PacCodecCapabilities, PacRecord},
        };
        use crate::plane::l1_control::discovery::snapshot::{
            AscsSinkSnapshot, CasSnapshot, GattServiceRange,
            PacsSinkSnapshot, SinkAseSnapshot,
        };

        let service = GattServiceRange {
            start_handle: 1,
            end_handle: 20,
        };
        let member = |member_index: usize, location: AudioLocation| {
            let contexts = AudioContexts {
                sink: ContextType::MEDIA,
                source: ContextType::PROHIBITED,
            };
            AudioMemberSnapshot {
                member_index,
                connection: ConnectionToken {
                    connection_handle: 0x0800 + member_index as u16,
                    generation: member_index as u64 + 1,
                },
                mtu: 247,
                pacs: PacsSinkSnapshot {
                    service,
                    sink_pac_handle: 2,
                    sink_audio_location_handle: 3,
                    available_contexts_handle: 4,
                    supported_contexts_handle: 5,
                    sink_pac_records: vec![PacRecord {
                        coding_format: CodingFormat::LC3,
                        codec_specific_capabilities:
                            PacCodecCapabilities::Standard(
                                CodecSpecificCapabilities {
                                    supported_sampling_frequencies:
                                        SupportedSamplingFrequency::FREQ_48000,
                                    supported_frame_durations:
                                        SupportedFrameDuration::
                                            DURATION_10000_US_SUPPORTED,
                                    supported_audio_channel_count: vec![1],
                                    min_octets_per_codec_frame: 26,
                                    max_octets_per_codec_frame: 155,
                                    supported_max_codec_frames_per_sdu: 1,
                                },
                            ),
                        metadata: Metadata::default(),
                    }],
                    sink_audio_location: location,
                    available_contexts: contexts,
                    supported_contexts: contexts,
                },
                ascs: AscsSinkSnapshot {
                    service,
                    sink_ases: vec![SinkAseSnapshot {
                        declaration_handle: 6,
                        value_handle: 7,
                        end_group_handle: 8,
                        properties: 0x12,
                        status: AseStatus {
                            ase_id: 1,
                            state: AseState::IDLE,
                            details: AseStatusDetails::None,
                        },
                    }],
                    control_point:
                        crate::plane::l1_control::discovery::snapshot::GattCharacteristicRange {
                            declaration_handle: 8,
                            value_handle: 9,
                            end_group_handle: 10,
                            properties: 0x12,
                        },
                },
                cas: CasSnapshot {
                    service,
                    included_csis: service,
                },
            }
        };

        let mut machine = machine_at_set_connected();
        machine.phase = ControlPhase::AudioSetDiscovered;
        machine.confirmed.audio_set = Some(AudioSetSnapshot {
            members: vec![
                member(0, AudioLocation::FRONT_RIGHT),
                member(1, AudioLocation::FRONT_LEFT),
            ],
        });

        machine.plan_codec().unwrap();
        assert_eq!(machine.phase(), ControlPhase::CodecPlanned);
        let plan = machine.planned.codec_plan().unwrap();
        assert_eq!(plan.left.member_index, 1);
        assert_eq!(plan.right.member_index, 0);
        assert_eq!(plan.format.sample_rate_hz, 48_000);
        assert_eq!(plan.format.frame_duration_us, 10_000);
        assert_eq!(plan.format.octets_per_sdu, 100);
    }

    #[test]
    fn audio_discovery_starts_from_set_connected_on_member_zero() {
        let mut machine = machine_at_set_connected();
        let actions = machine.begin_audio_discovery().unwrap();

        assert_eq!(
            machine.phase(),
            ControlPhase::DiscoveringAudioMember {
                member_index: 0,
            }
        );
        assert!(matches!(
            actions.as_slice(),
            [
                ControlAction::BeginGatt(GattRequest {
                    connection,
                    operation: crate::plane::l1_hci::gatt::transaction::GattOperation::
                        ExchangeMtu { client_rx_mtu: 247 },
                    ..
                }),
                ControlAction::ArmTimer(ControlTimer::Gatt {
                    member_index: 0,
                    ..
                })
            ] if connection.connection_handle == 0x0800
        ));
    }

    #[test]
    fn audio_discovery_timeout_cancels_gatt_before_acl_rollback() {
        let mut machine = machine_at_set_connected();
        let actions = machine.begin_audio_discovery().unwrap();
        let request = match &actions[0] {
            ControlAction::BeginGatt(request) => request.clone(),
            other => panic!("unexpected action {other:?}"),
        };

        let actions = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::Gatt {
                    member_index: 0,
                    request_id: request.request_id,
                },
            ))
            .unwrap();

        assert_eq!(
            actions[0],
            ControlAction::CancelGatt {
                connection: request.connection,
                request_id: request.request_id,
            }
        );
        assert_eq!(
            actions[1],
            ControlAction::DisarmTimer(ControlTimer::Gatt {
                member_index: 0,
                request_id: request.request_id,
            })
        );
        assert!(matches!(
            actions[2],
            ControlAction::DisconnectAcl { connection }
                if connection.connection_handle == 0x0801
        ));
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::AudioDiscoveryTimeout {
                member_index: 0,
                request_id,
            }) if *request_id == request.request_id
        ));
    }

    #[test]
    fn audio_discovery_gatt_failure_rolls_back_connected_set() {
        let mut machine = machine_at_set_connected();
        let actions = machine.begin_audio_discovery().unwrap();
        let request = match &actions[0] {
            ControlAction::BeginGatt(request) => request.clone(),
            other => panic!("unexpected action {other:?}"),
        };

        let failure = crate::plane::l1_hci::gatt::transaction::GattFailure {
            request_id: request.request_id,
            connection: request.connection,
            kind: crate::plane::l1_hci::gatt::transaction::GattFailureKind::Att {
                request_opcode: 0x02,
                attribute_handle: 0,
                error_code: 0x0E,
            },
        };
        let actions = machine
            .on_hci_report(&HciReport::Gatt(
                GattReport::Failed(failure.clone()),
            ))
            .unwrap();

        assert!(matches!(
            actions.first(),
            Some(ControlAction::DisarmTimer(
                ControlTimer::Gatt {
                    member_index: 0,
                    request_id,
                }
            )) if *request_id == request.request_id
        ));
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::DisconnectAcl { connection }
                if connection.connection_handle == 0x0801
        )));
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert!(matches!(
            machine.failure(),
            Some(ControlFailure::AudioDiscovery {
                member_index: 0,
                failure: AudioDiscoveryFailure::Gatt(actual),
            }) if actual == &failure
        ));
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



    #[test]
    fn ready_stage_continuation_starts_audio_discovery_without_core_protocol_command() {
        let mut machine = machine_at_set_connected();
        let actions = machine.continue_ready_stage().unwrap();

        assert_eq!(
            machine.phase(),
            ControlPhase::DiscoveringAudioMember { member_index: 0 }
        );
        assert!(actions.iter().any(|action| {
            matches!(action, ControlAction::BeginGatt(_))
        }));
    }


    #[test]
    fn normal_teardown_releases_ases_before_iso_path_and_cis_cleanup() {
        let mut machine = drive_to_streaming_ready();
        machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();
        machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Bound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();

        let actions = machine.begin_teardown().unwrap();
        assert!(matches!(
            actions.first(),
            Some(ControlAction::UnbindStreamRoute { .. })
        ));

        let actions = machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Unbound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                    was_bound: true,
                },
            ))
            .unwrap();

        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::ArmTimer(
                ControlTimer::CleanupAse {
                    side: AudioSide::Right,
                }
            )
        )));
        assert!(actions.iter().any(|action| {
            matches!(action, ControlAction::BeginGatt(_))
        }));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveIsoPathByRef { .. }
                | ControlAction::DisconnectCisByRef { .. }
        )));

        let actions = drive_graceful_release_gate(
            &mut machine,
            AudioSide::Right,
            actions,
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::ArmTimer(
                ControlTimer::CleanupAse {
                    side: AudioSide::Left,
                }
            )
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveIsoPathByRef { .. }
                | ControlAction::DisconnectCisByRef { .. }
        )));

        let actions = drive_graceful_release_gate(
            &mut machine,
            AudioSide::Left,
            actions,
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveIsoPathByRef { .. }
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::DisconnectCisByRef { .. }
        )));
    }

    #[test]
    fn group_release_ab_fans_out_both_ases_before_waiting_for_releasing() {
        use crate::plane::l1_hci::gatt::transaction::{
            GattOperation, GattResult, GattValueEvent,
        };

        let mut machine = drive_to_streaming_ready();
        machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();
        machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Bound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();

        let actions = machine
            .begin_teardown_with_release_mode(
                GracefulAseReleaseMode::Group,
            )
            .unwrap();
        assert!(matches!(
            actions.first(),
            Some(ControlAction::UnbindStreamRoute { .. })
        ));

        let actions = machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Unbound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                    was_bound: true,
                },
            ))
            .unwrap();

        let writes = actions
            .iter()
            .filter_map(|action| match action {
                ControlAction::BeginGatt(request) => Some(request.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(writes.len(), 2);
        assert!(writes.iter().all(|request| matches!(
            request.operation,
            GattOperation::WriteCommand {
                attribute_handle: 0x0043,
                ..
            }
        )));
        assert!(actions.contains(&ControlAction::ArmTimer(
            ControlTimer::CleanupAse {
                side: AudioSide::Left,
            },
        )));
        assert!(actions.contains(&ControlAction::ArmTimer(
            ControlTimer::CleanupAse {
                side: AudioSide::Right,
            },
        )));
        assert!(!actions.iter().any(|action| matches!(
            action,
            ControlAction::RemoveIsoPathByRef { .. }
                | ControlAction::DisconnectCisByRef { .. }
        )));

        let left_connection =
            machine.planned_sink(AudioSide::Left).unwrap().connection;
        let right_connection =
            machine.planned_sink(AudioSide::Right).unwrap().connection;
        let left = writes
            .iter()
            .find(|request| request.connection == left_connection)
            .unwrap()
            .clone();
        let right = writes
            .iter()
            .find(|request| request.connection == right_connection)
            .unwrap()
            .clone();

        for (side, write, should_start_transport_cleanup) in [
            (AudioSide::Right, right, false),
            (AudioSide::Left, left, true),
        ] {
            let _ = complete_gatt(
                &mut machine,
                &write,
                GattResult::WriteComplete {
                    attribute_handle: 0x0043,
                    with_response: false,
                },
            );
            assert!(machine
                .on_hci_report(&HciReport::Gatt(
                    GattReport::Value(
                        GattValueEvent::Notification {
                            connection: write.connection,
                            attribute_handle: 0x0043,
                            value: release_cp_success_bytes(
                                &machine,
                                side,
                            ),
                        },
                    ),
                ))
                .unwrap()
                .is_empty());

            let actions = machine
                .on_hci_report(&HciReport::Gatt(
                    GattReport::Value(
                        GattValueEvent::Notification {
                            connection: write.connection,
                            attribute_handle: 0x0046,
                            value: vec![
                                machine.planned_sink(side).unwrap().ase_id,
                                AseState::RELEASING.0,
                            ],
                        },
                    ),
                ))
                .unwrap();

            assert_eq!(
                actions.iter().any(|action| matches!(
                    action,
                    ControlAction::RemoveIsoPathByRef { .. }
                )),
                should_start_transport_cleanup,
            );
        }
    }

    #[test]
    fn cleanup_ase_timeout_preserves_release_gate_diagnostics() {
        let mut machine = drive_to_streaming_ready();
        machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();
        machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Bound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();

        let _ = machine.begin_teardown().unwrap();
        let _ = machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Unbound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                    was_bound: true,
                },
            ))
            .unwrap();

        let error = machine
            .on_event(ControlEvent::TimerExpired(
                ControlTimer::CleanupAse {
                    side: AudioSide::Right,
                },
            ))
            .unwrap();

        assert!(error.iter().any(|action| matches!(
            action,
            ControlAction::CancelGatt { .. }
        )));
        assert!(matches!(
            machine.rollback.cleanup_failures.first(),
            Some(ControlCleanupFailure::AseReleaseGateTimeout {
                side: AudioSide::Right,
                gate: AseOperationGateSnapshot {
                    phase: AseOperationPhase::WaitingGates,
                    pending_request_id: Some(_),
                    write_complete: false,
                    cp_success: false,
                    notified_state: None,
                },
            })
        ));
    }

    #[test]
    fn normal_teardown_returns_idle_after_graceful_cleanup() {
        let mut machine = drive_to_streaming_ready();
        machine
            .begin_stream_handoff(SessionId(7), StreamId(11))
            .unwrap();
        machine
            .on_hci_report(&HciReport::StreamRouteState(
                StreamRouteState::Bound {
                    session_id: SessionId(7),
                    stream_id: StreamId(11),
                },
            ))
            .unwrap();

        let actions = machine.begin_teardown().unwrap();
        assert_eq!(machine.phase(), ControlPhase::RollingBack);
        assert_eq!(machine.failure(), None);
        assert!(matches!(
            actions.first(),
            Some(ControlAction::UnbindStreamRoute {
                session_id: SessionId(7),
                stream_id: StreamId(11),
            })
        ));

        machine.rollback.stream_route = None;
        machine.rollback.stream_route_unbind_pending = false;
        machine.rollback.iso_path_queue.clear();
        machine.rollback.iso_path_current = None;
        machine.rollback.cis_queue.clear();
        machine.rollback.cis_current = None;
        machine.rollback.ase_queue.clear();
        machine.rollback.ase_current = None;
        machine.rollback.ase_operation = None;
        machine.rollback.cig_pending = false;
        machine.rollback.acl_queue.clear();
        machine.rollback.acl_current = None;
        machine.cleanup_knowledge.cig_may_exist = false;
        machine.confirmed.privacy_active = false;

        assert_eq!(
            machine.finish_rollback().unwrap(),
            Vec::<ControlAction>::new()
        );
        assert_eq!(machine.phase(), ControlPhase::Idle);
        assert_eq!(
            machine.condition_update(),
            ControlConditionUpdate::initial()
        );
        assert!(machine.stream_binding().is_err());
    }


#[test]
fn final_model_codec_only_fences_before_any_cig_work() {
    let mut machine = machine_at_codec_configured();
    machine.review_profile = ReviewStreamProfile::CodecOnly;

    let actions = machine.continue_ready_stage().unwrap();
    assert!(actions.is_empty());
    assert_eq!(machine.phase(), ControlPhase::CodecConfigured);
    assert!(machine.planned.iso_plan().is_none());
    assert!(!machine.cleanup_knowledge.cig_may_exist);
}

#[test]
fn final_model_low_qos_changes_only_iso_reliability_budget() {
    let mut machine = machine_at_codec_configured();
    machine.review_profile = ReviewStreamProfile::DualCisLowQos;

    machine.plan_iso().unwrap();
    let plan = machine.planned.iso_plan().unwrap();
    assert_eq!(plan.sdu_interval_us, 10_000);
    assert_eq!(plan.octets_per_sdu, 100);
    assert_eq!(plan.phy, 0x02);
    assert_eq!(plan.rtn, 3);
    assert_eq!(plan.max_transport_latency_ms, 22);

    let cig = plan.cig_config();
    assert_eq!(cig.cis.len(), 2);
    assert!(cig.cis.iter().all(|cis| cis.rtn_c_to_p == 3));
    assert_eq!(cig.max_transport_latency_c_to_p, 22);
}

#[test]
fn final_model_single_cis_right_uses_only_right_after_codec() {
    let mut machine = machine_at_codec_configured();
    machine.review_profile = ReviewStreamProfile::SingleCisRight;

    machine.plan_iso().unwrap();
    let plan = machine.planned.iso_plan().unwrap().clone();
    let actions = machine.begin_cig_configuration().unwrap();
    let config = actions.iter().find_map(|action| {
        if let ControlAction::ConfigureCig(config) = action {
            Some(config.clone())
        } else {
            None
        }
    }).expect("single-CIS model must configure a CIG");
    assert_eq!(config.cis.len(), 1);
    assert_eq!(config.cis[0].cis_id, plan.right.cis_id);

    machine.on_hci_report(&HciReport::CigConfigureState(
        CigConfigureState::Ready(CigAllocation {
            cig_id: plan.cig_id,
            cis: vec![
                crate::plane::l1_hci::iso::cig_cis::CisAllocation {
                    cis_id: plan.right.cis_id,
                    connection_handle: 0x0901,
                },
            ],
        }),
    )).unwrap();
    assert_eq!(machine.phase(), ControlPhase::CigConfigured);

    machine.plan_qos().unwrap();
    let actions = machine.begin_qos_configuration().unwrap();
    assert_eq!(
        machine.phase(),
        ControlPhase::ConfiguringQos {
            side: AudioSide::Right,
        }
    );
    let _ = drive_config_qos(
        &mut machine,
        AudioSide::Right,
        actions,
    );
    assert_eq!(machine.phase(), ControlPhase::QosConfigured);

    machine.plan_enable().unwrap();
    let actions = machine.begin_enable().unwrap();
    assert_eq!(
        machine.phase(),
        ControlPhase::Enabling {
            side: AudioSide::Right,
        }
    );
    let _ = drive_enable(
        &mut machine,
        AudioSide::Right,
        actions,
    );
    assert_eq!(machine.phase(), ControlPhase::Enabled);

    let actions = machine.begin_cis_creation().unwrap();
    let request = actions.iter().find_map(|action| {
        if let ControlAction::CreateCis(request) = action {
            Some(request.clone())
        } else {
            None
        }
    }).expect("single-CIS model must create one CIS");
    assert_eq!(request.bindings.len(), 1);
    assert_eq!(request.bindings[0].cis_id, plan.right.cis_id);
    assert_eq!(request.bindings[0].acl, plan.right.acl);

    machine.on_hci_report(&HciReport::SemanticCisCreateState(
        SemanticCisCreateState::Ready {
            cig_id: plan.cig_id,
            established_cis_ids: vec![plan.right.cis_id],
        },
    )).unwrap();
    assert_eq!(machine.phase(), ControlPhase::CisEstablished);

    let actions = machine.begin_iso_path_setup().unwrap();
    let right = CisRef {
        cig_id: plan.cig_id,
        cis_id: plan.right.cis_id,
    };
    assert!(actions.iter().any(|action| {
        *action == ControlAction::SetupIsoInputPathByRef { cis: right }
    }));
    assert_eq!(
        machine.phase(),
        ControlPhase::SettingUpIsoPath {
            side: AudioSide::Right,
        }
    );

    machine.on_hci_report(&HciReport::SemanticIsoPathState(
        SemanticIsoPathState::SetupComplete {
            cis: right,
            direction: IsoDataPathDirection::Input,
        },
    )).unwrap();
    assert_eq!(machine.phase(), ControlPhase::IsoPathsReady);

    let _ = machine.begin_streaming_gate().unwrap();
    assert_eq!(
        machine.phase(),
        ControlPhase::AwaitingStreaming {
            side: AudioSide::Right,
        }
    );
}

#[test]
fn review_emergency_teardown_from_codec_skips_ase_release() {
    let mut machine = machine_at_codec_configured();
    machine.review_profile = ReviewStreamProfile::CodecOnly;

    let actions = machine.begin_emergency_teardown_for_review().unwrap();
    assert_eq!(machine.phase(), ControlPhase::RollingBack);
    assert!(!actions.iter().any(|action| {
        matches!(
            action,
            ControlAction::BeginGatt(_)
                | ControlAction::CancelGatt { .. }
                | ControlAction::ConfigureCig(_)
                | ControlAction::CreateCis(_)
        )
    }));
    assert!(actions.iter().any(|action| {
        matches!(action, ControlAction::DisconnectAcl { .. })
    }));
}
