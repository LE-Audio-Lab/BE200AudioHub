use std::thread::{self, JoinHandle};

use crossbeam_channel::{select_biased, TryRecvError, TrySendError};

use crate::transport::worker::TransportTx;

#[cfg(test)]
use crate::transport::packet::ControllerPacket;

use super::{
    engine::{
        EngineFault, HciEngine, OperationalAttachState, PassiveScanState,
    },
    connection::{
        DisconnectState, FalConnectionState, LeConnectionSetSnapshot,
    },
    mailbox::{HciCommand, HciReport, HciWorkerPorts},
    privacy::ControllerPrivacyState,
    security::StoredEncryptionState,
};

const CONTROL_BUDGET_PER_TURN: usize = 4;
const CONTROLLER_BUDGET_PER_TURN: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciWorkerFault {
    Engine(EngineFault),
    ReportFull,
    ReportDisconnected,
    ControlDisconnected,
    ControllerDisconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HciWorkerExit {
    Stopped,
    Faulted(HciWorkerFault),
}

pub struct HciWorkerJoin {
    join: JoinHandle<HciWorkerExit>,
}

impl HciWorkerJoin {
    pub fn join(self) -> HciWorkerExit {
        self.join.join().unwrap_or(HciWorkerExit::Faulted(
            HciWorkerFault::Engine(EngineFault::PollBudgetExceeded),
        ))
    }
}

pub(crate) fn spawn_hci_worker(
    ports: HciWorkerPorts,
    transport_tx: TransportTx,
) -> HciWorkerJoin {
    let join = thread::Builder::new()
        .name("HciHost".into())
        .spawn(move || run_loop(ports, transport_tx))
        .expect("failed to spawn HCI host worker");

    HciWorkerJoin { join }
}

fn run_loop(
    ports: HciWorkerPorts,
    transport_tx: TransportTx,
) -> HciWorkerExit {
    let HciWorkerPorts {
        control_rx,
        stop_rx,
        controller_rx,
        report_tx,
    } = ports;

    let mut engine = HciEngine::new(transport_tx);
    let mut last_attach_state = engine.attach_state();
    let mut last_scan_state = engine.passive_scan_state();
    let mut last_privacy_state = engine.controller_privacy_state().clone();
    let mut last_fal_connection_state =
        engine.fal_connection_state().clone();
    let mut last_disconnect_state = engine.disconnect_state().clone();
    let mut last_stored_encryption_state =
        engine.stored_encryption_state().clone();
    let mut last_connection_set = engine.connection_set_snapshot();

    loop {
        match stop_rx.try_recv() {
            Ok(()) => return HciWorkerExit::Stopped,
            Err(TryRecvError::Disconnected) => return HciWorkerExit::Stopped,
            Err(TryRecvError::Empty) => {}
        }

        let mut did_work = false;

        for _ in 0..CONTROL_BUDGET_PER_TURN {
            match control_rx.try_recv() {
                Ok(command) => {
                    did_work = true;
                    if let Err(fault) = process_command(&mut engine, command) {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::Engine(fault),
                        );
                    }
                    if let Err(fault) = publish_state_transitions(
                        &mut engine,
                        &mut last_attach_state,
                        &mut last_scan_state,
                        &mut last_privacy_state,
                        &mut last_fal_connection_state,
                        &mut last_disconnect_state,
                        &mut last_stored_encryption_state,
                        &mut last_connection_set,
                        &report_tx,
                    ) {
                        return HciWorkerExit::Faulted(fault);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::ControlDisconnected,
                    )
                }
            }
        }

        let mut controller_packets = 0usize;
        for _ in 0..CONTROLLER_BUDGET_PER_TURN {
            match controller_rx.try_recv() {
                Ok(packet) => {
                    did_work = true;
                    controller_packets += 1;
                    if let Err(fault) = engine.ingest(packet) {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::Engine(fault),
                        );
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::ControllerDisconnected,
                    )
                }
            }
        }

        if controller_packets != 0 {
            if let Err(fault) = engine.poll_until_quiescent() {
                return HciWorkerExit::Faulted(
                    HciWorkerFault::Engine(fault),
                );
            }
            if let Err(fault) = publish_state_transitions(
                &mut engine,
                &mut last_attach_state,
                &mut last_scan_state,
                &mut last_privacy_state,
                &mut last_fal_connection_state,
                &mut last_disconnect_state,
                &mut last_stored_encryption_state,
                &mut last_connection_set,
                &report_tx,
            ) {
                return HciWorkerExit::Faulted(fault);
            }
        }

        if did_work {
            continue;
        }

        // Idle wait only. No HCI transaction waits here: every command/event
        // handler remains run-to-completion and returns to this selector.
        select_biased! {
            recv(stop_rx) -> _ => return HciWorkerExit::Stopped,

            recv(controller_rx) -> message => {
                let packet = match message {
                    Ok(packet) => packet,
                    Err(_) => {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::ControllerDisconnected,
                        )
                    }
                };
                if let Err(fault) = engine.ingest(packet) {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::Engine(fault),
                    );
                }
                if let Err(fault) = engine.poll_until_quiescent() {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::Engine(fault),
                    );
                }
                if let Err(fault) = publish_state_transitions(
                    &mut engine,
                    &mut last_attach_state,
                    &mut last_scan_state,
                    &mut last_privacy_state,
                    &mut last_fal_connection_state,
                    &mut last_disconnect_state,
                    &mut last_stored_encryption_state,
                    &mut last_connection_set,
                    &report_tx,
                ) {
                    return HciWorkerExit::Faulted(fault);
                }
            }

            recv(control_rx) -> message => {
                let command = match message {
                    Ok(command) => command,
                    Err(_) => {
                        return HciWorkerExit::Faulted(
                            HciWorkerFault::ControlDisconnected,
                        )
                    }
                };
                if let Err(fault) = process_command(&mut engine, command) {
                    return HciWorkerExit::Faulted(
                        HciWorkerFault::Engine(fault),
                    );
                }
                if let Err(fault) = publish_state_transitions(
                    &mut engine,
                    &mut last_attach_state,
                    &mut last_scan_state,
                    &mut last_privacy_state,
                    &mut last_fal_connection_state,
                    &mut last_disconnect_state,
                    &mut last_stored_encryption_state,
                    &mut last_connection_set,
                    &report_tx,
                ) {
                    return HciWorkerExit::Faulted(fault);
                }
            }
        }
    }
}

fn process_command(
    engine: &mut HciEngine,
    command: HciCommand,
) -> Result<(), EngineFault> {
    match command {
        HciCommand::BeginOperationalAttach => {
            engine.begin_operational_attach()
        }
        HciCommand::StartPassiveExtendedScan => {
            engine.begin_passive_extended_scan()
        }
        HciCommand::StopPassiveExtendedScan => {
            engine.stop_passive_extended_scan()
        }
        HciCommand::ConfigureControllerPrivacy(config) => {
            engine.begin_controller_privacy(config)
        }
        HciCommand::ClearControllerPrivacy => {
            engine.begin_controller_privacy_clear()
        }
        HciCommand::BeginFalConnection(request) => {
            engine.begin_fal_connection(request)
        }
        HciCommand::CancelFalConnection => {
            engine.cancel_fal_connection()
        }
        HciCommand::Disconnect { connection_handle } => {
            engine.begin_disconnect(connection_handle)
        }
        HciCommand::BeginStoredEncryption(request) => {
            engine.begin_stored_encryption(request)
        }
    }
}

fn publish_state_transitions(
    engine: &mut HciEngine,
    last_state: &mut OperationalAttachState,
    last_scan_state: &mut PassiveScanState,
    last_privacy_state: &mut ControllerPrivacyState,
    last_fal_connection_state: &mut FalConnectionState,
    last_disconnect_state: &mut DisconnectState,
    last_stored_encryption_state: &mut StoredEncryptionState,
    last_connection_set: &mut LeConnectionSetSnapshot,
    report_tx: &crossbeam_channel::Sender<HciReport>,
) -> Result<(), HciWorkerFault> {
    let state = engine.attach_state();
    if state != *last_state {
        report_tx
            .try_send(HciReport::AttachState(state))
            .map_err(map_report_error)?;
        *last_state = state;

        if state == OperationalAttachState::Ready {
            if let Some(capabilities) = engine.capabilities() {
                report_tx
                    .try_send(HciReport::CapabilitiesReady(capabilities))
                    .map_err(map_report_error)?;
            }
        }
    }

    let scan_state = engine.passive_scan_state();
    if scan_state != *last_scan_state {
        report_tx
            .try_send(HciReport::PassiveScanState(scan_state))
            .map_err(map_report_error)?;
        *last_scan_state = scan_state;
    }

    if let Some(summary) = engine.take_completed_scan() {
        report_tx
            .try_send(HciReport::ScanComplete(summary))
            .map_err(map_report_error)?;
    }

    let privacy_state = engine.controller_privacy_state().clone();
    if privacy_state != *last_privacy_state {
        report_tx
            .try_send(HciReport::ControllerPrivacyState(
                privacy_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_privacy_state = privacy_state;
    }

    let fal_connection_state = engine.fal_connection_state().clone();
    if fal_connection_state != *last_fal_connection_state {
        report_tx
            .try_send(HciReport::FalConnectionState(
                fal_connection_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_fal_connection_state = fal_connection_state;
    }

    let disconnect_state = engine.disconnect_state().clone();
    if disconnect_state != *last_disconnect_state {
        report_tx
            .try_send(HciReport::DisconnectState(
                disconnect_state.clone(),
            ))
            .map_err(map_report_error)?;
        *last_disconnect_state = disconnect_state;
    }

    let stored_encryption_state =
        *engine.stored_encryption_state();
    if stored_encryption_state != *last_stored_encryption_state {
        report_tx
            .try_send(HciReport::StoredEncryptionState(
                stored_encryption_state,
            ))
            .map_err(map_report_error)?;
        *last_stored_encryption_state = stored_encryption_state;
    }

    let connection_set = engine.connection_set_snapshot();
    if connection_set != *last_connection_set {
        report_tx
            .try_send(HciReport::ConnectionSet(
                connection_set.clone(),
            ))
            .map_err(map_report_error)?;
        *last_connection_set = connection_set;
    }

    Ok(())
}

fn map_report_error(
    error: TrySendError<HciReport>,
) -> HciWorkerFault {
    match error {
        TrySendError::Full(_) => HciWorkerFault::ReportFull,
        TrySendError::Disconnected(_) => HciWorkerFault::ReportDisconnected,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::{
        hci_host::{
            connection::{
                FalConnectRequest, LeInitiationParameters,
                LePeerIdentity,
            },
            mailbox::hci_mailbox,
            privacy::{
                ResolvingListConfig, ResolvingPeer,
                DEFAULT_RPA_TIMEOUT_SECONDS,
            },
        },
        runtime::mailbox::RecvError,
        transport::{
            packet::HostPacket,
            sink::ControllerPacketSink,
        },
    };

    use super::*;

    fn recv_host_packet(
        rx: &crate::runtime::mailbox::BoundedReceiver<HostPacket>,
    ) -> HostPacket {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match rx.try_recv() {
                Ok(packet) => return packet,
                Err(RecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for host packet"
                    );
                    thread::yield_now();
                }
                Err(RecvError::Disconnected) => {
                    panic!("transport test channel disconnected")
                }
            }
        }
    }

    fn recv_report(
        reports: &super::super::mailbox::HciReportInbox,
    ) -> HciReport {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match reports.try_recv() {
                Ok(report) => return report,
                Err(RecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for HCI report"
                    );
                    thread::yield_now();
                }
                Err(RecvError::Disconnected) => {
                    panic!("HCI report channel disconnected")
                }
            }
        }
    }

    #[test]
    fn worker_owns_device_and_completes_minimal_resetless_attach() {
        let (control, ingress, reports, ports) = hci_mailbox(16, 4, 8);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(16);
        let join = spawn_hci_worker(ports, transport_tx);

        control.try_begin_operational_attach().unwrap();

        assert_eq!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(vec![0x02, 0x10, 0x00])
        );
        assert_eq!(
            recv_report(&reports),
            HciReport::AttachState(
                OperationalAttachState::Discovering
            )
        );

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();

        // Minimal all-zero command bitmap causes only mandatory event masks.
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();

        assert_eq!(
            recv_report(&reports),
            HciReport::AttachState(OperationalAttachState::Ready)
        );
        assert!(matches!(
            recv_report(&reports),
            HciReport::CapabilitiesReady(_)
        ));

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn worker_routes_controller_privacy_as_typed_transaction() {
        let (control, ingress, reports, ports) = hci_mailbox(32, 8, 16);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(32);
        let join = spawn_hci_worker(ports, transport_tx);

        // Minimal resetless attach to Ready.
        control.try_begin_operational_attach().unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_report(&reports);

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_host_packet(&transport_rx);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();
        let _ = recv_report(&reports);
        let _ = recv_report(&reports);

        control
            .try_configure_controller_privacy(ResolvingListConfig {
                peers: vec![ResolvingPeer {
                    identity_address_le: [
                        0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8,
                    ],
                    identity_address_type: 0,
                    peer_irk: [0x11; 16],
                }],
                local_irk: [0x22; 16],
                rpa_timeout_seconds: DEFAULT_RPA_TIMEOUT_SECONDS,
            })
            .unwrap();

        let opcode = |packet: HostPacket| {
            let HostPacket::Command(bytes) = packet else {
                panic!("expected HCI command");
            };
            u16::from_le_bytes([bytes[0], bytes[1]])
        };

        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x202d);
        assert_eq!(
            recv_report(&reports),
            HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Configuring,
            )
        );

        for (current, extra, next) in [
            (0x202d, &[][..], 0x2029),
            (0x2029, &[][..], 0x202a),
            (0x202a, &[1][..], 0x2027),
            (0x2027, &[][..], 0x202e),
            (0x202e, &[][..], 0x202d),
        ] {
            let mut event = vec![
                0x0e,
                (4 + extra.len()) as u8,
                0x01,
                current as u8,
                (current >> 8) as u8,
                0x00,
            ];
            event.extend_from_slice(extra);
            ingress
                .try_send_controller(ControllerPacket::Event(event))
                .unwrap();
            assert_eq!(opcode(recv_host_packet(&transport_rx)), next);
        }

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x2d, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(
            recv_report(&reports),
            HciReport::ControllerPrivacyState(
                ControllerPrivacyState::Ready,
            )
        );

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn worker_routes_fal_connection_as_typed_transaction() {
        let (control, ingress, reports, ports) = hci_mailbox(48, 8, 24);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(48);
        let join = spawn_hci_worker(ports, transport_tx);

        // Minimal resetless attach to Ready.
        control.try_begin_operational_attach().unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_report(&reports);

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_host_packet(&transport_rx);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();
        let _ = recv_report(&reports);
        let _ = recv_report(&reports);

        control
            .try_begin_fal_connection(FalConnectRequest {
                peer: LePeerIdentity {
                    address_le: [
                        0x23, 0x2f, 0xef, 0x13, 0xc9, 0xe8,
                    ],
                    address_type: 0,
                },
                parameters: LeInitiationParameters::default(),
            })
            .unwrap();

        let opcode = |packet: HostPacket| {
            let HostPacket::Command(bytes) = packet else {
                panic!("expected HCI command");
            };
            u16::from_le_bytes([bytes[0], bytes[1]])
        };

        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x200f);
        assert_eq!(
            recv_report(&reports),
            HciReport::FalConnectionState(
                FalConnectionState::Preparing,
            )
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x05, 0x01, 0x0f, 0x20, 0x00, 0x08,
            ]))
            .unwrap();
        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x2010);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x10, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x2011);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x11, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x2043);
        assert_eq!(
            recv_report(&reports),
            HciReport::FalConnectionState(
                FalConnectionState::Initiating,
            )
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0f, 0x04, 0x00, 0x01, 0x43, 0x20,
            ]))
            .unwrap();
        assert_eq!(
            recv_report(&reports),
            HciReport::FalConnectionState(
                FalConnectionState::AwaitingConnection,
            )
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x3e, 0x13, 0x01, 0x00,
                0x00, 0x08,
                0x00,
                0x01,
                1, 2, 3, 4, 5, 6,
                0x18, 0x00,
                0x00, 0x00,
                0x2a, 0x00,
                0x00,
            ]))
            .unwrap();
        assert_eq!(opcode(recv_host_packet(&transport_rx)), 0x2010);
        assert_eq!(
            recv_report(&reports),
            HciReport::FalConnectionState(
                FalConnectionState::Cleaning,
            )
        );
        match recv_report(&reports) {
            HciReport::ConnectionSet(snapshot) => {
                assert_eq!(snapshot.connections.len(), 1);
                assert_eq!(
                    snapshot.connections[0].connection_handle,
                    0x0800
                );
            }
            other => {
                panic!("expected connection-set report, got {other:?}")
            }
        }

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x10, 0x20, 0x00,
            ]))
            .unwrap();

        match recv_report(&reports) {
            HciReport::FalConnectionState(
                FalConnectionState::Connected(connection),
            ) => {
                assert_eq!(connection.connection_handle, 0x0800);
                assert_eq!(connection.connection_interval, 24);
            }
            other => panic!("expected connected report, got {other:?}"),
        }

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn worker_routes_passive_scan_as_typed_transaction() {
        let (control, ingress, reports, ports) = hci_mailbox(32, 8, 16);
        let (transport_tx, transport_rx) =
            TransportTx::test_channel(32);
        let join = spawn_hci_worker(ports, transport_tx);

        // Bring the synthetic controller to the minimal resetless Ready state.
        control.try_begin_operational_attach().unwrap();
        assert_eq!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(vec![0x02, 0x10, 0x00])
        );
        assert!(matches!(
            recv_report(&reports),
            HciReport::AttachState(OperationalAttachState::Discovering)
        ));

        let mut supported =
            vec![0x0e, 0x44, 0x01, 0x02, 0x10, 0x00];
        supported.resize(2 + 0x44, 0);
        ingress
            .try_send_controller(ControllerPacket::Event(supported))
            .unwrap();
        let _ = recv_host_packet(&transport_rx);
        let _ = recv_host_packet(&transport_rx);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x0c, 0x00,
            ]))
            .unwrap();
        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x01, 0x20, 0x00,
            ]))
            .unwrap();

        assert!(matches!(
            recv_report(&reports),
            HciReport::AttachState(OperationalAttachState::Ready)
        ));
        assert!(matches!(
            recv_report(&reports),
            HciReport::CapabilitiesReady(_)
        ));

        control.try_start_passive_extended_scan().unwrap();
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));
        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Configuring)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x41, 0x20, 0x00,
            ]))
            .unwrap();
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));
        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Enabling)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x42, 0x20, 0x00,
            ]))
            .unwrap();
        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Scanning)
        );

        control.try_stop_passive_extended_scan().unwrap();
        assert!(matches!(
            recv_host_packet(&transport_rx),
            HostPacket::Command(_)
        ));
        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Disabling)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![
                0x0e, 0x04, 0x01, 0x42, 0x20, 0x00,
            ]))
            .unwrap();

        assert_eq!(
            recv_report(&reports),
            HciReport::PassiveScanState(PassiveScanState::Idle)
        );
        match recv_report(&reports) {
            HciReport::ScanComplete(summary) => {
                assert_eq!(summary.reports_seen, 0);
                assert!(summary.observations.is_empty());
                assert!(!summary.truncated);
            }
            other => panic!("unexpected report: {other:?}"),
        }

        control.request_stop().unwrap();
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }

    #[test]
    fn urgent_stop_is_not_blocked_by_full_controller_lane() {
        let (control, ingress, _reports, ports) =
            hci_mailbox(1, 1, 4);
        let (transport_tx, _transport_rx) =
            TransportTx::test_channel(4);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![1]))
            .unwrap();
        control.request_stop().unwrap();

        let join = spawn_hci_worker(ports, transport_tx);
        assert_eq!(join.join(), HciWorkerExit::Stopped);
    }
}
