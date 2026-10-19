use crate::{
    plane::l1_hci::interface::{
        command::HciControl,
        iso_tx::{HciIsoTx, HciLogicalIsoTx},
        mailbox::hci_mailbox_with_iso,
        report::HciReportInbox,
    },
    platform::backend::{BackendError, DeviceBackend},
    plane::l2_transport::worker::{
        spawn_transport_worker_with_sink, TransportTx,
        TransportWorkerExit, TransportWorkerJoin,
    },
};

use super::worker::{spawn_hci_worker, HciWorkerExit, HciWorkerJoin};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HciStackConfig {
    pub transport_tx_capacity: usize,
    pub controller_inbox_capacity: usize,
    pub control_inbox_capacity: usize,
    pub iso_data_inbox_capacity: usize,
    pub report_inbox_capacity: usize,
    pub sanitize_rx_flow_control_on_attach: bool,
}

impl Default for HciStackConfig {
    fn default() -> Self {
        Self {
            transport_tx_capacity: 256,
            controller_inbox_capacity: 256,
            control_inbox_capacity: 32,
            iso_data_inbox_capacity: 4,
            report_inbox_capacity: 64,
            sanitize_rx_flow_control_on_attach: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HciStackExit {
    pub hci: HciWorkerExit,
    pub transport: TransportWorkerExit,
}

pub struct HciStack {
    control: HciControl,
    reports: HciReportInbox,
    transport_tx: TransportTx,
    hci_join: HciWorkerJoin,
    transport_join: TransportWorkerJoin,
}

impl HciStack {
    pub fn control(&self) -> &HciControl {
        &self.control
    }

    pub fn reports(&self) -> &HciReportInbox {
        &self.reports
    }

    /// Dedicated realtime data-plane handle. It cannot issue HCI control
    /// commands and never blocks the producer on controller credit.
    pub fn iso_tx(&self) -> HciIsoTx {
        self.control.iso_tx()
    }

    /// Product realtime data-plane handle. Payloads are addressed only by
    /// SessionId/StreamId and never expose CIS/HCI connection handles.
    pub fn logical_iso_tx(&self) -> HciLogicalIsoTx {
        self.control.logical_iso_tx()
    }

    /// Ordered shutdown: stop HCI semantic production first, then cancel raw
    /// transport I/O. This prevents Device from generating new controller work
    /// while the USB backend is being torn down.
    pub fn shutdown(self) -> HciStackExit {
        let _ = self.control.request_stop();
        let hci = self.hci_join.join();

        self.transport_tx.cancel();
        let transport = self.transport_join.join();

        HciStackExit { hci, transport }
    }
}

pub fn spawn_hci_stack_with_backend<B, F>(
    factory: F,
    config: HciStackConfig,
) -> HciStack
where
    B: DeviceBackend + 'static,
    F: FnOnce() -> Result<B, BackendError> + Send + 'static,
{
    let (control, ingress, reports, ports) = hci_mailbox_with_iso(
        config.controller_inbox_capacity,
        config.control_inbox_capacity,
        config.report_inbox_capacity,
        config.iso_data_inbox_capacity,
    );

    let (transport_tx, transport_join) = spawn_transport_worker_with_sink(
        factory,
        config.transport_tx_capacity,
        ingress,
    );

    let hci_join = spawn_hci_worker(
        ports,
        transport_tx.clone(),
        config.sanitize_rx_flow_control_on_attach,
    );

    HciStack {
        control,
        reports,
        transport_tx,
        hci_join,
        transport_join,
    }
}

#[cfg(windows)]
pub fn spawn_be200_hci_stack(config: HciStackConfig) -> HciStack {
    spawn_hci_stack_with_backend(
        crate::platform::windows::be200::open_backend,
        config,
    )
}

#[cfg(test)]
mod tests {
    use crate::platform::backend::test::TestBackend;

    use super::*;

    #[test]
    fn stack_starts_and_stops_workers_in_order() {
        let stack = spawn_hci_stack_with_backend(
            || Ok(TestBackend::default()),
            HciStackConfig {
                transport_tx_capacity: 8,
                controller_inbox_capacity: 8,
                control_inbox_capacity: 4,
                iso_data_inbox_capacity: 2,
                report_inbox_capacity: 4,
                sanitize_rx_flow_control_on_attach: false,
            },
        );

        let exit = stack.shutdown();
        assert_eq!(exit.hci, HciWorkerExit::Stopped);
        assert_eq!(exit.transport, TransportWorkerExit::Cancelled);
    }

    #[test]
    fn default_capacities_are_all_bounded_and_nonzero() {
        let config = HciStackConfig::default();
        assert!(config.transport_tx_capacity > 0);
        assert!(config.controller_inbox_capacity > 0);
        assert!(config.control_inbox_capacity > 0);
        assert!(config.iso_data_inbox_capacity > 0);
        assert!(config.report_inbox_capacity > 0);
        assert!(!config.sanitize_rx_flow_control_on_attach);
    }
}
