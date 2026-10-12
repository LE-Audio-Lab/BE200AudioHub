use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bumble::keys::{JsonKeyStore, KeyStore};
use bumble::{Address, AddressType};
use bumble_hci::{Command, ReturnParameters};
use bumble_smp::{AddressResolver, LocalKeyMaterial};
use bumble_transport::{Error, Result as TransportResult};
use serde::{Deserialize, Serialize};

use crate::{AttachedController, ScanObservation};

const SET_DB_SCHEMA_VERSION: u32 = 1;
const LOCAL_IDENTITY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerIdentity {
    pub public_address: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondMemberSummary {
    pub identity_address: String,
    pub address_type: u8,
    pub rank: Option<u8>,
    pub csis_verified: bool,
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
    local_keys: Option<LocalKeyMaterial>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LocalIdentityRecord {
    schema_version: u32,
    controller_identity: String,
    ltk_hex: String,
    ediv: u16,
    rand_hex: String,
    irk_hex: String,
    csrk_hex: String,
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
    #[serde(default)]
    rank: Option<u8>,
    #[serde(default)]
    csis_verified: bool,
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

        let has_existing_bonds = !key_store
            .get_all()
            .map_err(|error| Error::Remote(format!(
                "failed to inspect pairing key store before local-identity load: {error}"
            )))?
            .is_empty()
            || set_db.sets.values().any(|record| !record.members.is_empty());

        let local_keys = load_or_create_local_identity(
            &root.join("local-identity.json"),
            &identity.public_address,
            has_existing_bonds,
        )?;

        Ok(Self {
            controller_identity: identity.public_address.clone(),
            root,
            key_store,
            set_db,
            local_keys,
        })
    }

    /// Archive all controller-local bond state and create a fresh persistent
    /// coordinator identity. This is intentionally explicit because rotating
    /// the local IRK invalidates the old peer view of this coordinator.
    pub fn reseed_for_controller(
        identity: &ControllerIdentity,
    ) -> TransportResult<(Self, PathBuf)> {
        let root = bonding_root()?;
        std::fs::create_dir_all(&root)
            .map_err(|error| Error::Remote(format!("failed to create bond directory: {error}")))?;

        let identity_file = root.join("controller-identity.txt");
        enforce_controller_identity(&identity_file, &identity.public_address)?;

        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| Error::Remote(format!("system clock is before UNIX epoch: {error}")))?
            .as_secs();
        let archive = root.join("archive").join(format!("reseed-{epoch}"));
        std::fs::create_dir_all(&archive)
            .map_err(|error| Error::Remote(format!(
                "failed to create bond archive {}: {error}",
                archive.display()
            )))?;

        for name in ["pairing-keys.json", "bond-sets.json", "local-identity.json"] {
            let source = root.join(name);
            if !source.exists() {
                continue;
            }
            let destination = archive.join(name);
            std::fs::rename(&source, &destination).map_err(|error| {
                Error::Remote(format!(
                    "failed to archive {} -> {}: {error}",
                    source.display(),
                    destination.display()
                ))
            })?;
        }

        let store = Self::open_for_controller(identity)?;
        Ok((store, archive))
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
    pub fn local_identity_path(&self) -> PathBuf {
        self.root.join("local-identity.json")
    }

    pub fn has_persistent_local_identity(&self) -> bool {
        self.local_keys.is_some()
    }

    pub(crate) fn local_key_material(&self) -> TransportResult<LocalKeyMaterial> {
        self.local_keys.clone().ok_or_else(|| Error::Remote(
            "legacy bond state has peer keys but no persisted local IRK. The original local IRK was ephemeral and cannot be reconstructed; archive/reset this controller bond state once, then re-pair so all set members see one stable coordinator identity.".into()
        ))
    }

    pub fn local_irk(&self) -> TransportResult<[u8; 16]> {
        Ok(self.local_key_material()?.irk)
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
                            rank: member.rank,
                            csis_verified: member.csis_verified,
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
                rank: None,
                csis_verified: false,
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

    pub fn update_csis_member(
        &mut self,
        target_name: &str,
        identity_address: &str,
        set_size: Option<u8>,
        sirk: [u8; 16],
        rank: Option<u8>,
    ) -> TransportResult<()> {
        let sirk_hex = hex(&sirk);
        let record = self.set_db.sets.entry(target_name.to_owned()).or_default();

        if let Some(size) = set_size {
            if let Some(existing) = record.set_size {
                if existing != size {
                    return Err(Error::Remote(format!(
                        "CSIS set-size mismatch for {target_name:?}: stored {existing}, peer {size}"
                    )));
                }
            }
            record.set_size = Some(size);
        }

        match &record.sirk_hex {
            Some(existing) if !existing.eq_ignore_ascii_case(&sirk_hex) => {
                return Err(Error::Remote(format!(
                    "CSIS SIRK mismatch for {target_name:?}; refusing to merge distinct sets"
                )));
            }
            _ => record.sirk_hex = Some(sirk_hex),
        }

        let member = record
            .members
            .iter_mut()
            .find(|member| member.identity_address.eq_ignore_ascii_case(identity_address))
            .ok_or_else(|| Error::Remote(format!(
                "cannot attach CSIS metadata to unknown member {identity_address}"
            )))?;

        member.rank = rank;
        member.csis_verified = true;
        self.save_set_db()
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

    pub fn set_sirk(&self, target_name: &str) -> TransportResult<Option<[u8; 16]>> {
        self.set_db
            .sets
            .get(target_name)
            .and_then(|record| record.sirk_hex.as_deref())
            .map(decode_hex_16)
            .transpose()
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

    pub(crate) fn encryption_key_for_identity(
        &self,
        identity_address: &str,
    ) -> TransportResult<[u8; 16]> {
        let keys = self
            .key_store
            .get(identity_address)
            .map_err(|error| Error::Remote(format!(
                "failed to read stored bond for {identity_address}: {error}"
            )))?
            .ok_or_else(|| Error::Remote(format!(
                "no stored bond exists for identity {identity_address}"
            )))?;

        let key = keys
            .ltk
            .as_ref()
            .or(keys.ltk_central.as_ref())
            .or(keys.ltk_peripheral.as_ref())
            .ok_or_else(|| Error::Remote(format!(
                "stored bond for {identity_address} has no usable LTK"
            )))?;

        key.value
            .as_slice()
            .try_into()
            .map_err(|_| Error::Remote(format!(
                "stored LTK for {identity_address} is not 16 bytes"
            )))
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

fn load_or_create_local_identity(
    path: &Path,
    controller_identity: &str,
    has_existing_bonds: bool,
) -> TransportResult<Option<LocalKeyMaterial>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let record: LocalIdentityRecord = serde_json::from_slice(&bytes)
                .map_err(|error| Error::Remote(format!(
                    "invalid local identity database: {error}"
                )))?;
            if record.schema_version != LOCAL_IDENTITY_SCHEMA_VERSION {
                return Err(Error::Remote(format!(
                    "unsupported local identity schema {}; expected {}",
                    record.schema_version, LOCAL_IDENTITY_SCHEMA_VERSION
                )));
            }
            if !record.controller_identity.eq_ignore_ascii_case(controller_identity) {
                return Err(Error::Remote(
                    "local identity database belongs to a different controller identity".into()
                ));
            }

            let identity_address = Address::parse(
                controller_identity,
                AddressType::PUBLIC_DEVICE,
            )
            .map_err(|error| Error::InvalidSpec(format!(
                "invalid controller identity for local key material: {error}"
            )))?;

            Ok(Some(LocalKeyMaterial {
                ltk: decode_hex_16_named(&record.ltk_hex, "local LTK")?,
                ediv: record.ediv,
                rand: decode_hex_8_named(&record.rand_hex, "local RAND")?,
                irk: decode_hex_16_named(&record.irk_hex, "local IRK")?,
                identity_address,
                csrk: decode_hex_16_named(&record.csrk_hex, "local CSRK")?,
            }))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if has_existing_bonds {
                return Ok(None);
            }

            let identity_address = Address::parse(
                controller_identity,
                AddressType::PUBLIC_DEVICE,
            )
            .map_err(|error| Error::InvalidSpec(format!(
                "invalid controller identity for local key generation: {error}"
            )))?;
            let local_keys = LocalKeyMaterial::generate(identity_address);
            let record = LocalIdentityRecord {
                schema_version: LOCAL_IDENTITY_SCHEMA_VERSION,
                controller_identity: controller_identity.to_owned(),
                ltk_hex: hex(&local_keys.ltk),
                ediv: local_keys.ediv,
                rand_hex: hex(&local_keys.rand),
                irk_hex: hex(&local_keys.irk),
                csrk_hex: hex(&local_keys.csrk),
            };
            let bytes = serde_json::to_vec_pretty(&record).map_err(|error| {
                Error::Remote(format!("failed to encode local identity database: {error}"))
            })?;
            atomic_write(path, &bytes)?;
            Ok(Some(local_keys))
        }
        Err(error) => Err(Error::Remote(format!(
            "failed to read local identity database: {error}"
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex_16_named(value: &str, label: &str) -> TransportResult<[u8; 16]> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Remote(format!("{label} is not 16-byte hexadecimal data")));
    }
    let mut output = [0u8; 16];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| Error::Remote(format!("{label} contains invalid hex")))?;
    }
    Ok(output)
}

fn decode_hex_8_named(value: &str, label: &str) -> TransportResult<[u8; 8]> {
    if value.len() != 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Remote(format!("{label} is not 8-byte hexadecimal data")));
    }
    let mut output = [0u8; 8];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| Error::Remote(format!("{label} contains invalid hex")))?;
    }
    Ok(output)
}

fn decode_hex_16(value: &str) -> TransportResult<[u8; 16]> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Remote(
            "stored CSIS SIRK is not 16-byte hexadecimal data".into(),
        ));
    }
    let mut output = [0u8; 16];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| Error::Remote("stored CSIS SIRK contains invalid hex".into()))?;
    }
    Ok(output)
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

    #[test]
    fn sirk_hex_round_trip() {
        let value = [0xAB; 16];
        assert_eq!(decode_hex_16(&hex(&value)).unwrap(), value);
    }
}
