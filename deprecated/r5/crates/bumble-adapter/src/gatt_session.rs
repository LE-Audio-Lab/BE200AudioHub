use std::time::{Duration, Instant};

use bumble_att::AttPdu;
use bumble_gatt::GattClient;
use bumble_transport::{
    Error, ExternalAttTransport, ExternalHostActivity, Result as TransportResult,
};

use crate::AttachedController;

const EVENT_WAIT_SLICE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttValueEvent {
    pub connection_handle: u16,
    pub attribute_handle: u16,
    pub value: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeAclState {
    pub connected: bool,
    pub encrypted: bool,
}

impl AttachedController {
    pub fn le_acl_state(&self, connection_handle: u16) -> LeAclState {
        LeAclState {
            connected: self.device.is_connected_on_handle(connection_handle),
            encrypted: self.device.is_encrypted_on_handle(connection_handle),
        }
    }

    /// Run one synchronous GATT operation on an already connected and encrypted
    /// ACL without exposing the adapter's Host/Device ownership boundary.
    pub fn with_encrypted_gatt<T, F>(
        &mut self,
        connection_handle: u16,
        timeout: Duration,
        operation: F,
    ) -> TransportResult<T>
    where
        F: for<'a> FnOnce(
            &mut GattClient,
            &mut ExternalAttTransport<'a>,
        ) -> TransportResult<T>,
    {
        if !self.device.is_connected_on_handle(connection_handle) {
            return Err(Error::Remote(format!(
                "cannot open GATT session: LE ACL 0x{connection_handle:04X} is not connected"
            )));
        }
        if !self.device.is_encrypted_on_handle(connection_handle) {
            return Err(Error::Remote(format!(
                "cannot open GATT session: LE ACL 0x{connection_handle:04X} is not encrypted"
            )));
        }

        let mut transport = ExternalAttTransport::new(
            &mut self.host,
            &mut self.device,
            connection_handle,
            timeout,
        )?;
        let mut client = GattClient::new();
        operation(&mut client, &mut transport)
    }
}

impl AttachedController {
    /// Preserve notifications that arrive *during* a synchronous ATT request.
    /// ExternalAttTransport routes unsolicited PDUs into its own local queue;
    /// they would otherwise be lost when that temporary transport is dropped.
    pub fn with_encrypted_gatt_events<T, F>(
        &mut self,
        connection_handle: u16,
        timeout: Duration,
        operation: F,
    ) -> TransportResult<(T, Vec<AttValueEvent>)>
    where
        F: for<'a> FnOnce(
            &mut GattClient,
            &mut ExternalAttTransport<'a>,
        ) -> TransportResult<T>,
    {
        let (value, buffered) = self.with_encrypted_gatt(
            connection_handle,
            timeout,
            |client, transport| {
                let value = operation(client, transport)?;
                Ok((value, transport.take_unsolicited()))
            },
        )?;
        // The borrowed ATT transport has been dropped: indications can now be
        // confirmed through Device without aliasing Host/Device mutably.
        let events = self.process_att_pdus(connection_handle, buffered)?;
        Ok((value, events))
    }

    /// Nonblocking drain of already-received ATT notifications across ACLs.
    /// Never call this while an ATT request is in flight on those handles.
    pub fn drain_att_value_events(
        &mut self,
        handles: &[u16],
    ) -> TransportResult<Vec<AttValueEvent>> {
        self.device.poll(&mut self.host);
        let mut events = Vec::new();
        for &handle in handles {
            let pending = self.device.take_inbox_on_handle(handle);
            events.extend(self.process_att_pdus(handle, pending)?);
        }
        Ok(events)
    }

    /// Pump HCI -> Device -> ATT, preserving the link/handle association.
    /// An indication is explicitly acknowledged; unsolicited ATT responses
    /// outside an active request are treated as protocol errors, not discarded.
    pub fn wait_for_att_value_events(
        &mut self,
        handles: &[u16],
        timeout: Duration,
    ) -> TransportResult<Vec<AttValueEvent>> {
        if handles.is_empty() || timeout.is_zero() {
            return Err(Error::InvalidSpec(
                "ATT event wait requires at least one handle and nonzero timeout".into(),
            ));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let events = self.drain_att_value_events(handles)?;
            if !events.is_empty() {
                return Ok(events);
            }
            for &handle in handles {
                if !self.device.is_connected_on_handle(handle) {
                    return Err(Error::Remote(format!(
                        "LE ACL 0x{handle:04X} disconnected while awaiting ATT value notification"
                    )));
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Remote(format!(
                    "timed out awaiting ATT value notification on handles {handles:?}"
                )));
            }
            match self.host.wait_for_device_activity(
                &mut self.device,
                remaining.min(EVENT_WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet | ExternalHostActivity::Timeout => {}
                ExternalHostActivity::Ended => {
                    return Err(Error::Remote(
                        "HCI transport ended while waiting for ATT events".into(),
                    ));
                }
            }
        }
    }

    fn process_att_pdus(
        &mut self,
        handle: u16,
        pdus: Vec<AttPdu>,
    ) -> TransportResult<Vec<AttValueEvent>> {
        let mut events = Vec::new();
        for pdu in pdus {
            match pdu {
                AttPdu::HandleValueNotification {
                    attribute_handle,
                    attribute_value,
                } => events.push(AttValueEvent {
                    connection_handle: handle,
                    attribute_handle,
                    value: attribute_value,
                }),
                AttPdu::HandleValueIndication {
                    attribute_handle,
                    attribute_value,
                } => {
                    if !self.device.send_att_on_handle(
                        &mut self.host,
                        handle,
                        &AttPdu::HandleValueConfirmation,
                    ) {
                        return Err(Error::Remote(format!(
                            "failed to confirm ATT indication on LE ACL 0x{handle:04X}"
                        )));
                    }
                    events.push(AttValueEvent {
                        connection_handle: handle,
                        attribute_handle,
                        value: attribute_value,
                    });
                }
                other => {
                    return Err(Error::Remote(format!(
                        "unexpected unconsumed ATT PDU on LE ACL 0x{handle:04X}: {other:?}"
                    )));
                }
            }
        }
        Ok(events)
    }
}
