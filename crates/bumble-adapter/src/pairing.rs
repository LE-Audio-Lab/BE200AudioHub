use std::time::{Duration, Instant};

use bumble::keys::{Key, KeyStore, PairingKeys};
use bumble::{Address, AddressType};
use bumble_smp::{
    IdentityAddressType, PairingCapabilities, PairingConfig,
};
use bumble_transport::{
    Error, ExternalHostActivity, LePairingSession, Result as TransportResult,
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
        controller_public_address: &str,
        target_name: &str,
        store: &mut BondStore,
        timeout: Duration,
    ) -> TransportResult<PairPersistReport> {
        let local_address = Address::parse(
            controller_public_address,
            AddressType::PUBLIC_DEVICE,
        )
        .map_err(|error| Error::InvalidSpec(format!(
            "invalid controller public address: {error}"
        )))?;

        let config = PairingConfig {
            secure_connections: true,
            ct2: false,
            mitm: false,
            bonding: true,
            capabilities: PairingCapabilities::default(),
            identity_address_type: Some(IdentityAddressType::Public),
            oob: None,
        };

        let before = store
            .key_store()
            .get_all()
            .map_err(|error| Error::Remote(format!(
                "failed to snapshot bond store before pairing: {error}"
            )))?;

        let mut pairing = LePairingSession::accept_all(
            &self.device,
            connection_handle,
            local_address,
            config,
        )?;

        let keys = pairing.pair(
            &mut self.host,
            &mut self.device,
            timeout,
        )?;

        if !self.device.is_encrypted_on_handle(connection_handle) {
            return Err(Error::Remote(
                "SMP pairing completed without an encrypted LE ACL".into(),
            ));
        }

        let stored = pairing
            .store_bond(store.key_store_mut())?;
        if !stored {
            return Err(Error::Remote(
                "peer completed pairing without a persistent bond".into(),
            ));
        }

        let after = store
            .key_store()
            .get_all()
            .map_err(|error| Error::Remote(format!(
                "failed to reload bond store after pairing: {error}"
            )))?;

        let identity = find_stored_identity(&before, &after, &keys)
            .ok_or_else(|| Error::Remote(
                "pairing keys were persisted but the peer identity could not be identified".into(),
            ))?;

        let address_type = keys
            .address_type
            .map(|value| value.0)
            .unwrap_or(AddressType::RANDOM_DEVICE.0);

        store.record_member(target_name, &identity, address_type)?;

        self.device.set_key_store(store.clone_key_store());
        self.device
            .refresh_address_resolver()
            .map_err(|error| Error::Remote(format!(
                "failed to refresh RPA resolver after pairing: {error}"
            )))?;

        let encryption_key = selected_encryption_key(&keys, true);
        let authenticated = encryption_key
            .as_ref()
            .is_some_and(|key| key.authenticated);

        Ok(PairPersistReport {
            identity_address: identity,
            address_type,
            encrypted: true,
            authenticated,
            has_ltk: encryption_key.is_some(),
            has_irk: keys.irk.is_some(),
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

fn find_stored_identity(
    before: &[(String, PairingKeys)],
    after: &[(String, PairingKeys)],
    keys: &PairingKeys,
) -> Option<String> {
    let new_names = after
        .iter()
        .filter(|(name, _)| !before.iter().any(|(old, _)| old == name))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    if new_names.len() == 1 {
        return new_names.into_iter().next();
    }

    after
        .iter()
        .find(|(_, stored)| *stored == *keys)
        .map(|(name, _)| name.clone())
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