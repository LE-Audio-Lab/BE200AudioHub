use crossbeam_channel::{
    bounded, Receiver, Sender, TryRecvError, TrySendError,
};

use crate::{
    runtime::mailbox::{RecvError, SendError},
    transport::{
        packet::ControllerPacket,
        sink::{ControllerPacketSink, PacketSinkError},
    },
};

use super::{
    connection::{
        DisconnectState, FalConnectRequest, FalConnectionState,
        LeConnectionSetSnapshot,
    },
    engine::{
        ControllerCapabilities, OperationalAttachState, PassiveScanState,
    },
    privacy::{ControllerPrivacyState, ResolvingListConfig},
    scan::ScanSummary,
    security::{StoredEncryptionRequest, StoredEncryptionState},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HciCommand {
    BeginOperationalAttach,
    StartPassiveExtendedScan,
    StopPassiveExtendedScan,
    ConfigureControllerPrivacy(ResolvingListConfig),
    BeginFalConnection(FalConnectRequest),
    CancelFalConnection,
    Disconnect { connection_handle: u16 },
    BeginStoredEncryption(StoredEncryptionRequest),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HciReport {
    AttachState(OperationalAttachState),
    CapabilitiesReady(ControllerCapabilities),
    PassiveScanState(PassiveScanState),
    ScanComplete(ScanSummary),
    ControllerPrivacyState(ControllerPrivacyState),
    FalConnectionState(FalConnectionState),
    DisconnectState(DisconnectState),
    StoredEncryptionState(StoredEncryptionState),
    ConnectionSet(LeConnectionSetSnapshot),
}

#[derive(Clone)]
pub struct HciControl {
    tx: Sender<HciCommand>,
    stop_tx: Sender<()>,
}

#[derive(Clone)]
pub struct HciIngress {
    tx: Sender<ControllerPacket>,
}

pub struct HciReportInbox {
    rx: Receiver<HciReport>,
}

pub(crate) struct HciWorkerPorts {
    pub control_rx: Receiver<HciCommand>,
    pub stop_rx: Receiver<()>,
    pub controller_rx: Receiver<ControllerPacket>,
    pub report_tx: Sender<HciReport>,
}

pub(crate) fn hci_mailbox(
    controller_capacity: usize,
    control_capacity: usize,
    report_capacity: usize,
) -> (
    HciControl,
    HciIngress,
    HciReportInbox,
    HciWorkerPorts,
) {
    assert!(controller_capacity > 0);
    assert!(control_capacity > 0);
    assert!(report_capacity > 0);

    let (control_tx, control_rx) = bounded(control_capacity);
    let (stop_tx, stop_rx) = bounded(1);
    let (controller_tx, controller_rx) = bounded(controller_capacity);
    let (report_tx, report_rx) = bounded(report_capacity);

    (
        HciControl {
            tx: control_tx,
            stop_tx,
        },
        HciIngress { tx: controller_tx },
        HciReportInbox { rx: report_rx },
        HciWorkerPorts {
            control_rx,
            stop_rx,
            controller_rx,
            report_tx,
        },
    )
}

impl HciControl {
    pub fn try_begin_operational_attach(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BeginOperationalAttach)
            .map_err(map_send_error)
    }

    pub fn try_start_passive_extended_scan(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::StartPassiveExtendedScan)
            .map_err(map_send_error)
    }

    pub fn try_stop_passive_extended_scan(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::StopPassiveExtendedScan)
            .map_err(map_send_error)
    }

    pub fn try_configure_controller_privacy(
        &self,
        config: ResolvingListConfig,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::ConfigureControllerPrivacy(config))
            .map_err(map_send_error)
    }

    pub fn try_begin_fal_connection(
        &self,
        request: FalConnectRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BeginFalConnection(request))
            .map_err(map_send_error)
    }

    pub fn try_cancel_fal_connection(&self) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::CancelFalConnection)
            .map_err(map_send_error)
    }

    pub fn try_disconnect(
        &self,
        connection_handle: u16,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::Disconnect { connection_handle })
            .map_err(map_send_error)
    }

    pub fn try_begin_stored_encryption(
        &self,
        request: StoredEncryptionRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(HciCommand::BeginStoredEncryption(request))
            .map_err(map_send_error)
    }

    /// Urgent lifecycle stop has a dedicated one-slot lane. A full stop lane
    /// means a stop is already pending and is therefore treated as success.
    pub fn request_stop(&self) -> Result<(), SendError> {
        match self.stop_tx.try_send(()) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err(SendError::Disconnected),
        }
    }
}

impl ControllerPacketSink for HciIngress {
    fn try_send_controller(
        &self,
        packet: ControllerPacket,
    ) -> Result<(), PacketSinkError> {
        self.tx.try_send(packet).map_err(|error| match error {
            TrySendError::Full(_) => PacketSinkError::Full,
            TrySendError::Disconnected(_) => PacketSinkError::Disconnected,
        })
    }
}

impl HciReportInbox {
    pub fn try_recv(&self) -> Result<HciReport, RecvError> {
        self.rx.try_recv().map_err(|error| match error {
            TryRecvError::Empty => RecvError::Empty,
            TryRecvError::Disconnected => RecvError::Disconnected,
        })
    }
}

fn map_send_error<T>(error: TrySendError<T>) -> SendError {
    match error {
        TrySendError::Full(_) => SendError::Full,
        TrySendError::Disconnected(_) => SendError::Disconnected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_lane_remains_available_when_controller_lane_is_full() {
        let (control, ingress, _reports, _ports) = hci_mailbox(1, 1, 1);

        ingress
            .try_send_controller(ControllerPacket::Event(vec![1]))
            .unwrap();
        assert_eq!(
            ingress.try_send_controller(ControllerPacket::Event(vec![2])),
            Err(PacketSinkError::Full)
        );

        assert_eq!(control.request_stop(), Ok(()));
        assert_eq!(control.request_stop(), Ok(()));
    }

    #[test]
    fn stored_encryption_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        let request = StoredEncryptionRequest {
            connection_handle: 0x0800,
            key: super::super::security::StoredLeKeyMaterial::
                secure_connections([0x55; 16]),
        };
        control
            .try_begin_stored_encryption(request)
            .unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::BeginStoredEncryption(request)
        );
    }

    #[test]
    fn disconnect_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        control.try_disconnect(0x0800).unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::Disconnect {
                connection_handle: 0x0800,
            }
        );
    }

    #[test]
    fn fal_cancel_uses_bounded_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox(1, 2, 1);

        control.try_cancel_fal_connection().unwrap();
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::CancelFalConnection
        );
    }

    #[test]
    fn control_and_controller_backpressure_are_independent() {
        let (control, ingress, _reports, _ports) = hci_mailbox(1, 1, 1);

        control.try_begin_operational_attach().unwrap();
        assert_eq!(
            control.try_begin_operational_attach(),
            Err(SendError::Full)
        );

        ingress
            .try_send_controller(ControllerPacket::Event(vec![1]))
            .unwrap();
        assert_eq!(
            ingress.try_send_controller(ControllerPacket::Event(vec![2])),
            Err(PacketSinkError::Full)
        );
    }
}
