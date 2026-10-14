use std::path::Path;

use bumble::{
    keys::{JsonKeyStore, Key, KeyStore, PairingKeys},
    Address, AddressType,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, PartialEq, Eq)]
pub struct ReviewBondMember {
    pub identity_address: String,
    pub identity_address_le: [u8; 6],
    pub address_type: u8,
    pub peer_irk: [u8; 16],
    pub long_term_key: [u8; 16],
    pub encrypted_diversifier: u16,
    pub random_number: [u8; 8],
}

#[derive(Clone, PartialEq, Eq)]
pub struct BondStoreFingerprint {
    controller_identity: [u8; 32],
    local_identity: [u8; 32],
    bond_sets: [u8; 32],
    pairing_keys: [u8; 32],
}

pub fn fingerprint_bond_store(
    root: &Path,
) -> Result<BondStoreFingerprint, BondInputError> {
    Ok(BondStoreFingerprint {
        controller_identity: hash_file(
            &root.join("controller-identity.txt"),
            "controller-identity.txt",
        )?,
        local_identity: hash_file(
            &root.join("local-identity.json"),
            "local-identity.json",
        )?,
        bond_sets: hash_file(
            &root.join("bond-sets.json"),
            "bond-sets.json",
        )?,
        pairing_keys: hash_file(
            &root.join("pairing-keys.json"),
            "pairing-keys.json",
        )?,
    })
}

fn hash_file(
    path: &Path,
    label: &str,
) -> Result<[u8; 32], BondInputError> {
    let bytes = std::fs::read(path).map_err(|error| {
        BondInputError(format!("cannot read {label}: {error}"))
    })?;
    Ok(Sha256::digest(bytes).into())
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReviewBondMaterial {
    pub controller_identity: String,
    pub controller_identity_le: [u8; 6],
    pub local_irk: [u8; 16],
    pub members: Vec<ReviewBondMember>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondInputError(pub String);

pub fn load_verified_bond_set(
    root: &Path,
    target_name: &str,
) -> Result<ReviewBondMaterial, BondInputError> {
    if target_name.trim().is_empty() {
        return Err(BondInputError("target name must not be empty".into()));
    }

    let controller_identity = std::fs::read_to_string(
        root.join("controller-identity.txt"),
    )
    .map_err(|error| {
        BondInputError(format!(
            "cannot read controller-identity.txt: {error}"
        ))
    })?
    .trim()
    .to_string();

    if controller_identity.is_empty() {
        return Err(BondInputError(
            "controller identity is empty".into(),
        ));
    }

    let controller_address = Address::parse(
        &controller_identity,
        AddressType::PUBLIC_DEVICE,
    )
    .map_err(|error| {
        BondInputError(format!(
            "invalid controller identity {controller_identity:?}: {error}"
        ))
    })?;
    let controller_identity_le = *controller_address.address_bytes();

    let local_identity = read_json(
        &root.join("local-identity.json"),
        "local-identity.json",
    )?;
    require_schema(&local_identity, "local identity")?;
    require_controller_identity(
        &local_identity,
        &controller_identity,
        "local identity",
    )?;
    let local_irk = decode_hex::<16>(
        required_str(&local_identity, "irk_hex", "local identity")?,
        "local IRK",
    )?;

    let set_db = read_json(
        &root.join("bond-sets.json"),
        "bond-sets.json",
    )?;
    require_schema(&set_db, "bond set database")?;
    require_controller_identity(
        &set_db,
        &controller_identity,
        "bond set database",
    )?;

    let set = set_db
        .get("sets")
        .and_then(Value::as_object)
        .and_then(|sets| sets.get(target_name))
        .ok_or_else(|| {
            BondInputError(format!(
                "no stored coordinated set named {target_name:?}"
            ))
        })?;
    let set_size = set
        .get("set_size")
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or_else(|| {
            BondInputError(
                "bond set set_size must be a byte".into(),
            )
        })?;
    if set_size != 2 {
        return Err(BondInputError(format!(
            "Review-2B dual-ACL HIL requires stored CSIS set_size=2; found {set_size}"
        )));
    }

    let members = set
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            BondInputError(
                "bond set members must be an array".into(),
            )
        })?;

    let key_store = JsonKeyStore::new(
        Some(&controller_identity),
        root.join("pairing-keys.json"),
    );

    let mut output = Vec::new();
    for member in members {
        if member
            .get("csis_verified")
            .and_then(Value::as_bool)
            != Some(true)
        {
            continue;
        }

        let identity_address =
            required_str(member, "identity_address", "bond member")?
                .to_string();
        let address_type = member
            .get("address_type")
            .and_then(Value::as_u64)
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| {
                BondInputError(
                    "bond member address_type must be a byte".into(),
                )
            })?;

        if !matches!(address_type, 0 | 1) {
            return Err(BondInputError(format!(
                "bond member {identity_address} has unsupported address type {address_type}"
            )));
        }

        let address = Address::parse(
            &identity_address,
            AddressType(address_type),
        )
        .map_err(|error| {
            BondInputError(format!(
                "invalid bonded identity {identity_address}: {error}"
            ))
        })?;

        let keys = key_store
            .get(&identity_address)
            .map_err(|error| {
                BondInputError(format!(
                    "failed to read pairing keys for {identity_address}: {error}"
                ))
            })?
            .ok_or_else(|| {
                BondInputError(format!(
                    "no pairing keys for verified identity {identity_address}"
                ))
            })?;

        if let Some(stored_type) = keys.address_type {
            if stored_type.0 != address_type {
                return Err(BondInputError(format!(
                    "address type mismatch for {identity_address}: set={address_type} keys={}",
                    stored_type.0
                )));
            }
        }

        let peer_irk = key_16(
            keys.irk.as_ref(),
            &identity_address,
            "peer IRK",
        )?;
        let encryption_key = selected_encryption_key(&keys)
            .ok_or_else(|| {
                BondInputError(format!(
                    "stored bond for {identity_address} has no usable LTK"
                ))
            })?;
        let long_term_key = bytes_16(
            &encryption_key.value,
            &identity_address,
            "LTK",
        )?;
        let random_number = match encryption_key.rand.as_deref() {
            Some(value) => bytes_8(
                value,
                &identity_address,
                "RAND",
            )?,
            None => [0; 8],
        };

        output.push(ReviewBondMember {
            identity_address,
            identity_address_le: *address.address_bytes(),
            address_type,
            peer_irk,
            long_term_key,
            encrypted_diversifier:
                encryption_key.ediv.unwrap_or(0),
            random_number,
        });
    }

    if output.is_empty() {
        return Err(BondInputError(format!(
            "stored set {target_name:?} has no CSIS-verified members"
        )));
    }

    if output.len() != usize::from(set_size) {
        return Err(BondInputError(format!(
            "stored set {target_name:?} is incomplete: set_size={set_size}, CSIS-verified members={}",
            output.len()
        )));
    }

    Ok(ReviewBondMaterial {
        controller_identity,
        controller_identity_le,
        local_irk,
        members: output,
    })
}

fn read_json(
    path: &Path,
    label: &str,
) -> Result<Value, BondInputError> {
    let bytes = std::fs::read(path).map_err(|error| {
        BondInputError(format!("cannot read {label}: {error}"))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        BondInputError(format!("invalid {label}: {error}"))
    })
}

fn require_schema(
    value: &Value,
    label: &str,
) -> Result<(), BondInputError> {
    if value.get("schema_version").and_then(Value::as_u64)
        != Some(1)
    {
        return Err(BondInputError(format!(
            "{label} schema_version must be 1"
        )));
    }
    Ok(())
}

fn require_controller_identity(
    value: &Value,
    expected: &str,
    label: &str,
) -> Result<(), BondInputError> {
    let actual = required_str(
        value,
        "controller_identity",
        label,
    )?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(BondInputError(format!(
            "{label} belongs to controller {actual}, expected {expected}"
        )));
    }
    Ok(())
}

fn required_str<'a>(
    value: &'a Value,
    field: &str,
    label: &str,
) -> Result<&'a str, BondInputError> {
    value.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            BondInputError(format!(
                "{label} field {field:?} must be a string"
            ))
        })
}

fn selected_encryption_key(
    keys: &PairingKeys,
) -> Option<&Key> {
    keys.ltk
        .as_ref()
        .or(keys.ltk_central.as_ref())
        .or(keys.ltk_peripheral.as_ref())
}

fn key_16(
    key: Option<&Key>,
    identity: &str,
    label: &str,
) -> Result<[u8; 16], BondInputError> {
    let key = key.ok_or_else(|| {
        BondInputError(format!(
            "stored bond for {identity} has no {label}"
        ))
    })?;
    bytes_16(&key.value, identity, label)
}

fn bytes_16(
    value: &[u8],
    identity: &str,
    label: &str,
) -> Result<[u8; 16], BondInputError> {
    value.try_into().map_err(|_| {
        BondInputError(format!(
            "{label} for {identity} is not 16 bytes"
        ))
    })
}

fn bytes_8(
    value: &[u8],
    identity: &str,
    label: &str,
) -> Result<[u8; 8], BondInputError> {
    value.try_into().map_err(|_| {
        BondInputError(format!(
            "{label} for {identity} is not 8 bytes"
        ))
    })
}

fn decode_hex<const N: usize>(
    value: &str,
    label: &str,
) -> Result<[u8; N], BondInputError> {
    if value.len() != N * 2
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(BondInputError(format!(
            "{label} is not {N}-byte hexadecimal data"
        )));
    }

    let mut output = [0u8; N];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(
            &value[index * 2..index * 2 + 2],
            16,
        )
        .map_err(|_| {
            BondInputError(format!(
                "{label} contains invalid hexadecimal data"
            ))
        })?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        path::PathBuf,
    };

    use bumble::keys::{Key, PairingKeys};
    use serde_json::json;

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn fixture_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "be200-review2-bond-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ))
    }

    fn write_fixture(root: &Path) {
        let controller = "44:A3:BB:36:94:1A";
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("controller-identity.txt"),
            format!("{controller}\n"),
        )
        .unwrap();

        std::fs::write(
            root.join("local-identity.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "controller_identity": controller,
                "ltk_hex": "00000000000000000000000000000000",
                "ediv": 0,
                "rand_hex": "0000000000000000",
                "irk_hex": "22222222222222222222222222222222",
                "csrk_hex": "00000000000000000000000000000000"
            }))
            .unwrap(),
        )
        .unwrap();

        std::fs::write(
            root.join("bond-sets.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "controller_identity": controller,
                "sets": {
                    "Galaxy Buds3 Pro (2F23)": {
                        "set_size": 2,
                        "members": [
                            {
                                "identity_address": "E8:C9:13:EF:2F:23",
                                "address_type": 0,
                                "rank": 1,
                                "csis_verified": true
                            },
                            {
                                "identity_address": "E8:C9:13:F6:5C:E0",
                                "address_type": 0,
                                "rank": 2,
                                "csis_verified": true
                            },
                            {
                                "identity_address": "AA:BB:CC:DD:EE:FF",
                                "address_type": 0,
                                "csis_verified": false
                            }
                        ]
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let mut store = JsonKeyStore::new(
            Some(controller),
            root.join("pairing-keys.json"),
        );

        for (identity, seed, ediv) in [
            ("E8:C9:13:EF:2F:23", 0x11, 0x1234),
            ("E8:C9:13:F6:5C:E0", 0x33, 0),
        ] {
            let mut ltk = Key::new(vec![seed; 16]);
            ltk.ediv = Some(ediv);
            ltk.rand = Some(vec![seed.wrapping_add(1); 8]);

            store
                .update(
                    identity,
                    PairingKeys {
                        address_type:
                            Some(AddressType::PUBLIC_DEVICE),
                        ltk: Some(ltk),
                        irk: Some(Key::new(vec![
                            seed.wrapping_add(2);
                            16
                        ])),
                        ..PairingKeys::default()
                    },
                )
                .unwrap();
        }
    }

    #[test]
    fn loader_reads_only_verified_members_and_key_material() {
        let root = fixture_root();
        write_fixture(&root);

        let material = load_verified_bond_set(
            &root,
            "Galaxy Buds3 Pro (2F23)",
        )
        .unwrap();

        assert_eq!(
            material.controller_identity,
            "44:A3:BB:36:94:1A"
        );
        assert_eq!(
            material.controller_identity_le,
            [0x1a, 0x94, 0x36, 0xbb, 0xa3, 0x44]
        );
        assert_eq!(material.local_irk, [0x22; 16]);
        assert_eq!(material.members.len(), 2);

        let right = &material.members[0];
        assert_eq!(
            right.identity_address,
            "E8:C9:13:EF:2F:23"
        );
        assert_eq!(right.address_type, 0);
        assert_eq!(right.peer_irk, [0x13; 16]);
        assert_eq!(right.long_term_key, [0x11; 16]);
        assert_eq!(right.encrypted_diversifier, 0x1234);
        assert_eq!(right.random_number, [0x12; 8]);

        let left = &material.members[1];
        assert_eq!(
            left.identity_address,
            "E8:C9:13:F6:5C:E0"
        );
        assert_eq!(left.peer_irk, [0x35; 16]);
        assert_eq!(left.long_term_key, [0x33; 16]);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fingerprint_detects_any_bond_store_mutation() {
        let root = fixture_root();
        write_fixture(&root);

        let before = fingerprint_bond_store(&root).unwrap();
        let path = root.join("bond-sets.json");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(b' ');
        std::fs::write(&path, bytes).unwrap();
        let after = fingerprint_bond_store(&root).unwrap();

        assert!(before != after);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loader_requires_dual_member_set_size_and_complete_verified_members() {
        let root = fixture_root();
        write_fixture(&root);

        let path = root.join("bond-sets.json");
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["sets"]["Galaxy Buds3 Pro (2F23)"]["set_size"] =
            Value::from(3);
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            load_verified_bond_set(
                &root,
                "Galaxy Buds3 Pro (2F23)"
            ),
            Err(BondInputError(message))
                if message.contains("set_size=2")
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loader_rejects_controller_identity_mismatch() {
        let root = fixture_root();
        write_fixture(&root);

        let mut value: Value = serde_json::from_slice(
            &std::fs::read(root.join("local-identity.json")).unwrap(),
        )
        .unwrap();
        value["controller_identity"] =
            Value::String("00:00:00:00:00:00".into());
        std::fs::write(
            root.join("local-identity.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            load_verified_bond_set(
                &root,
                "Galaxy Buds3 Pro (2F23)"
            ),
            Err(BondInputError(message))
                if message.contains("belongs to controller")
        ));

        std::fs::remove_dir_all(root).unwrap();
    }
}
