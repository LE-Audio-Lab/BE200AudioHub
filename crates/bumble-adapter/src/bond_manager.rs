use std::time::{Duration, Instant};

use bumble::keys::{Key, KeyStore, MemoryKeyStore, PairingKeys};
use bumble::{Address, AddressType};
use bumble_crypto::{random_128, EccKey};
use bumble_smp::{
    AcceptAllDelegate, IdentityAddressType, PairingCapabilities, PairingConfig, PairingRole,
    ScPairingSession, ScPairingState, SmpPdu, SMP_CID,
};
use bumble_transport::{Error, ExternalHostActivity, Result as TransportResult};

use crate::{AttachedController, BondStore};

const WAIT_SLICE: Duration = Duration::from_millis(250);

/// Peer keys produced by one completed and encrypted SMP transaction, but not
/// yet committed to the persistent BondStore.
///
/// The transaction remains provisional until the caller verifies higher-level
/// membership (for CSIP, the peer SIRK) and explicitly commits it.
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
/// The exclusive mutable borrow of AttachedController is the one-in-flight
/// bond guard: no second ACL/SMP transaction can be started through this API
/// until the current call returns.
pub struct LeBondManager<'a> {
    controller: &'a mut AttachedController,
}

impl AttachedController {
    pub fn le_bond_manager(&mut self) -> LeBondManager<'_> {
        LeBondManager { controller: self }
    }
}

impl LeBondManager<'_> {
    /// Run LE Secure Connections and stage the resulting peer keys in memory.
    ///
    /// This method never writes pairing-keys.json, never records set
    /// membership, and never refreshes the persistent address resolver. The
    /// encrypted ACL remains active for an immediate verifier step.
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
            "[R2/BOND-MGR] begin provisional bond: handle=0x{connection_handle:04X} local={} type={} peer={} SC={} MITM={} bonding={}",
            connection_local_address,
            connection_local_address_type,
            connection.peer_address.to_string(true),
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
            connection.peer_address,
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
        let mut idle_slices = 0u32;

        let keys = loop {
            flush_sc_outbound(
                &mut pairing,
                &mut self.controller.host,
                &mut self.controller.device,
                connection_handle,
            )?;

            self.controller.device.poll(&mut self.controller.host);
            for payload in self
                .controller
                .device
                .take_l2cap_on_handle(connection_handle, SMP_CID)
            {
                let pdu = SmpPdu::from_bytes(&payload)
                    .map_err(|error| Error::Remote(format!("invalid SMP PDU: {error}")))?;
                if matches!(pdu, SmpPdu::SecurityRequest { .. }) {
                    println!(
                        "[R2/BOND-MGR] peer Security Request observed while initiator session is active"
                    );
                    continue;
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

            if pairing.state() == ScPairingState::WaitEncryption {
                if !encryption_started {
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
                    println!("[R2/BOND-MGR] LE Start Encryption submitted");
                }

                if self
                    .controller
                    .device
                    .is_encrypted_on_handle(connection_handle)
                    && !marked_encrypted
                {
                    pairing.mark_encrypted().map_err(|error| {
                        Error::Remote(format!("failed to mark SC encrypted: {error}"))
                    })?;
                    marked_encrypted = true;
                }
            }

            flush_sc_outbound(
                &mut pairing,
                &mut self.controller.host,
                &mut self.controller.device,
                connection_handle,
            )?;

            if pairing.state() == ScPairingState::Failed {
                return Err(Error::Remote(format!(
                    "SMP pairing failed: {:?}",
                    pairing.failure()
                )));
            }
            if pairing.state() == ScPairingState::Complete {
                let keys = pairing.pairing_keys().ok_or_else(|| {
                    Error::Remote("SC pairing completed without keys".into())
                })?;
                println!(
                    "[R2/BOND-MGR] SMP complete after {:?}; encrypted={}",
                    started_at.elapsed(),
                    self.controller
                        .device
                        .is_encrypted_on_handle(connection_handle)
                );
                break keys;
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
                    "timed out provisional pairing on handle 0x{connection_handle:04X}; state={:?}; encrypted={}",
                    pairing.state(),
                    self.controller
                        .device
                        .is_encrypted_on_handle(connection_handle)
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
                            "[R2/BOND-MGR] waiting: {:?} state={:?} encrypted={}",
                            started_at.elapsed(),
                            pairing.state(),
                            self.controller
                                .device
                                .is_encrypted_on_handle(connection_handle)
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

        if !self
            .controller
            .device
            .is_encrypted_on_handle(connection_handle)
        {
            return Err(Error::Remote(
                "SMP completed without an encrypted LE ACL".into(),
            ));
        }

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
            .or_else(|| {
                (staged_all.len() == 1).then(|| staged_all[0].0.clone())
            })
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

        let staged_keys = staged.get(&identity).map_err(|error| {
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

fn flush_sc_outbound(
    pairing: &mut ScPairingSession,
    host: &mut bumble_host::LocalLink,
    device: &mut bumble_host::Device,
    connection_handle: u16,
) -> TransportResult<()> {
    for pdu in pairing.drain_outbound() {
        if !device.send_l2cap_on_handle(host, connection_handle, SMP_CID, &pdu.to_bytes()) {
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
