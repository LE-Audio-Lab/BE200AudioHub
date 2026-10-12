use std::time::{Duration, Instant};

use bumble::keys::{Key, KeyStore, MemoryKeyStore, PairingKeys};
use bumble::{Address, AddressType};
use bumble_crypto::{random_128, EccKey};
use bumble_smp::{
    AcceptAllDelegate, IdentityAddressType, PairingCapabilities, PairingConfig, PairingRole,
    ScPairingSession, ScPairingState, SmpPdu, SMP_CID,
};
use bumble_transport::{
    Error, ExternalHostActivity, Result as TransportResult,
};

use crate::{AttachedController, BondStore};

const WAIT_SLICE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairPersistReport {
    pub identity_address: String,
    pub address_type: u8,
    pub encrypted: bool,
    pub authenticated: bool,
    pub has_ltk: bool,
    pub has_irk: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondResumeReport {
    pub identity_address: String,
    pub connection_handle: u16,
    pub encrypted: bool,
}

impl AttachedController {
    /// Pair one already-connected LE peer using Secure Connections Just Works,
    /// persist the resulting bond immediately, then refresh the host-side RPA
    /// resolver from the on-disk key store.
    pub fn pair_and_persist(
        &mut self,
        connection_handle: u16,
        connection_local_address: &str,
        connection_local_address_type: u8,
        target_name: &str,
        expected_peer_identity: Option<&str>,
        store: &mut BondStore,
        timeout: Duration,
    ) -> TransportResult<PairPersistReport> {
        let local_address = Address::parse(
            connection_local_address,
            AddressType(connection_local_address_type),
        )
        .map_err(|error| Error::InvalidSpec(format!(
            "invalid connection-local address: {error}"
        )))?;
        let connection = self
            .device
            .le_connection(connection_handle)
            .cloned()
            .ok_or_else(|| Error::Remote(format!(
                "unknown LE connection handle 0x{connection_handle:04X}"
            )))?;
        if connection.role != 0 {
            return Err(Error::Remote(
                "R2.4 pairing currently requires the BE200 to be LE Central".into(),
            ));
        }
        let peer_address = connection.peer_address.clone();

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
            "[R2.4/SMP] config: SC={} MITM={} bonding={} CT2={} IO=NoInputNoOutput local_identity=public",
            config.secure_connections,
            config.mitm,
            config.bonding,
            config.ct2
        );
        println!(
            "[R2.4/SMP] key distribution: initiator=0x{:02X} responder=0x{:02X} max_key_size={}",
            config.capabilities.local_initiator_key_distribution.0,
            config.capabilities.local_responder_key_distribution.0,
            config.capabilities.maximum_encryption_key_size
        );

        let mut pairing = ScPairingSession::new(
            PairingRole::Initiator,
            config.clone(),
            Box::new(AcceptAllDelegate),
            local_address,
            peer_address,
            EccKey::generate(),
            random_128(),
        )
        .map_err(|error| Error::Remote(format!(
            "failed to create persistent-identity SC session: {error}"
        )))?;
        pairing
            .set_local_key_material(store.local_key_material()?)
            .map_err(|error| Error::Remote(format!(
                "failed to inject persistent local key material: {error}"
            )))?;

        println!(
            "[R2.4/SMP] session created: handle=0x{connection_handle:04X} role=Initiator local_link={} type={} encrypted={}",
            connection_local_address,
            connection_local_address_type,
            self.device.is_encrypted_on_handle(connection_handle)
        );
        println!(
            "[R2.4/SMP] persistent coordinator identity: public={} local-IRK=loaded",
            store.controller_identity()
        );
        pairing
            .start()
            .map_err(|error| Error::Remote(format!("failed to start SC pairing: {error}")))?;
        println!(
            "[R2.4/SMP] begin: Pairing Request queued; state={:?}",
            pairing.state()
        );

        let started_at = Instant::now();
        let deadline = started_at + timeout;
        let mut last_state = pairing.state();
        let mut last_encrypted = self.device.is_encrypted_on_handle(connection_handle);
        let mut encryption_started = false;
        let mut marked_encrypted = false;
        let mut idle_slices: u32 = 0;

        let keys = loop {
            flush_sc_outbound(
                &mut pairing,
                &mut self.host,
                &mut self.device,
                connection_handle,
            )?;

            self.device.poll(&mut self.host);
            for payload in self.device.take_l2cap_on_handle(connection_handle, SMP_CID) {
                let pdu = SmpPdu::from_bytes(&payload)
                    .map_err(|error| Error::Remote(format!("invalid SMP PDU: {error}")))?;
                if matches!(pdu, SmpPdu::SecurityRequest { .. }) {
                    println!("[R2.4/SMP] peer Security Request observed while local pairing already active; continuing current initiator session");
                    continue;
                }
                pairing
                    .process(pdu)
                    .map_err(|error| Error::Remote(format!("SC pairing process failed: {error}")))?;
            }

            for request in self
                .device
                .take_long_term_key_requests_on_handle(connection_handle)
            {
                let Some(key) = pairing.ltk() else {
                    self.device
                        .reject_long_term_key_request(&mut self.host, request.connection_handle);
                    return Err(Error::Remote(format!(
                        "controller requested LTK before SC session produced one on handle 0x{:04X}",
                        request.connection_handle
                    )));
                };
                if !self.device.reply_long_term_key_request(
                    &mut self.host,
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
                    let key = pairing
                        .ltk()
                        .ok_or_else(|| Error::Remote(
                            "SC pairing reached encryption without an LTK".into()
                        ))?;
                    if !self.device.enable_encryption_on_handle(
                        &mut self.host,
                        connection_handle,
                        key,
                    ) {
                        return Err(Error::Remote(format!(
                            "failed to start encryption on handle 0x{connection_handle:04X}"
                        )));
                    }
                    encryption_started = true;
                }

                if self.device.is_encrypted_on_handle(connection_handle) && !marked_encrypted {
                    pairing
                        .mark_encrypted()
                        .map_err(|error| Error::Remote(format!(
                            "failed to mark SC session encrypted: {error}"
                        )))?;
                    marked_encrypted = true;
                }
            }

            flush_sc_outbound(
                &mut pairing,
                &mut self.host,
                &mut self.device,
                connection_handle,
            )?;

            if pairing.state() == ScPairingState::Failed {
                return Err(Error::Remote(format!(
                    "SMP pairing failed: {:?}",
                    pairing.failure()
                )));
            }
            if pairing.state() == ScPairingState::Complete {
                let keys = pairing
                    .pairing_keys()
                    .ok_or_else(|| Error::Remote(
                        "SC pairing completed without keys".into()
                    ))?;
                println!(
                    "[R2.4/SMP] complete after {:?}: state={:?} encrypted={}",
                    started_at.elapsed(),
                    pairing.state(),
                    self.device.is_encrypted_on_handle(connection_handle)
                );
                break keys;
            }

            let state = pairing.state();
            if state != last_state {
                println!(
                    "[R2.4/SMP] state: {:?} -> {:?} at {:?}",
                    last_state,
                    state,
                    started_at.elapsed()
                );
                last_state = state;
            }

            let encrypted = self.device.is_encrypted_on_handle(connection_handle);
            if encrypted != last_encrypted {
                println!(
                    "[R2.4/SMP] ACL encryption: {} -> {} at {:?}",
                    last_encrypted,
                    encrypted,
                    started_at.elapsed()
                );
                last_encrypted = encrypted;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Remote(format!(
                    "timed out pairing LE connection 0x{connection_handle:04X}; final_state={:?}; encrypted={}; host_state={:?}; elapsed={:?}",
                    pairing.state(),
                    self.device.is_encrypted_on_handle(connection_handle),
                    self.host.state(),
                    started_at.elapsed()
                )));
            }

            match self.host.wait_for_device_activity(
                &mut self.device,
                remaining.min(WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet => {
                    idle_slices = 0;
                    println!(
                        "[R2.4/SMP] HCI activity: packet at {:?}; state={:?}",
                        started_at.elapsed(),
                        pairing.state()
                    );
                }
                ExternalHostActivity::Timeout => {
                    idle_slices = idle_slices.saturating_add(1);
                    if idle_slices == 1 || idle_slices % 4 == 0 {
                        println!(
                            "[R2.4/SMP] waiting: {:?} elapsed, state={:?}, encrypted={}",
                            started_at.elapsed(),
                            pairing.state(),
                            self.device.is_encrypted_on_handle(connection_handle)
                        );
                    }
                }
                ExternalHostActivity::Ended => {
                    return Err(Error::Remote(format!(
                        "transport ended while pairing LE connection 0x{connection_handle:04X}; state={:?}; elapsed={:?}",
                        pairing.state(),
                        started_at.elapsed()
                    )));
                }
            }
        };
        println!(
            "[R2.4/SMP] pairing keys ready: LTK={} IRK={} address_type={:?}",
            selected_encryption_key(&keys, true).is_some(),
            keys.irk.is_some(),
            keys.address_type.map(|value| value.0)
        );

        if !self.device.is_encrypted_on_handle(connection_handle) {
            return Err(Error::Remote(
                "SMP pairing completed without an encrypted LE ACL".into(),
            ));
        }

        println!("[R2.4/SMP] staging bond in memory for identity validation...");
        let mut staged = MemoryKeyStore::new();
        let stored = pairing
            .store_bond(&mut staged)
            .map_err(|error| Error::Remote(format!(
                "failed to stage SC bond: {error}"
            )))?;
        println!("[R2.4/SMP] staged store_bond returned {stored}");
        if !stored {
            return Err(Error::Remote(
                "peer completed pairing without a persistent bond".into(),
            ));
        }

        let staged_all = staged
            .get_all()
            .map_err(|error| Error::Remote(format!(
                "failed to inspect staged bond: {error}"
            )))?;
        let identity = staged_all
            .iter()
            .find(|(_, staged_keys)| *staged_keys == keys)
            .map(|(name, _)| name.clone())
            .or_else(|| {
                if staged_all.len() == 1 {
                    Some(staged_all[0].0.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| Error::Remote(
                "staged pairing keys did not expose one unambiguous peer identity".into(),
            ))?;

        if let Some(expected) = expected_peer_identity {
            if !identity.eq_ignore_ascii_case(expected) {
                return Err(Error::Remote(format!(
                    "paired peer identity {identity} did not match expected coordinated-set member {expected}; staged keys were discarded and persistent bond store was NOT modified"
                )));
            }
            println!(
                "[R2.4/SMP] staged identity validation: PASS actual={} expected={}",
                identity, expected
            );
        }

        let staged_keys = staged
            .get(&identity)
            .map_err(|error| Error::Remote(format!(
                "failed to read staged keys for {identity}: {error}"
            )))?
            .ok_or_else(|| Error::Remote(format!(
                "staged bond for {identity} disappeared before commit"
            )))?;

        println!("[R2.4/SMP] committing validated bond into persistent key store...");
        store
            .key_store_mut()
            .update(&identity, staged_keys)
            .map_err(|error| Error::Remote(format!(
                "failed to commit validated bond for {identity}: {error}"
            )))?;
        println!("[R2.4/SMP] persistent bond commit: PASS identity={identity}");

        let address_type = keys
            .address_type
            .map(|value| value.0)
            .unwrap_or(AddressType::RANDOM_DEVICE.0);

        let encryption_key = selected_encryption_key(&keys, true);
        let has_ltk = encryption_key.is_some();
        let has_irk = keys.irk.is_some();
        if !has_ltk || !has_irk {
            return Err(Error::Remote(format!(
                "bond keys were persisted for {identity}, but durable RPA reconnect requires both LTK and IRK (LTK={has_ltk}, IRK={has_irk}); set membership was not recorded and the bond was not deleted"
            )));
        }

        let authenticated = encryption_key
            .as_ref()
            .is_some_and(|key| key.authenticated);

        store.record_member(target_name, &identity, address_type)?;

        self.device.set_key_store(store.clone_key_store());
        self.device
            .refresh_address_resolver()
            .map_err(|error| Error::Remote(format!(
                "failed to refresh RPA resolver after pairing: {error}"
            )))?;

        Ok(PairPersistReport {
            identity_address: identity,
            address_type,
            encrypted: true,
            authenticated,
            has_ltk,
            has_irk,
        })
    }

    /// Re-enable encryption on an already-established ACL using only the
    /// persisted bond. No SMP Pairing Request is sent on this path.
    pub fn resume_bonded_encryption(
        &mut self,
        connection_handle: u16,
        identity_address: &str,
        store: &BondStore,
        timeout: Duration,
    ) -> TransportResult<BondResumeReport> {
        if self.device.is_encrypted_on_handle(connection_handle) {
            return Ok(BondResumeReport {
                identity_address: identity_address.to_owned(),
                connection_handle,
                encrypted: true,
            });
        }

        let keys = store
            .key_store()
            .get(identity_address)
            .map_err(|error| Error::Remote(format!(
                "failed to read stored bond for {identity_address}: {error}"
            )))?
            .ok_or_else(|| Error::Remote(format!(
                "no stored bond exists for identity {identity_address}"
            )))?;

        let key = selected_encryption_key(&keys, true)
            .ok_or_else(|| Error::Remote(format!(
                "stored bond for {identity_address} has no usable LTK"
            )))?;

        let long_term_key: [u8; 16] = key
            .value
            .as_slice()
            .try_into()
            .map_err(|_| Error::Remote(format!(
                "stored LTK for {identity_address} is not 16 bytes"
            )))?;

        let random_number: [u8; 8] = match key.rand.as_deref() {
            Some(value) => value.try_into().map_err(|_| Error::Remote(format!(
                "stored RAND for {identity_address} is not 8 bytes"
            )))?,
            None => [0; 8],
        };

        if !self.device.enable_encryption_with_parameters_on_handle(
            &mut self.host,
            connection_handle,
            long_term_key,
            key.ediv.unwrap_or(0),
            random_number,
        ) {
            return Err(Error::Remote(format!(
                "failed to submit LE encryption for handle 0x{connection_handle:04X}"
            )));
        }

        let deadline = Instant::now() + timeout;
        loop {
            self.device.poll(&mut self.host);

            if self.device.is_encrypted_on_handle(connection_handle) {
                return Ok(BondResumeReport {
                    identity_address: identity_address.to_owned(),
                    connection_handle,
                    encrypted: true,
                });
            }

            if !self.device.is_connected_on_handle(connection_handle) {
                return Err(Error::Remote(format!(
                    "LE ACL 0x{connection_handle:04X} disconnected before stored-bond encryption completed"
                )));
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Remote(format!(
                    "timed out waiting for encryption on handle 0x{connection_handle:04X}"
                )));
            }

            match self.host.wait_for_device_activity(
                &mut self.device,
                remaining.min(WAIT_SLICE),
            )? {
                ExternalHostActivity::Packet | ExternalHostActivity::Timeout => {}
                ExternalHostActivity::Ended => {
                    return Err(Error::Remote(
                        "Bumble HCI transport ended while resuming stored bond".into(),
                    ))
                }
            }
        }
    }
}

fn flush_sc_outbound(
    pairing: &mut ScPairingSession,
    host: &mut bumble_host::LocalLink,
    device: &mut bumble_host::Device,
    connection_handle: u16,
) -> TransportResult<()> {
    for pdu in pairing.drain_outbound() {
        if !device.send_l2cap_on_handle(
            host,
            connection_handle,
            SMP_CID,
            &pdu.to_bytes(),
        ) {
            return Err(Error::Remote(format!(
                "failed to send SMP PDU on handle 0x{connection_handle:04X}"
            )));
        }
    }
    Ok(())
}

fn selected_encryption_key(
    keys: &PairingKeys,
    local_is_central: bool,
) -> Option<&Key> {
    keys.ltk.as_ref().or_else(|| {
        if local_is_central {
            keys.ltk_central.as_ref()
        } else {
            keys.ltk_peripheral.as_ref()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_connections_key_is_preferred() {
        let keys = PairingKeys {
            ltk: Some(Key {
                value: vec![1; 16],
                ..Key::default()
            }),
            ltk_central: Some(Key {
                value: vec![2; 16],
                ..Key::default()
            }),
            ..PairingKeys::default()
        };

        assert_eq!(
            selected_encryption_key(&keys, true).unwrap().value,
            vec![1; 16]
        );
    }
}