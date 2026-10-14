use std::time::Duration;

use bumble_gatt::GattClient;
use bumble_profiles::csip::{sih, CoordinatedSetIdentificationProxy};
use bumble_transport::{Error, ExternalAttTransport, Result as TransportResult};

use crate::{AttachedController, BondStore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsisMemberReport {
    pub sirk: [u8; 16],
    pub set_size: Option<u8>,
    pub rank: Option<u8>,
}

impl AttachedController {
    /// Read CSIS metadata using an already committed peer bond.
    pub fn read_csis_member(
        &mut self,
        connection_handle: u16,
        identity_address: &str,
        store: &BondStore,
        timeout: Duration,
    ) -> TransportResult<CsisMemberReport> {
        let ltk = store.encryption_key_for_identity(identity_address)?;
        self.read_csis_member_with_ltk(connection_handle, ltk, timeout)
    }

    /// Read CSIS metadata using the LTK from a provisional bond transaction.
    ///
    /// This is the verifier path used before the peer keys exist in the
    /// persistent BondStore. The ACL must already be encrypted by the SMP
    /// transaction that produced this LTK.
    pub fn read_csis_member_with_ltk(
        &mut self,
        connection_handle: u16,
        ltk: [u8; 16],
        timeout: Duration,
    ) -> TransportResult<CsisMemberReport> {
        if !self.device.is_connected_on_handle(connection_handle) {
            return Err(Error::Remote(format!(
                "cannot read CSIS: LE ACL 0x{connection_handle:04X} is not connected"
            )));
        }
        if !self.device.is_encrypted_on_handle(connection_handle) {
            return Err(Error::Remote(format!(
                "cannot read CSIS: LE ACL 0x{connection_handle:04X} is not encrypted"
            )));
        }

        let mut transport =
            ExternalAttTransport::new(&mut self.host, &mut self.device, connection_handle, timeout)?;
        let mut client = GattClient::new();

        let proxy = CoordinatedSetIdentificationProxy::discover(&mut client, &mut transport)
            .map_err(|error| Error::Remote(format!("CSIS discovery failed: {error}")))?
            .ok_or_else(|| Error::Remote("peer does not expose a CSIS service".into()))?;

        let (_, sirk) = proxy
            .read_set_identity_resolving_key(&mut client, &mut transport, Some(ltk))
            .map_err(|error| Error::Remote(format!("CSIS SIRK read failed: {error}")))?;

        let set_size = match proxy.coordinated_set_size.as_ref() {
            Some(characteristic) => Some(read_u8(
                &mut client,
                &mut transport,
                characteristic.handle,
                "CSIS Set Size",
            )?),
            None => None,
        };

        let rank = match proxy.set_member_rank.as_ref() {
            Some(characteristic) => Some(read_u8(
                &mut client,
                &mut transport,
                characteristic.handle,
                "CSIS Member Rank",
            )?),
            None => None,
        };

        Ok(CsisMemberReport {
            sirk,
            set_size,
            rank,
        })
    }
}

/// Verify one six-byte Resolvable Set Identifier against a stored SIRK.
pub fn rsi_matches_sirk(sirk: &[u8; 16], rsi: &[u8; 6]) -> TransportResult<bool> {
    let hash = sih(sirk, &rsi[3..])
        .map_err(|error| Error::Remote(format!("CSIS RSI hash failed: {error}")))?;
    Ok(hash.as_slice() == &rsi[..3])
}

fn read_u8(
    client: &mut GattClient,
    transport: &mut ExternalAttTransport<'_>,
    handle: u16,
    label: &str,
) -> TransportResult<u8> {
    let value = client
        .read_value(transport, handle, false)
        .map_err(|error| Error::Remote(format!("{label} read failed: {error}")))?;
    match value.as_slice() {
        [value] => Ok(*value),
        _ => Err(Error::Remote(format!(
            "{label} returned {} bytes; expected 1",
            value.len()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bumble_profiles::csip::rsi_with_prand;

    #[test]
    fn rsi_matches_its_sirk() {
        let sirk = [0x5A; 16];
        let rsi = rsi_with_prand(&sirk, [0x01, 0x02, 0x43]).unwrap();
        assert!(rsi_matches_sirk(&sirk, &rsi).unwrap());
        assert!(!rsi_matches_sirk(&[0xA5; 16], &rsi).unwrap());
    }
}
