use std::time::{Duration, Instant};

use bumble::{Address, AddressType};
use bumble_hci::{Command, ReturnParameters};
use bumble_host::{DeviceConnectionTransport, DeviceEvent, LeConnectionInfo};
use bumble_transport::{Error, ExternalHostActivity, Result as TransportResult};

use crate::{AttachedController, LeAclReport, ScanObservation};

const WAIT_SLICE: Duration = Duration::from_millis(250);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const CANCEL_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const LOCAL_HOST_TERMINATED_REASON: u8 = 0x13;

/// Generic LE connection manager over the external BE200 controller.
///
/// This layer owns HCI initiating policy and lifecycle. It deliberately knows
/// nothing about CSIS, RSI, Swift Pair, SMP, or peer-key persistence. The
/// scanner-supplied address and address type are accepted as opaque target
/// identity and are registered with the controller Filter Accept List.
pub struct LeConnectionManager<'a> {
    controller: &'a mut AttachedController,
    in_flight: bool,
}

impl AttachedController {
    pub fn le_connection_manager(&mut self) -> LeConnectionManager<'_> {
        LeConnectionManager {
            controller: self,
            in_flight: false,
        }
    }
}

impl LeConnectionManager<'_> {
    /// Establish one direct LE connection using the controller Filter Accept
    /// List. This mirrors the generic direct/auto-connect path used by mature
    /// Bluetooth hosts instead of issuing a CSIS-specific explicit-peer
    /// connection command.
    ///
    /// The current R2 HIL owns the controller exclusively, so this method uses
    /// an exclusive one-entry Filter Accept List transaction:
    ///
    /// clear -> add scanner address -> initiate with filter policy -> clear.
    pub fn direct_connect_filter_accept_list(
        &mut self,
        observation: &ScanObservation,
        timeout: Duration,
    ) -> TransportResult<LeAclReport> {
        if self.in_flight {
            return Err(Error::Remote(
                "LE connection manager already has an in-flight procedure".into(),
            ));
        }
        if self.controller.device.is_le_connecting() {
            return Err(Error::Remote(
                "Bumble Device reports an existing LE initiation procedure".into(),
            ));
        }
        if !observation.is_connectable {
            return Err(Error::InvalidSpec(
                "selected scan observation is not connectable".into(),
            ));
        }
        if !matches!(observation.address_type, 0 | 1) {
            return Err(Error::InvalidSpec(format!(
                "Filter Accept List direct connect requires public/random scanner address; got type {}",
                observation.address_type
            )));
        }

        let target = Address::parse(
            &observation.address,
            AddressType(observation.address_type),
        )
        .map_err(|error| Error::InvalidSpec(format!(
            "invalid scanner-supplied target address: {error}"
        )))?;

        self.in_flight = true;
        let result = self.direct_connect_filter_accept_list_inner(
            observation,
            target,
            timeout,
        );
        self.in_flight = false;
        result
    }

    fn direct_connect_filter_accept_list_inner(
        &mut self,
        observation: &ScanObservation,
        target: Address,
        timeout: Duration,
    ) -> TransportResult<LeAclReport> {
        println!(
            "[R2/CONN-MGR] begin direct connection: scanner_target={} type={} policy=filter-accept-list",
            observation.address, observation.address_type
        );

        let capacity = self.read_filter_accept_list_capacity()?;
        println!(
            "[R2/CONN-MGR] controller Filter Accept List capacity={capacity}; HIL transaction owns the list exclusively"
        );

        self.successful_command(Command::LeClearFilterAcceptList)?;
        println!("[R2/CONN-MGR] Filter Accept List cleared");

        if let Err(error) = self.successful_command(Command::LeAddDeviceToFilterAcceptList {
            address_type: observation.address_type,
            address: target.clone(),
        }) {
            let _ = self.clear_filter_accept_list_best_effort("add failure");
            return Err(Error::Remote(format!(
                "failed to add scanner target {} to Filter Accept List: {error}",
                observation.address
            )));
        }
        println!(
            "[R2/CONN-MGR] Filter Accept List add: address={} type={} PASS",
            observation.address, observation.address_type
        );

        // Bluetooth Core: peer fields are ignored when Initiator_Filter_Policy
        // selects the Filter Accept List. Use the canonical zero public address
        // rather than smuggling CSIS identity policy into this layer.
        let any_peer = Address::parse(
            "00:00:00:00:00:00",
            AddressType::PUBLIC_DEVICE,
        )
        .map_err(|error| Error::InvalidSpec(format!(
            "failed to construct Filter Accept List wildcard peer: {error}"
        )))?;

        self.controller.device.poll(&mut self.controller.host);
        self.controller.device.take_device_events();

        let command = Command::LeExtendedCreateConnection {
            initiator_filter_policy: 1,
            own_address_type: 0,
            peer_address_type: 0,
            peer_address: any_peer,
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
            "[R2/CONN-MGR] submit HCI 0x2043: filter_policy=1 own_type=public peer_fields=ANY phys=1M scan=0x0010/0x0010"
        );
        let submit_started = Instant::now();
        if let Err(error) = self.successful_command_with_timeout(command, timeout) {
            println!(
                "[R2/CONN-MGR] create-connection submission failed after {:?}: {error}",
                submit_started.elapsed()
            );
            let cancel = self.cancel_and_settle();
            let cleanup = self.clear_filter_accept_list_best_effort("submission failure");
            return Err(Error::Remote(format!(
                "Filter Accept List create-connection submission failed: {error}; cancel={cancel}; cleanup={cleanup}"
            )));
        }
        println!(
            "[R2/CONN-MGR] HCI 0x2043 Command Status: SUCCESS after {:?}",
            submit_started.elapsed()
        );

        let deadline = Instant::now() + timeout;
        let started = Instant::now();
        let mut idle_slices: u32 = 0;

        loop {
            self.controller.device.poll(&mut self.controller.host);

            for event in self.controller.device.take_device_events() {
                match event {
                    DeviceEvent::LeConnectionEstablished(info) => {
                        println!(
                            "[R2/CONN-MGR] LE connection established after {:?}: handle=0x{:04X} controller_peer={} expected_scanner_target={}",
                            started.elapsed(),
                            info.connection_handle,
                            info.peer_address.to_string(true),
                            observation.address
                        );
                        if let Err(error) = self.successful_command(Command::LeClearFilterAcceptList) {
                            let _ = self.controller.disconnect_le_connection(
                                info.connection_handle,
                                LOCAL_HOST_TERMINATED_REASON,
                                DISCONNECT_TIMEOUT,
                            );
                            return Err(Error::Remote(format!(
                                "connection succeeded but Filter Accept List cleanup failed; connection was terminated: {error}"
                            )));
                        }
                        println!("[R2/CONN-MGR] Filter Accept List post-connect cleanup: PASS");
                        return Ok(report_from_connection(info));
                    }
                    DeviceEvent::ConnectionFailed {
                        transport: DeviceConnectionTransport::Le,
                        peer_address,
                        status,
                    } => {
                        let cleanup = self.clear_filter_accept_list_best_effort("connection failure");
                        return Err(Error::Remote(format!(
                            "Filter Accept List direct connection failed with HCI status 0x{status:02X}; controller_peer={}; cleanup={cleanup}",
                            peer_address.to_string(true)
                        )));
                    }
                    _ => {}
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // One final poll closes the boundary where Connection Complete
                // arrived at the deadline but was not yet journaled.
                self.controller.device.poll(&mut self.controller.host);
                for event in self.controller.device.take_device_events() {
                    if let DeviceEvent::LeConnectionEstablished(info) = event {
                        println!(
                            "[R2/CONN-MGR] late Connection Complete at timeout boundary: handle=0x{:04X}",
                            info.connection_handle
                        );
                        self.successful_command(Command::LeClearFilterAcceptList)?;
                        return Ok(report_from_connection(info));
                    }
                }

                println!(
                    "[R2/CONN-MGR] timeout after {:?}; cancelling generic direct connection",
                    started.elapsed()
                );
                let cancel = self.cancel_and_settle();
                let cleanup = self.clear_filter_accept_list_best_effort("timeout");
                return Err(Error::Remote(format!(
                    "timed out waiting for Filter Accept List direct connection; cancel={cancel}; cleanup={cleanup}"
                )));
            }

            match self.controller.host.wait_for_device_activity(
                &mut self.controller.device,
                remaining.min(WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet => {
                    idle_slices = 0;
                    println!(
                        "[R2/CONN-MGR] HCI activity while initiating at {:?}",
                        started.elapsed()
                    );
                }
                ExternalHostActivity::Timeout => {
                    idle_slices = idle_slices.saturating_add(1);
                    if idle_slices == 1 || idle_slices % 4 == 0 {
                        println!(
                            "[R2/CONN-MGR] waiting for Connection Complete: {:?} elapsed",
                            started.elapsed()
                        );
                    }
                }
                ExternalHostActivity::Ended => {
                    let cleanup = self.clear_filter_accept_list_best_effort("transport ended");
                    return Err(Error::Remote(format!(
                        "HCI transport ended during direct connection; cleanup={cleanup}"
                    )));
                }
            }
        }
    }

    fn read_filter_accept_list_capacity(&mut self) -> TransportResult<u8> {
        let response = self
            .controller
            .host
            .send_command(Command::LeReadFilterAcceptListSize, COMMAND_TIMEOUT)?;
        if response.status() != Some(0) {
            return Err(Error::Remote(format!(
                "LE Read Filter Accept List Size failed with status {:?}",
                response.status()
            )));
        }
        match response.return_parameters() {
            Some(ReturnParameters::LeReadFilterAcceptListSize {
                filter_accept_list_size,
                ..
            }) => Ok(*filter_accept_list_size),
            other => Err(Error::Remote(format!(
                "LE Read Filter Accept List Size returned unexpected parameters: {other:?}"
            ))),
        }
    }

    fn successful_command(&mut self, command: Command) -> TransportResult<()> {
        self.successful_command_with_timeout(command, COMMAND_TIMEOUT)
    }

    fn successful_command_with_timeout(
        &mut self,
        command: Command,
        timeout: Duration,
    ) -> TransportResult<()> {
        let opcode = command.op_code();
        let response = self.controller.host.send_command(command, timeout)?;
        if response.status() == Some(0) {
            Ok(())
        } else {
            Err(Error::Remote(format!(
                "HCI command {opcode:#06x} failed with status {:?}",
                response.status()
            )))
        }
    }

    fn cancel_and_settle(&mut self) -> String {
        let response = self
            .controller
            .host
            .send_command(Command::LeCreateConnectionCancel, COMMAND_TIMEOUT);
        let response_text = match response {
            Ok(response) => format!("status={:?}", response.status()),
            Err(error) => format!("error={error}"),
        };

        let deadline = Instant::now() + CANCEL_SETTLE_TIMEOUT;
        while Instant::now() < deadline {
            self.controller.device.poll(&mut self.controller.host);
            for event in self.controller.device.take_device_events() {
                match event {
                    DeviceEvent::ConnectionFailed {
                        transport: DeviceConnectionTransport::Le,
                        status,
                        ..
                    } => return format!("{response_text}, settled_failure=0x{status:02X}"),
                    DeviceEvent::LeConnectionEstablished(info) => {
                        let disconnect = self.controller.disconnect_le_connection(
                            info.connection_handle,
                            LOCAL_HOST_TERMINATED_REASON,
                            DISCONNECT_TIMEOUT,
                        );
                        return format!(
                            "{response_text}, late_connection=0x{:04X}, disconnect={disconnect:?}",
                            info.connection_handle
                        );
                    }
                    _ => {}
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.controller.host.wait_for_device_activity(
                &mut self.controller.device,
                remaining.min(WAIT_SLICE),
            ) {
                Ok(ExternalHostActivity::Packet | ExternalHostActivity::Timeout) => {}
                Ok(ExternalHostActivity::Ended) => {
                    return format!("{response_text}, transport-ended")
                }
                Err(error) => return format!("{response_text}, settle-error={error}"),
            }
        }

        format!("{response_text}, settle-timeout")
    }

    fn clear_filter_accept_list_best_effort(&mut self, context: &str) -> String {
        match self.successful_command(Command::LeClearFilterAcceptList) {
            Ok(()) => format!("PASS({context})"),
            Err(error) => format!("ERROR({context}): {error}"),
        }
    }
}

fn report_from_connection(info: LeConnectionInfo) -> LeAclReport {
    LeAclReport {
        connection_handle: info.connection_handle,
        role: info.role,
        peer_address: info.peer_address.to_string(true),
        local_address: String::new(),
        local_address_type: 0,
        connection_interval: info.parameters.connection_interval,
        peripheral_latency: info.parameters.peripheral_latency,
        supervision_timeout: info.parameters.supervision_timeout,
    }
}
