use std::time::{Duration, Instant};

use bumble::{Address, AddressType};
use bumble_hci::Command;
use bumble_host::DeviceEvent;
use bumble_transport::{Error, ExternalHostActivity, Result as TransportResult};

use crate::{AttachedController, ScanObservation};

const WAIT_SLICE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeAclReport {
    pub connection_handle: u16,
    pub role: u8,
    pub peer_address: String,
    pub connection_interval: u16,
    pub peripheral_latency: u16,
    pub supervision_timeout: u16,
}

impl AttachedController {
    /// R2.3: create one LE ACL from a candidate returned by this process's
    /// immediately preceding scan. The local controller uses its public
    /// address, matching the R1.6/R2.2 no-reset path.
    pub fn connect_scanned_observation(
        &mut self,
        observation: &ScanObservation,
        timeout: Duration,
    ) -> TransportResult<LeAclReport> {
        if !observation.is_connectable {
            return Err(Error::InvalidSpec(
                "selected advertisement is not connectable".into(),
            ));
        }
        if !matches!(observation.address_type, 0 | 1) {
            return Err(Error::InvalidSpec(format!(
                "R2.3 only accepts direct public/random peer addresses; scan returned address type {}",
                observation.address_type
            )));
        }

        let peer = Address::parse(
            &observation.address,
            AddressType(observation.address_type),
        )
        .map_err(|error| Error::InvalidSpec(format!("invalid scanned peer address: {error}")))?;

        // The scan command is already disabled before R2.3 arrives here.
        // Drain stale scan events so connection failure/success reporting is
        // scoped to this one procedure.
        self.device.poll(&mut self.host);
        self.device.take_device_events();

        let command = Command::LeExtendedCreateConnection {
            initiator_filter_policy: 0,
            own_address_type: 0,
            peer_address_type: observation.address_type,
            peer_address: peer.clone(),
            initiating_phys: 0x01,
            scan_intervals: vec![0x0010],
            scan_windows: vec![0x0010],
            connection_interval_mins: vec![24],
            connection_interval_maxs: vec![40],
            max_latencies: vec![0],
            supervision_timeouts: vec![42],
            min_ce_lengths: vec![0],
            max_ce_lengths: vec![0],
        };

        successful_status(&mut self.host, command, timeout)?;
        let deadline = Instant::now() + timeout;

        loop {
            self.device.poll(&mut self.host);

            if let Some(handle) = self.device.connection_handle_for_peer(&peer) {
                let connection = self
                    .device
                    .le_connection(handle)
                    .cloned()
                    .ok_or_else(|| Error::Remote(
                        "LE connection handle appeared without connection metadata".into(),
                    ))?;
                return Ok(LeAclReport {
                    connection_handle: handle,
                    role: connection.role,
                    peer_address: connection.peer_address.to_string(true),
                    connection_interval: connection.parameters.connection_interval,
                    peripheral_latency: connection.parameters.peripheral_latency,
                    supervision_timeout: connection.parameters.supervision_timeout,
                });
            }

            for event in self.device.take_device_events() {
                if let DeviceEvent::ConnectionFailed {
                    peer_address,
                    status,
                    ..
                } = event
                {
                    if peer_address == peer {
                        return Err(Error::Remote(format!(
                            "LE Extended Create Connection failed with HCI status 0x{status:02X}"
                        )));
                    }
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let _ = self.host.send_command(
                    Command::LeCreateConnectionCancel,
                    Duration::from_secs(2),
                );
                return Err(Error::Remote(
                    "timed out waiting for LE Enhanced Connection Complete".into(),
                ));
            }

            match self.host.wait_for_device_activity(
                &mut self.device,
                remaining.min(WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet | ExternalHostActivity::Timeout => {}
                ExternalHostActivity::Ended => {
                    return Err(Error::Remote(
                        "Bumble HCI transport ended while connecting".into(),
                    ))
                }
            }
        }
    }

    /// Keep one established ACL alive while still pumping controller events.
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
                        "Bumble HCI transport ended during ACL linger".into(),
                    ))
                }
            }
        }

        Ok(())
    }

    /// Submit a normal HCI Disconnect and wait for the matching completion.
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
                        )))
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
                        "Bumble HCI transport ended while disconnecting".into(),
                    ))
                }
            }
        }
    }
}

fn successful_status(
    host: &mut bumble_transport::ExternalHost,
    command: Command,
    timeout: Duration,
) -> TransportResult<()> {
    let opcode = command.op_code();
    let response = host.send_command(command, timeout)?;
    if response.status() == Some(0) {
        Ok(())
    } else {
        Err(Error::Remote(format!(
            "HCI command {opcode:#06x} failed with status {:?}",
            response.status()
        )))
    }
}
