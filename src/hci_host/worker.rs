use std::thread::{self, JoinHandle};

use crossbeam_channel::{select_biased, TryRecvError, TrySendError};

use crate::transport::{
    packet::ControllerPacket,
    worker::TransportTx,
};

use super::{
    engine::{EngineFault, HciEngine, OperationalAttachState},
    mailbox::{HciCommand, HciReport, HciWorkerPorts},
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
                    if let Err(fault) = publish_attach_transition(
                        &engine,
                        &mut last_attach_state,
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
            if let Err(fault) = publish_attach_transition(
                &engine,
                &mut last_attach_state,
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
                if let Err(fault) = publish_attach_transition(
                    &engine,
                    &mut last_attach_state,
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
                if let Err(fault) = publish_attach_transition(
                    &engine,
                    &mut last_attach_state,
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
    }
}

fn publish_attach_transition(
    engine: &HciEngine,
    last_state: &mut OperationalAttachState,
    report_tx: &crossbeam_channel::Sender<HciReport>,
) -> Result<(), HciWorkerFault> {
    let state = engine.attach_state();
    if state == *last_state {
        return Ok(());
    }

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
        hci_host::mailbox::hci_mailbox,
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
