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

        println!(
            "[R2/ACL] submit HCI 0x2043 LE Extended Create Connection: peer={} type={} own_type=public phys=1M scan=0x0010/0x0010 conn_interval=24..40 latency=0 supervision=42",
            observation.address,
            observation.address_type
        );
        let submit_started = Instant::now();
        if let Err(error) = successful_status(&mut self.host, command, timeout) {
            println!(
                "[R2/ACL] HCI 0x2043 submit failed after {:?}: {error}",
                submit_started.elapsed()
            );
            println!(
                "[R2/ACL] ambiguous initiator state; issuing best-effort LE Create Connection Cancel..."
            );
            let cancel_result = self.host.send_command(
                Command::LeCreateConnectionCancel,
                Duration::from_secs(2),
            );
            match &cancel_result {
                Ok(response) => println!(
                    "[R2/ACL] cancel response: status={:?} host_state={:?}",
                    response.status(),
                    self.host.state()
                ),
                Err(cancel_error) => println!(
                    "[R2/ACL] cancel failed: {cancel_error}; host_state={:?}",
                    self.host.state()
                ),
            }
            self.device.poll(&mut self.host);
            self.device.take_device_events();
            return Err(Error::Remote(format!(
                "LE Extended Create Connection command submission failed: {error}; cancel={}",
                match cancel_result {
                    Ok(response) => format!("status={:?}", response.status()),
                    Err(cancel_error) => format!("error={cancel_error}"),
                }
            )));
        }
        println!(
            "[R2/ACL] HCI 0x2043 Command Status: SUCCESS after {:?}",
            submit_started.elapsed()
        );

        let wait_started = Instant::now();
        let deadline = Instant::now() + timeout;
        let mut idle_slices: u32 = 0;

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
                println!(
                    "[R2/ACL] LE Enhanced Connection Complete: SUCCESS after {:?}, handle=0x{handle:04X}",
                    wait_started.elapsed()
                );
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
                        println!(
                            "[R2/ACL] LE Enhanced Connection Complete: FAILED status=0x{status:02X} after {:?}",
                            wait_started.elapsed()
                        );
                        return Err(Error::Remote(format!(
                            "LE Extended Create Connection failed with HCI status 0x{status:02X}"
                        )));
                    }
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                println!(
                    "[R2/ACL] timed out waiting for LE Enhanced Connection Complete after {:?}; cancelling initiator...",
                    wait_started.elapsed()
                );
                let cancel = self.host.send_command(
                    Command::LeCreateConnectionCancel,
                    Duration::from_secs(2),
                );
                match &cancel {
                    Ok(response) => println!(
                        "[R2/ACL] timeout cancel response: status={:?}",
                        response.status()
                    ),
                    Err(error) => println!(
                        "[R2/ACL] timeout cancel failed: {error}"
                    ),
                }
                return Err(Error::Remote(format!(
                    "timed out waiting for LE Enhanced Connection Complete; cancel={}",
                    match cancel {
                        Ok(response) => format!("status={:?}", response.status()),
                        Err(error) => format!("error={error}"),
                    }
                )));
            }

            match self.host.wait_for_device_activity(
                &mut self.device,
                remaining.min(WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet => {
                    idle_slices = 0;
                    println!(
                        "[R2/ACL] HCI activity while initiating at {:?}",
                        wait_started.elapsed()
                    );
                }
                ExternalHostActivity::Timeout => {
                    idle_slices = idle_slices.saturating_add(1);
                    if idle_slices == 1 || idle_slices % 4 == 0 {
                        println!(
                            "[R2/ACL] waiting for LE Enhanced Connection Complete: {:?} elapsed",
                            wait_started.elapsed()
                        );
                    }
                }
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
