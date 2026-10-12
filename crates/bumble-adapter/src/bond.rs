use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bumble::keys::{JsonKeyStore, KeyStore};
use bumble::{Address, AddressType};
use bumble_hci::{Command, ReturnParameters};
use bumble_smp::AddressResolver;
use bumble_transport::{Error, Result as TransportResult};
use serde::{Deserialize, Serialize};

use crate::{AttachedController, ScanObservation};

const SET_DB_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerIdentity {
    pub public_address: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondMemberSummary {
    pub identity_address: String,
    pub address_type: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondSetSummary {
    pub target_name: String,
    pub members: Vec<BondMemberSummary>,
    pub set_size: Option<u8>,
    pub sirk_hex: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BondStore {
    controller_identity: String,
    root: PathBuf,
    key_store: JsonKeyStore,
    set_db: BondSetDb,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BondSetDb {
    schema_version: u32,
    controller_identity: String,
    #[serde(default)]
    sets: BTreeMap<String, BondSetRecord>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct BondSetRecord {
    #[serde(default)]
    members: Vec<BondMemberRecord>,
    #[serde(default)]
    set_size: Option<u8>,
    #[serde(default)]
    sirk_hex: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BondMemberRecord {
    identity_address: String,
    address_type: u8,
}

impl BondStore {
    pub fn open_for_controller(identity: &ControllerIdentity) -> TransportResult<Self> {
        let root = bonding_root()?;
        std::fs::create_dir_all(&root)
            .map_err(|error| Error::Remote(format!("failed to create bond directory: {error}")))?;

        let identity_file = root.join("controller-identity.txt");
        enforce_controller_identity(&identity_file, &identity.public_address)?;

        let set_db_path = root.join("bond-sets.json");
        let set_db = load_set_db(&set_db_path, &identity.public_address)?;

        let key_store = JsonKeyStore::new(
            Some(&identity.public_address),
            root.join("pairing-keys.json"),
        );

        Ok(Self {
            controller_identity: identity.public_address.clone(),
            root,
            key_store,
            set_db,
        })
    }

    pub fn controller_identity(&self) -> &str {
        &self.controller_identity
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn pairing_keys_path(&self) -> PathBuf {
        self.root.join("pairing-keys.json")
    }

    pub fn set_db_path(&self) -> PathBuf {
        self.root.join("bond-sets.json")
    }

    pub fn pairing_key_count(&self) -> TransportResult<usize> {
        self.key_store
            .get_all()
            .map(|entries| entries.len())
            .map_err(|error| Error::Remote(format!("failed to read pairing key store: {error}")))
    }

    pub fn set_summary(&self, target_name: &str) -> BondSetSummary {
        let record = self.set_db.sets.get(target_name);
        BondSetSummary {
            target_name: target_name.to_owned(),
            members: record
                .map(|record| {
                    record
                        .members
                        .iter()
                        .map(|member| BondMemberSummary {
                            identity_address: member.identity_address.clone(),
                            address_type: member.address_type,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            set_size: record.and_then(|record| record.set_size),
            sirk_hex: record.and_then(|record| record.sirk_hex.clone()),
        }
    }

    pub fn record_member(
        &mut self,
        target_name: &str,
        identity_address: &str,
        address_type: u8,
    ) -> TransportResult<()> {
        let record = self.set_db.sets.entry(target_name.to_owned()).or_default();
        if !record
            .members
            .iter()
            .any(|member| member.identity_address.eq_ignore_ascii_case(identity_address))
        {
            record.members.push(BondMemberRecord {
                identity_address: identity_address.to_owned(),
                address_type,
            });
        }
        self.save_set_db()
    }

    pub fn adopt_existing_bond(
        &mut self,
        target_name: &str,
        identity_address: &str,
    ) -> TransportResult<bool> {
        let Some(keys) = self
            .key_store
            .get(identity_address)
            .map_err(|error| Error::Remote(format!(
                "failed to inspect existing bond {identity_address}: {error}"
            )))?
        else {
            return Ok(false);
        };

        let address_type = keys
            .address_type
            .map(|value| value.0)
            .unwrap_or(AddressType::RANDOM_DEVICE.0);
        self.record_member(target_name, identity_address, address_type)?;
        Ok(true)
    }

    pub fn update_csis(
        &mut self,
        target_name: &str,
        set_size: Option<u8>,
        sirk_hex: Option<String>,
    ) -> TransportResult<()> {
        let record = self.set_db.sets.entry(target_name.to_owned()).or_default();
        if let Some(set_size) = set_size {
            record.set_size = Some(set_size);
        }
        if let Some(sirk_hex) = sirk_hex {
            match &record.sirk_hex {
                Some(existing) if !existing.eq_ignore_ascii_case(&sirk_hex) => {
                    return Err(Error::Remote(format!(
                        "CSIS SIRK mismatch for {target_name:?}; refusing to merge distinct sets"
                    )));
                }
                _ => record.sirk_hex = Some(sirk_hex),
            }
        }
        self.save_set_db()
    }

    pub fn resolve_observation_identity(
        &self,
        observation: &ScanObservation,
    ) -> TransportResult<Option<String>> {
        let address = Address::parse(
            &observation.address,
            AddressType(observation.address_type),
        )
        .map_err(|error| Error::InvalidSpec(format!("invalid scan address: {error}")))?;

        if self
            .key_store
            .get(&address.to_string(false))
            .map_err(|error| Error::Remote(format!("failed to query bond store: {error}")))?
            .is_some()
        {
            return Ok(Some(address.to_string(false)));
        }

        let resolving_keys = self
            .key_store
            .get_resolving_keys()
            .map_err(|error| Error::Remote(format!("failed to load IRK resolver: {error}")))?;
        let resolver = AddressResolver::new(resolving_keys);
        Ok(resolver.resolve(&address).map(|identity| identity.to_string(false)))
    }

    pub fn is_member_of(&self, target_name: &str, identity_address: &str) -> bool {
        self.set_db
            .sets
            .get(target_name)
            .is_some_and(|record| {
                record
                    .members
                    .iter()
                    .any(|member| member.identity_address.eq_ignore_ascii_case(identity_address))
            })
    }

    pub(crate) fn key_store(&self) -> &JsonKeyStore {
        &self.key_store
    }

    pub(crate) fn key_store_mut(&mut self) -> &mut JsonKeyStore {
        &mut self.key_store
    }

    pub(crate) fn clone_key_store(&self) -> JsonKeyStore {
        self.key_store.clone()
    }

    fn save_set_db(&self) -> TransportResult<()> {
        let bytes = serde_json::to_vec_pretty(&self.set_db)
            .map_err(|error| Error::Remote(format!("failed to encode bond set database: {error}")))?;
        atomic_write(&self.set_db_path(), &bytes)
    }
}

impl AttachedController {
    pub fn read_controller_identity(
        &mut self,
        timeout: Duration,
    ) -> TransportResult<ControllerIdentity> {
        let response = self.host.send_command(Command::ReadBdAddr, timeout)?;
        if response.status() != Some(0) {
            return Err(Error::Remote(format!(
                "Read BD_ADDR failed with status {:?}",
                response.status()
            )));
        }
        let Some(ReturnParameters::ReadBdAddr { bd_addr, .. }) = response.return_parameters() else {
            return Err(Error::Remote(
                "Read BD_ADDR returned unexpected parameters".into(),
            ));
        };
        Ok(ControllerIdentity {
            public_address: bd_addr.to_string(false),
        })
    }

    pub fn install_bond_store(&mut self, store: &BondStore) -> TransportResult<usize> {
        self.device.set_key_store(store.clone_key_store());
        self.device
            .refresh_address_resolver()
            .map_err(|error| Error::Remote(format!("failed to refresh address resolver: {error}")))
    }
}

fn bonding_root() -> TransportResult<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
        Error::Remote("LOCALAPPDATA is unavailable; cannot persist Bluetooth bonds".into())
    })?;
    Ok(PathBuf::from(local).join("BE200AudioHub").join("bonding"))
}

fn enforce_controller_identity(path: &Path, identity: &str) -> TransportResult<()> {
    match std::fs::read_to_string(path) {
        Ok(existing) => {
            let existing = existing.trim();
            if !existing.eq_ignore_ascii_case(identity) {
                return Err(Error::Remote(format!(
                    "controller identity changed from {existing} to {identity}; refusing to reuse old bonds"
                )));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            atomic_write(path, format!("{identity}\n").as_bytes())
        }
        Err(error) => Err(Error::Remote(format!(
            "failed to read controller identity guard: {error}"
        ))),
    }
}

fn load_set_db(path: &Path, controller_identity: &str) -> TransportResult<BondSetDb> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let db: BondSetDb = serde_json::from_slice(&bytes)
                .map_err(|error| Error::Remote(format!("invalid bond set database: {error}")))?;
            if db.schema_version != SET_DB_SCHEMA_VERSION {
                return Err(Error::Remote(format!(
                    "unsupported bond set schema {}; expected {}",
                    db.schema_version, SET_DB_SCHEMA_VERSION
                )));
            }
            if !db
                .controller_identity
                .eq_ignore_ascii_case(controller_identity)
            {
                return Err(Error::Remote(
                    "bond set database belongs to a different controller identity".into(),
                ));
            }
            Ok(db)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BondSetDb {
            schema_version: SET_DB_SCHEMA_VERSION,
            controller_identity: controller_identity.to_owned(),
            sets: BTreeMap::new(),
        }),
        Err(error) => Err(Error::Remote(format!(
            "failed to read bond set database: {error}"
        ))),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> TransportResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::Remote(format!("failed to create {}: {error}", parent.display())))?;
    }
    let mut temp_name: OsString = path.as_os_str().to_os_string();
    temp_name.push(".tmp");
    let temp = PathBuf::from(temp_name);
    std::fs::write(&temp, bytes)
        .map_err(|error| Error::Remote(format!("failed to write {}: {error}", temp.display())))?;
    std::fs::rename(&temp, path)
        .map_err(|error| Error::Remote(format!("failed to replace {}: {error}", path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_summary_is_stable() {
        let db = BondSetDb {
            schema_version: SET_DB_SCHEMA_VERSION,
            controller_identity: "11:22:33:44:55:66".into(),
            sets: BTreeMap::new(),
        };
        assert!(db.sets.is_empty());
    }
}