use bumble::keys::KeyStore;
use bumble_transport::{Error, Result as TransportResult};

use crate::{BondStore, CsisMemberReport, ProvisionalBond};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedBondReport {
    pub identity_address: String,
    pub address_type: u8,
    pub authenticated: bool,
    pub rank: Option<u8>,
    pub set_size: Option<u8>,
}

/// Commit a peer transaction only after authoritative CSIS verification.
///
/// Validation failures perform no write. The peer key file is snapshotted and
/// restored if a subsequent metadata write fails. A metadata I/O failure after
/// `record_member` can leave a recoverable unverified member record, but never
/// a falsely CSIS-verified member.
pub fn commit_verified_set_member(
    store: &mut BondStore,
    target_name: &str,
    provisional: ProvisionalBond,
    verified: &CsisMemberReport,
) -> TransportResult<CommittedBondReport> {
    let expected_sirk = store
        .set_sirk(target_name)?
        .ok_or_else(|| Error::Remote(format!(
            "cannot commit set member for {target_name:?}: expected SIRK is absent"
        )))?;
    if verified.sirk != expected_sirk {
        return Err(Error::Remote(
            "refusing commit: verifier report SIRK does not match stored set".into(),
        ));
    }

    let summary = store.set_summary(target_name);
    if let (Some(expected), Some(actual)) = (summary.set_size, verified.set_size) {
        if expected != actual {
            return Err(Error::Remote(format!(
                "refusing commit: verified Set Size {actual} does not match stored {expected}"
            )));
        }
    }

    let identity = provisional.identity_address().to_owned();
    if store.is_member_of(target_name, &identity) {
        return Err(Error::Remote(format!(
            "refusing duplicate commit: {identity} is already a member of {target_name:?}"
        )));
    }

    let previous_keys = store
        .key_store()
        .get(&identity)
        .map_err(|error| Error::Remote(format!(
            "failed to snapshot existing keys for {identity}: {error}"
        )))?;

    println!(
        "[R2/BOND-COMMIT] committing verified peer keys: identity={} type={} rank={} set_size={}",
        identity,
        provisional.address_type(),
        verified
            .rank
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into()),
        verified
            .set_size
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into())
    );

    store
        .key_store_mut()
        .update(&identity, provisional.pairing_keys().clone())
        .map_err(|error| Error::Remote(format!(
            "failed to commit verified peer keys for {identity}: {error}"
        )))?;

    let metadata_result = (|| -> TransportResult<()> {
        store.record_member(target_name, &identity, provisional.address_type())?;
        store.update_csis_member(
            target_name,
            &identity,
            verified.set_size,
            verified.sirk,
            verified.rank,
        )?;
        Ok(())
    })();

    if let Err(metadata_error) = metadata_result {
        let rollback = match previous_keys {
            Some(keys) => store.key_store_mut().update(&identity, keys),
            None => store.key_store_mut().delete(&identity),
        };
        return match rollback {
            Ok(()) => Err(Error::Remote(format!(
                "verified peer metadata commit failed for {identity}: {metadata_error}; peer key file was rolled back. A recoverable unverified set record may remain."
            ))),
            Err(rollback_error) => Err(Error::Remote(format!(
                "verified peer metadata commit failed for {identity}: {metadata_error}; key rollback also failed: {rollback_error}"
            ))),
        };
    }

    println!(
        "[R2/BOND-COMMIT] PASS: peer keys and CSIS-verified member metadata persisted for {}",
        identity
    );

    Ok(CommittedBondReport {
        identity_address: identity,
        address_type: provisional.address_type(),
        authenticated: provisional.authenticated(),
        rank: verified.rank,
        set_size: verified.set_size,
    })
}
