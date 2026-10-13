use std::time::Duration;

use bumble_gatt::GattClient;
use bumble_transport::{Error, ExternalAttTransport, Result as TransportResult};

use crate::AttachedController;

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
