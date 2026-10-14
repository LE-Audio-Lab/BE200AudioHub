use std::time::{Duration, Instant};

use bumble::keys::{Key, KeyStore, MemoryKeyStore, PairingKeys};
use bumble::{Address, AddressType};
use bumble_crypto::{random_128, EccKey};
use bumble_host::DeviceEvent;
use bumble_smp::{
    AcceptAllDelegate, IdentityAddressType, PairingCapabilities, PairingConfig, PairingRole,
    ScPairingSession, ScPairingState, SmpPdu, SMP_CID,
};
use bumble_transport::{Error, ExternalHostActivity, Result as TransportResult};

use crate::{AttachedController, BondStore};

const WAIT_SLICE: Duration = Duration::from_millis(250);

/// Peer keys produced by one completed and encrypted SMP transaction, but not
/// yet committed to the persistent BondStore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisionalBond {
    identity_address: String,
    address_type: u8,
    authenticated: bool,
    keys: PairingKeys,
    ltk: [u8; 16],
}

impl ProvisionalBond {
    pub fn identity_address(&self) -> &str {
        &self.identity_address
    }

    pub fn address_type(&self) -> u8 {
        self.address_type
    }

    pub fn authenticated(&self) -> bool {
        self.authenticated
    }

    pub fn has_irk(&self) -> bool {
        self.keys.irk.is_some()
    }

    pub fn ltk(&self) -> [u8; 16] {
        self.ltk
    }

    pub(crate) fn pairing_keys(&self) -> &PairingKeys {
        &self.keys
    }
}

/// Synchronous generic LE bond manager.
///
/// V4 retains the endpoint-order handling from V3 and corrects one pinned
/// Bumble-RS host defect: the LE Meta Event parser constructs every LE
/// connection address as RANDOM_DEVICE and Device discards the separate HCI
/// peer_address_type field. For a public scanner/FAL target, that corrupts the
/// f5/f6 address-type bit and the peer rejects our DHKey Check with SMP 0x0B.
///
/// When the caller supplies an expected public identity whose bytes exactly
/// match the controller metadata, V4 reconstructs the SC responder link
/// address with PUBLIC_DEVICE. It never changes address bytes and refuses to
/// use the expected identity as a link-address override when the bytes differ.
pub struct LeBondManager<'a> {
    controller: &'a mut AttachedController,
}

impl AttachedController {
    pub fn le_bond_manager(&mut self) -> LeBondManager<'_> {
        LeBondManager { controller: self }
    }
}

impl LeBondManager<'_> {
    pub fn pair_provisional(
        &mut self,
        connection_handle: u16,
        connection_local_address: &str,
        connection_local_address_type: u8,
        expected_peer_identity: Option<&str>,
        store: &BondStore,
        timeout: Duration,
    ) -> TransportResult<ProvisionalBond> {
        let local_address = Address::parse(
            connection_local_address,
            AddressType(connection_local_address_type),
        )
        .map_err(|error| {
            Error::InvalidSpec(format!("invalid connection-local address: {error}"))
        })?;

        let connection = self
            .controller
            .device
            .le_connection(connection_handle)
            .cloned()
            .ok_or_else(|| {
                Error::Remote(format!(
                    "unknown LE connection handle 0x{connection_handle:04X}"
                ))
            })?;
        if connection.role != 0 {
            return Err(Error::Remote(
                "LeBondManager requires the local controller to be LE Central".into(),
            ));
        }

        let host_metadata_peer = connection.peer_address.clone();
        let peer_address = corrected_sc_peer_link_address(
            &host_metadata_peer,
            expected_peer_identity,
        )?;

        println!(
            "[R2/BOND-MGR] SC address context: initiator={} type={} responder={} type={} host_metadata_responder={} type={}",
            local_address.to_string(false),
            local_address.address_type().0,
            peer_address.to_string(false),
            peer_address.address_type().0,
            host_metadata_peer.to_string(false),
            host_metadata_peer.address_type().0,
        );
        if peer_address.address_type() != host_metadata_peer.address_type() {
            println!(
                "[R2/BOND-MGR] corrected pinned Bumble-RS LE address-type loss: same peer bytes, host metadata type={} -> SC link type={}",
                host_metadata_peer.address_type().0,
                peer_address.address_type().0,
            );
        }

        let config = PairingConfig {
            secure_connections: true,
            ct2: false,
            mitm: false,
            bonding: true,
            capabilities: PairingCapabilities::default(),
            identity_address_type: Some(IdentityAddressType::Public),
            oob: None,
        };

        println!(
            "[R2/BOND-MGR] begin provisional bond: handle=0x{connection_handle:04X} local={} type={} peer={} type={} SC={} MITM={} bonding={}",
            local_address.to_string(false),
            local_address.address_type().0,
            peer_address.to_string(false),
            peer_address.address_type().0,
            config.secure_connections,
            config.mitm,
            config.bonding
        );
        println!(
            "[R2/BOND-MGR] key distribution: initiator=0x{:02X} responder=0x{:02X} max_key_size={}",
            config.capabilities.local_initiator_key_distribution.0,
            config.capabilities.local_responder_key_distribution.0,
            config.capabilities.maximum_encryption_key_size
        );

        let mut pairing = ScPairingSession::new(
            PairingRole::Initiator,
            config,
            Box::new(AcceptAllDelegate),
            local_address,
            peer_address,
            EccKey::generate(),
            random_128(),
        )
        .map_err(|error| {
            Error::Remote(format!("failed to create provisional SC session: {error}"))
        })?;

        pairing
            .set_local_key_material(store.local_key_material()?)
            .map_err(|error| {
                Error::Remote(format!(
                    "failed to inject persistent local key material: {error}"
                ))
            })?;
        pairing
            .start()
            .map_err(|error| Error::Remote(format!("failed to start SC pairing: {error}")))?;

        println!(
            "[R2/BOND-MGR] Pairing Request queued; state={:?}; persistent keys are still untouched",
            pairing.state()
        );

        let started_at = Instant::now();
        let deadline = started_at + timeout;
        let mut last_state = pairing.state();
        let mut last_encrypted = self
            .controller
            .device
            .is_encrypted_on_handle(connection_handle);
        let mut encryption_started = false;
        let mut marked_encrypted = false;
        let mut inferred_encryption = false;
        let mut complete_wait_logged = false;
        let mut idle_slices = 0u32;
        let mut peer_failure_reason: Option<u8> = None;

        let keys = loop {
            flush_sc_outbound(
                &mut pairing,
                &mut self.controller.host,
                &mut self.controller.device,
                connection_handle,
            )?;

            self.controller.device.poll(&mut self.controller.host);

            mark_encrypted_from_controller_state(
                &mut pairing,
                &mut self.controller.host,
                &mut self.controller.device,
                connection_handle,
                &mut marked_encrypted,
            )?;

            for event in self.controller.device.take_device_events() {
                match event {
                    DeviceEvent::EncryptionChange {
                        status,
                        connection_handle: handle,
                        encryption_enabled,
                        encryption_key_size,
                    } if handle == connection_handle => {
                        println!(
                            "[R2/BOND-MGR] HCI Encryption Change: status=0x{status:02X} enabled={} key_size={}",
                            encryption_enabled,
                            encryption_key_size
                        );
                        if status != 0 {
                            return Err(Error::Remote(format!(
                                "controller rejected pairing encryption on handle 0x{handle:04X}: HCI status 0x{status:02X}"
                            )));
                        }
                        if inferred_encryption {
                            println!(
                                "[R2/BOND-MGR] controller encryption confirmation arrived after Bulk-IN key-distribution inference"
                            );
                        }
                    }
                    DeviceEvent::Disconnected {
                        connection_handle: handle,
                        reason,
                    } if handle == connection_handle => {
                        return Err(Error::Remote(format!(
                            "LE ACL 0x{handle:04X} disconnected during provisional pairing, reason=0x{reason:02X}"
                        )));
                    }
                    DeviceEvent::PairingFailed {
                        connection_handle: handle,
                        reason,
                    } if handle == connection_handle => {
                        println!(
                            "[R2/BOND-MGR] Device pairing event: FAILED reason={reason:?}"
                        );
                    }
                    _ => {}
                }
            }

            for payload in self
                .controller
                .device
                .take_l2cap_on_handle(connection_handle, SMP_CID)
            {
                let pdu = SmpPdu::from_bytes(&payload)
                    .map_err(|error| Error::Remote(format!("invalid SMP PDU: {error}")))?;
                println!(
                    "[R2/BOND-MGR] RX SMP {} code=0x{:02X} bytes={} while state={:?} encrypted={}",
                    pdu_label(&pdu),
                    pdu.code(),
                    hex_bytes(&payload),
                    pairing.state(),
                    self.controller
                        .device
                        .is_encrypted_on_handle(connection_handle)
                );

                if matches!(pdu, SmpPdu::SecurityRequest { .. }) {
                    println!(
                        "[R2/BOND-MGR] peer Security Request observed while initiator session is active"
                    );
                    continue;
                }

                if let SmpPdu::PairingFailed { reason } = pdu {
                    peer_failure_reason = Some(reason);
                    println!(
                        "[R2/BOND-MGR] peer sent Pairing Failed reason=0x{reason:02X}"
                    );
                    pairing
                        .process(SmpPdu::PairingFailed { reason })
                        .map_err(|error| {
                            Error::Remote(format!("SC pairing process failed: {error}"))
                        })?;
                    continue;
                }

                if pairing.state() == ScPairingState::WaitEncryption
                    && is_key_distribution_pdu(&pdu)
                {
                    infer_encryption_from_key_distribution(
                        &mut pairing,
                        &mut self.controller.host,
                        &mut self.controller.device,
                        connection_handle,
                        encryption_started,
                        &mut marked_encrypted,
                        &mut inferred_encryption,
                    )?;
                }

                pairing.process(pdu).map_err(|error| {
                    Error::Remote(format!("SC pairing process failed: {error}"))
                })?;
            }

            for request in self
                .controller
                .device
                .take_long_term_key_requests_on_handle(connection_handle)
            {
                let Some(key) = pairing.ltk() else {
                    self.controller.device.reject_long_term_key_request(
                        &mut self.controller.host,
                        request.connection_handle,
                    );
                    return Err(Error::Remote(format!(
                        "controller requested LTK before SC produced one on handle 0x{:04X}",
                        request.connection_handle
                    )));
                };
                println!(
                    "[R2/BOND-MGR] HCI LE Long Term Key Request: handle=0x{:04X} ediv=0x{:04X} rand={} -> reply with pairing LTK",
                    request.connection_handle,
                    request.encryption_diversifier,
                    hex_bytes(&request.random_number)
                );
                if !self.controller.device.reply_long_term_key_request(
                    &mut self.controller.host,
                    request.connection_handle,
                    key,
                ) {
                    return Err(Error::Remote(format!(
                        "failed to answer controller LTK request on handle 0x{:04X}",
                        request.connection_handle
                    )));
                }
            }

            if pairing.state() == ScPairingState::WaitEncryption && !encryption_started {
                let key = pairing.ltk().ok_or_else(|| {
                    Error::Remote("SC reached encryption without an LTK".into())
                })?;
                if !self.controller.device.enable_encryption_on_handle(
                    &mut self.controller.host,
                    connection_handle,
                    key,
                ) {
                    return Err(Error::Remote(format!(
                        "failed to start encryption on handle 0x{connection_handle:04X}"
                    )));
                }
                encryption_started = true;
                println!(
                    "[R2/BOND-MGR] LE Start Encryption submitted with SC pairing LTK"
                );
            }

            mark_encrypted_from_controller_state(
                &mut pairing,
                &mut self.controller.host,
                &mut self.controller.device,
                connection_handle,
                &mut marked_encrypted,
            )?;

            flush_sc_outbound(
                &mut pairing,
                &mut self.controller.host,
                &mut self.controller.device,
                connection_handle,
            )?;

            if pairing.state() == ScPairingState::Failed {
                return Err(Error::Remote(match peer_failure_reason {
                    Some(reason) => format!(
                        "SMP pairing failed after peer Pairing Failed reason=0x{reason:02X}; state={:?}; encrypted={}",
                        pairing.state(),
                        self.controller
                            .device
                            .is_encrypted_on_handle(connection_handle)
                    ),
                    None => format!(
                        "SMP pairing failed locally: {:?}; state={:?}; encrypted={}; inferred_encryption={}",
                        pairing.failure(),
                        pairing.state(),
                        self.controller
                            .device
                            .is_encrypted_on_handle(connection_handle),
                        inferred_encryption
                    ),
                }));
            }

            if pairing.state() == ScPairingState::Complete {
                if self
                    .controller
                    .device
                    .is_encrypted_on_handle(connection_handle)
                {
                    let keys = pairing.pairing_keys().ok_or_else(|| {
                        Error::Remote("SC pairing completed without keys".into())
                    })?;
                    println!(
                        "[R2/BOND-MGR] SMP complete after {:?}; encrypted=true inferred_encryption={}",
                        started_at.elapsed(),
                        inferred_encryption
                    );
                    break keys;
                }

                if !complete_wait_logged {
                    println!(
                        "[R2/BOND-MGR] SMP key distribution complete; waiting for controller Encryption Change confirmation before verifier handoff"
                    );
                    complete_wait_logged = true;
                }
            }

            let state = pairing.state();
            if state != last_state {
                println!(
                    "[R2/BOND-MGR] state: {:?} -> {:?} at {:?}",
                    last_state,
                    state,
                    started_at.elapsed()
                );
                last_state = state;
            }

            let encrypted = self
                .controller
                .device
                .is_encrypted_on_handle(connection_handle);
            if encrypted != last_encrypted {
                println!(
                    "[R2/BOND-MGR] ACL encryption: {} -> {} at {:?}",
                    last_encrypted,
                    encrypted,
                    started_at.elapsed()
                );
                last_encrypted = encrypted;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Remote(format!(
                    "timed out provisional pairing on handle 0x{connection_handle:04X}; state={:?}; encrypted={}; inferred_encryption={}",
                    pairing.state(),
                    self.controller
                        .device
                        .is_encrypted_on_handle(connection_handle),
                    inferred_encryption
                )));
            }

            match self.controller.host.wait_for_device_activity(
                &mut self.controller.device,
                remaining.min(WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet => {
                    idle_slices = 0;
                }
                ExternalHostActivity::Timeout => {
                    idle_slices = idle_slices.saturating_add(1);
                    if idle_slices == 1 || idle_slices % 4 == 0 {
                        println!(
                            "[R2/BOND-MGR] waiting: {:?} state={:?} encrypted={} inferred_encryption={}",
                            started_at.elapsed(),
                            pairing.state(),
                            self.controller
                                .device
                                .is_encrypted_on_handle(connection_handle),
                            inferred_encryption
                        );
                    }
                }
                ExternalHostActivity::Ended => {
                    return Err(Error::Remote(
                        "HCI transport ended during provisional pairing".into(),
                    ));
                }
            }
        };

        let mut staged = MemoryKeyStore::new();
        if !pairing.store_bond(&mut staged).map_err(|error| {
            Error::Remote(format!("failed to stage SC bond: {error}"))
        })? {
            return Err(Error::Remote(
                "peer completed pairing without bondable key material".into(),
            ));
        }

        let staged_all = staged.get_all().map_err(|error| {
            Error::Remote(format!("failed to inspect staged bond: {error}"))
        })?;
        let identity = staged_all
            .iter()
            .find(|(_, staged_keys)| *staged_keys == keys)
            .map(|(name, _)| name.clone())
            .or_else(|| (staged_all.len() == 1).then(|| staged_all[0].0.clone()))
            .ok_or_else(|| {
                Error::Remote(
                    "staged pairing keys did not expose one unambiguous peer identity".into(),
                )
            })?;

        if let Some(expected) = expected_peer_identity {
            if !identity.eq_ignore_ascii_case(expected) {
                return Err(Error::Remote(format!(
                    "paired peer identity {identity} did not match expected {expected}; provisional keys were discarded"
                )));
            }
            println!(
                "[R2/BOND-MGR] staged identity validation: PASS actual={} expected={}",
                identity, expected
            );
        }

        let staged_keys = staged
            .get(&identity)
            .map_err(|error| {
                Error::Remote(format!("failed to read staged keys for {identity}: {error}"))
            })?
            .ok_or_else(|| {
                Error::Remote(format!(
                    "staged bond for {identity} disappeared before transaction handoff"
                ))
            })?;

        let selected_key = selected_encryption_key(&staged_keys, true).ok_or_else(|| {
            Error::Remote(format!("provisional bond for {identity} has no usable LTK"))
        })?;
        let ltk: [u8; 16] = selected_key.value.as_slice().try_into().map_err(|_| {
            Error::Remote(format!("provisional LTK for {identity} is not 16 bytes"))
        })?;
        if staged_keys.irk.is_none() {
            return Err(Error::Remote(format!(
                "provisional bond for {identity} has no peer IRK"
            )));
        }

        let address_type = staged_keys
            .address_type
            .map(|value| value.0)
            .unwrap_or(AddressType::RANDOM_DEVICE.0);
        let authenticated = selected_key.authenticated;

        println!(
            "[R2/BOND-MGR] provisional transaction READY identity={} type={} LTK=true IRK=true authenticated={}; persistent store unchanged",
            identity,
            address_type,
            authenticated
        );

        Ok(ProvisionalBond {
            identity_address: identity,
            address_type,
            authenticated,
            keys: staged_keys,
            ltk,
        })
    }
}

fn corrected_sc_peer_link_address(
    host_metadata_peer: &Address,
    expected_peer_identity: Option<&str>,
) -> TransportResult<Address> {
    let Some(expected) = expected_peer_identity else {
        return Ok(host_metadata_peer.clone());
    };

    let expected_without_qualifier = expected.strip_suffix("/P").unwrap_or(expected);
    let metadata_value = host_metadata_peer.to_string(false);
    if !metadata_value.eq_ignore_ascii_case(expected_without_qualifier) {
        return Ok(host_metadata_peer.clone());
    }

    Address::parse(expected_without_qualifier, AddressType::PUBLIC_DEVICE).map_err(|error| {
        Error::InvalidSpec(format!(
            "expected public peer identity could not reconstruct SC link address: {error}"
        ))
    })
}

fn mark_encrypted_from_controller_state(
    pairing: &mut ScPairingSession,
    host: &mut bumble_host::LocalLink,
    device: &mut bumble_host::Device,
    connection_handle: u16,
    marked_encrypted: &mut bool,
) -> TransportResult<()> {
    if pairing.state() == ScPairingState::WaitEncryption
        && device.is_encrypted_on_handle(connection_handle)
        && !*marked_encrypted
    {
        pairing.mark_encrypted().map_err(|error| {
            Error::Remote(format!("failed to mark SC encrypted: {error}"))
        })?;
        *marked_encrypted = true;
        println!(
            "[R2/BOND-MGR] encryption handoff: controller ACL encrypted -> SC key distribution enabled"
        );
        flush_sc_outbound(pairing, host, device, connection_handle)?;
    }
    Ok(())
}

fn infer_encryption_from_key_distribution(
    pairing: &mut ScPairingSession,
    host: &mut bumble_host::LocalLink,
    device: &mut bumble_host::Device,
    connection_handle: u16,
    encryption_started: bool,
    marked_encrypted: &mut bool,
    inferred_encryption: &mut bool,
) -> TransportResult<()> {
    if pairing.state() != ScPairingState::WaitEncryption || *marked_encrypted {
        return Ok(());
    }
    if !encryption_started {
        return Err(Error::Remote(
            "received SMP key-distribution PDU before LE Start Encryption was submitted".into(),
        ));
    }

    pairing.mark_encrypted().map_err(|error| {
        Error::Remote(format!(
            "failed to infer encrypted SC handoff from key-distribution PDU: {error}"
        ))
    })?;
    *marked_encrypted = true;
    *inferred_encryption = true;
    println!(
        "[R2/BOND-MGR] encryption handoff inferred: encrypted key-distribution ACL payload arrived on Bulk-IN before HCI Encryption Change on Interrupt-IN"
    );
    flush_sc_outbound(pairing, host, device, connection_handle)?;
    Ok(())
}

fn is_key_distribution_pdu(pdu: &SmpPdu) -> bool {
    matches!(
        pdu,
        SmpPdu::EncryptionInformation { .. }
            | SmpPdu::MasterIdentification { .. }
            | SmpPdu::IdentityInformation { .. }
            | SmpPdu::IdentityAddressInformation { .. }
            | SmpPdu::SigningInformation { .. }
    )
}

fn pdu_label(pdu: &SmpPdu) -> &'static str {
    match pdu {
        SmpPdu::PairingRequest(_) => "PairingRequest",
        SmpPdu::PairingResponse(_) => "PairingResponse",
        SmpPdu::PairingConfirm { .. } => "PairingConfirm",
        SmpPdu::PairingRandom { .. } => "PairingRandom",
        SmpPdu::PairingFailed { .. } => "PairingFailed",
        SmpPdu::PairingPublicKey { .. } => "PairingPublicKey",
        SmpPdu::PairingDhKeyCheck { .. } => "PairingDhKeyCheck",
        SmpPdu::KeypressNotification { .. } => "KeypressNotification",
        SmpPdu::EncryptionInformation { .. } => "EncryptionInformation",
        SmpPdu::MasterIdentification { .. } => "MasterIdentification",
        SmpPdu::IdentityInformation { .. } => "IdentityInformation",
        SmpPdu::IdentityAddressInformation { .. } => "IdentityAddressInformation",
        SmpPdu::SigningInformation { .. } => "SigningInformation",
        SmpPdu::SecurityRequest { .. } => "SecurityRequest",
        SmpPdu::Generic { .. } => "Generic",
    }
}

fn flush_sc_outbound(
    pairing: &mut ScPairingSession,
    host: &mut bumble_host::LocalLink,
    device: &mut bumble_host::Device,
    connection_handle: u16,
) -> TransportResult<()> {
    for pdu in pairing.drain_outbound() {
        let bytes = pdu.to_bytes();
        println!(
            "[R2/BOND-MGR] TX SMP {} code=0x{:02X} bytes={} while state={:?}",
            pdu_label(&pdu),
            pdu.code(),
            hex_bytes(&bytes),
            pairing.state()
        );
        if !device.send_l2cap_on_handle(host, connection_handle, SMP_CID, &bytes) {
            return Err(Error::Remote(format!(
                "failed to send SMP PDU on handle 0x{connection_handle:04X}"
            )));
        }
    }
    Ok(())
}

fn selected_encryption_key(keys: &PairingKeys, local_is_central: bool) -> Option<&Key> {
    keys.ltk.as_ref().or_else(|| {
        if local_is_central {
            keys.ltk_central.as_ref()
        } else {
            keys.ltk_peripheral.as_ref()
        }
    })
}

fn hex_bytes(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}
