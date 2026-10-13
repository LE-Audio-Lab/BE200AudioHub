use std::time::{Duration, Instant};

use bumble::keys::{Key, KeyStore, PairingKeys};
use bumble_transport::{Error, ExternalHostActivity, Result as TransportResult};

use crate::{AttachedController, BondStore};

const WAIT_SLICE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondResumeReport {
    pub identity_address: String,
    pub connection_handle: u16,
    pub encrypted: bool,
}

impl AttachedController {
    /// Restore encryption on an established ACL using the persisted peer LTK.
    /// No SMP Pairing Request is sent and no persistent state is modified.
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
        let key = selected_encryption_key(&keys, true).ok_or_else(|| {
            Error::Remote(format!(
                "stored bond for {identity_address} has no usable LTK"
            ))
        })?;

        let long_term_key: [u8; 16] = key.value.as_slice().try_into().map_err(|_| {
            Error::Remote(format!(
                "stored LTK for {identity_address} is not 16 bytes"
            ))
        })?;
        let random_number: [u8; 8] = match key.rand.as_deref() {
            Some(value) => value.try_into().map_err(|_| {
                Error::Remote(format!(
                    "stored RAND for {identity_address} is not 8 bytes"
                ))
            })?,
            None => [0; 8],
        };

        println!(
            "[R2/SECURITY] restore stored-LTK encryption: identity={} handle=0x{:04X}",
            identity_address, connection_handle
        );
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
                println!(
                    "[R2/SECURITY] stored-LTK encryption confirmed: handle=0x{:04X}",
                    connection_handle
                );
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
                        "HCI transport ended while restoring stored bond".into(),
                    ));
                }
            }
        }
    }
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
