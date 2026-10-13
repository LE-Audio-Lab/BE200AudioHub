use std::time::Duration;

use bumble::keys::{JsonKeyStore, KeyStore};
use bumble::{Address, AddressType};
use bumble_hci::{Command, ReturnParameters};
use bumble_transport::{Error, Result as TransportResult};

use crate::{AttachedController, BondMemberSummary, BondStore};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_RPA_TIMEOUT_SECONDS: u16 = 900;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondedPeerIdentity {
    pub identity_address: String,
    pub address_type: u8,
    pub peer_irk: [u8; 16],
}

impl BondStore {
    pub fn bonded_peer_identity(
        &self,
        member: &BondMemberSummary,
    ) -> TransportResult<BondedPeerIdentity> {
        let key_store = JsonKeyStore::new(
            Some(self.controller_identity()),
            self.pairing_keys_path(),
        );
        let keys = key_store
            .get(&member.identity_address)
            .map_err(|error| Error::Remote(format!(
                "failed to read stored keys for {}: {error}",
                member.identity_address
            )))?
            .ok_or_else(|| Error::Remote(format!(
                "no stored keys exist for bonded identity {}",
                member.identity_address
            )))?;

        let address_type = keys
            .address_type
            .map(|value| value.0)
            .unwrap_or(member.address_type);
        if !matches!(address_type, 0 | 1) {
            return Err(Error::InvalidSpec(format!(
                "bonded identity {} has unsupported address type {}",
                member.identity_address, address_type
            )));
        }

        let irk = keys.irk.ok_or_else(|| Error::Remote(format!(
            "bonded identity {} has no peer IRK",
            member.identity_address
        )))?;
        let peer_irk: [u8; 16] = irk.value.as_slice().try_into().map_err(|_| {
            Error::Remote(format!(
                "peer IRK for {} is not 16 bytes",
                member.identity_address
            ))
        })?;

        Address::parse(
            &member.identity_address,
            AddressType(address_type),
        )
        .map_err(|error| Error::InvalidSpec(format!(
            "invalid bonded identity address {}: {error}",
            member.identity_address
        )))?;

        Ok(BondedPeerIdentity {
            identity_address: member.identity_address.clone(),
            address_type,
            peer_irk,
        })
    }
}

impl AttachedController {
    /// Install all bonded set members in the controller resolving list.
    ///
    /// R3 still performs host-side IRK resolution for observability, but ACL
    /// initiation uses stable identity addresses in the Filter Accept List.
    /// The controller then resolves the currently advertised RPA, eliminating
    /// the stale-RPA race between a finite scan and connection initiation.
    pub fn configure_bonded_identity_resolution(
        &mut self,
        store: &BondStore,
        members: &[BondMemberSummary],
    ) -> TransportResult<Vec<BondedPeerIdentity>> {
        if self.device.is_le_connecting() {
            return Err(Error::Remote(
                "cannot configure resolving list while an LE initiation procedure is active".into(),
            ));
        }
        if members.is_empty() {
            return Err(Error::InvalidSpec(
                "cannot configure resolving list for an empty bonded set".into(),
            ));
        }

        let local_irk = store.local_irk()?;
        let peers = members
            .iter()
            .map(|member| store.bonded_peer_identity(member))
            .collect::<TransportResult<Vec<_>>>()?;

        self.successful_privacy_command(Command::LeSetAddressResolutionEnable {
            address_resolution_enable: 0,
        })?;
        self.successful_privacy_command(Command::LeClearResolvingList)?;

        let response = self
            .host
            .send_command(Command::LeReadResolvingListSize, COMMAND_TIMEOUT)?;
        if response.status() != Some(0) {
            return Err(Error::Remote(format!(
                "LE Read Resolving List Size failed with status {:?}",
                response.status()
            )));
        }
        let capacity = match response.return_parameters() {
            Some(ReturnParameters::LeReadResolvingListSize {
                resolving_list_size,
                ..
            }) => *resolving_list_size,
            other => {
                return Err(Error::Remote(format!(
                    "LE Read Resolving List Size returned unexpected parameters: {other:?}"
                )))
            }
        };
        if usize::from(capacity) < peers.len() {
            return Err(Error::Remote(format!(
                "controller resolving-list capacity {} is smaller than bonded set size {}",
                capacity,
                peers.len()
            )));
        }

        println!(
            "[R3/PRIVACY] configure controller resolving list: capacity={} members={}",
            capacity,
            peers.len()
        );
        for peer in &peers {
            let identity = Address::parse(
                &peer.identity_address,
                AddressType(peer.address_type),
            )
            .map_err(|error| Error::InvalidSpec(format!(
                "invalid bonded identity address {}: {error}",
                peer.identity_address
            )))?;
            self.successful_privacy_command(Command::LeAddDeviceToResolvingList {
                peer_identity_address_type: peer.address_type,
                peer_identity_address: identity,
                peer_irk: peer.peer_irk,
                local_irk,
            })?;
            println!(
                "[R3/PRIVACY] resolving-list add: identity={} type={} PASS",
                peer.identity_address, peer.address_type
            );
        }

        self.successful_privacy_command(Command::LeSetResolvablePrivateAddressTimeout {
            rpa_timeout: DEFAULT_RPA_TIMEOUT_SECONDS,
        })?;
        self.successful_privacy_command(Command::LeSetAddressResolutionEnable {
            address_resolution_enable: 1,
        })?;
        println!(
            "[R3/PRIVACY] controller address resolution ENABLED; FAL may use stable identity addresses"
        );
        Ok(peers)
    }

    pub fn clear_bonded_identity_resolution(&mut self) -> TransportResult<()> {
        self.successful_privacy_command(Command::LeSetAddressResolutionEnable {
            address_resolution_enable: 0,
        })?;
        self.successful_privacy_command(Command::LeClearResolvingList)?;
        println!("[R3/PRIVACY] controller resolving list cleared");
        Ok(())
    }

    fn successful_privacy_command(&mut self, command: Command) -> TransportResult<()> {
        let opcode = command.op_code();
        let response = self.host.send_command(command, COMMAND_TIMEOUT)?;
        if response.status() == Some(0) {
            Ok(())
        } else {
            Err(Error::Remote(format!(
                "HCI privacy command {opcode:#06x} failed with status {:?}",
                response.status()
            )))
        }
    }
}
