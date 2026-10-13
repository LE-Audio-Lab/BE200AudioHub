use std::time::{Duration, Instant};

use bumble_host::DeviceEvent;
use bumble_transport::{Error, ExternalHostActivity, Result as TransportResult};

use crate::AttachedController;

const WAIT_SLICE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeAclReport {
    pub connection_handle: u16,
    pub role: u8,
    pub peer_address: String,
    pub local_address: String,
    pub local_address_type: u8,
    pub connection_interval: u16,
    pub peripheral_latency: u16,
    pub supervision_timeout: u16,
}

impl AttachedController {
    /// Keep one established ACL alive while continuing to pump controller events.
    pub fn linger_le_connection(
        &mut self,
        connection_handle: u16,
        duration: Duration,
    ) -> TransportResult<()> {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.device.poll(&mut self.host);
            if !self.device.is_connected_on_handle(connection_handle) {
                return Err(Error::Remote(format!(
                    "LE ACL 0x{connection_handle:04X} disconnected during linger"
                )));
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.host.wait_for_device_activity(
                &mut self.device,
                remaining.min(WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet | ExternalHostActivity::Timeout => {}
                ExternalHostActivity::Ended => {
                    return Err(Error::Remote(
                        "HCI transport ended during ACL linger".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Submit HCI Disconnect and wait for the matching completion event.
    pub fn disconnect_le_connection(
        &mut self,
        connection_handle: u16,
        reason: u8,
        timeout: Duration,
    ) -> TransportResult<u8> {
        if !self.device.is_connected_on_handle(connection_handle) {
            return Err(Error::InvalidSpec(format!(
                "LE ACL 0x{connection_handle:04X} is not connected"
            )));
        }

        self.device.take_device_events();
        if !self
            .device
            .disconnect_handle(&mut self.host, connection_handle, reason)
        {
            return Err(Error::Remote(format!(
                "failed to submit HCI Disconnect for handle 0x{connection_handle:04X}"
            )));
        }

        let deadline = Instant::now() + timeout;
        loop {
            self.device.poll(&mut self.host);
            for event in self.device.take_device_events() {
                match event {
                    DeviceEvent::Disconnected {
                        connection_handle: handle,
                        reason,
                    } if handle == connection_handle => return Ok(reason),
                    DeviceEvent::DisconnectionFailed {
                        connection_handle: handle,
                        status,
                    } if handle == connection_handle => {
                        return Err(Error::Remote(format!(
                            "HCI Disconnect failed for handle 0x{handle:04X} with status 0x{status:02X}"
                        )));
                    }
                    _ => {}
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Remote(format!(
                    "timed out waiting for Disconnection Complete on handle 0x{connection_handle:04X}"
                )));
            }
            match self.host.wait_for_device_activity(
                &mut self.device,
                remaining.min(WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet | ExternalHostActivity::Timeout => {}
                ExternalHostActivity::Ended => {
                    return Err(Error::Remote(
                        "HCI transport ended while disconnecting".into(),
                    ));
                }
            }
        }
    }
}
