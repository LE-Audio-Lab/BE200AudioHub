use bumble::keys::KeyStore;
use bumble_transport::{Error, Result as TransportResult};

use crate::{BondStore, CommittedBondReport, CsisMemberReport, ProvisionalBond};

/// Commit the primary peer after its encrypted CSIS metadata has been read.
///
/// Unlike a later set member, the primary establishes the expected SIRK and
/// Set Size. The peer keys are still provisional until this function receives
/// the CSIS report from the encrypted ACL.
pub fn commit_primary_bond(
    store: &mut BondStore,
    target_name: &str,
    provisional: ProvisionalBond,
    csis: &CsisMemberReport,
) -> TransportResult<CommittedBondReport> {
    if csis.sirk == [0; 16] {
        return Err(Error::Remote(
            "refusing primary commit: peer returned an all-zero SIRK".into(),
        ));
    }
    if csis.set_size == Some(0) {
        return Err(Error::Remote(
            "refusing primary commit: peer returned CSIS Set Size 0".into(),
        ));
    }
    if csis.rank == Some(0) {
        return Err(Error::Remote(
            "refusing primary commit: peer returned invalid CSIS rank 0".into(),
        ));
    }

    let identity = provisional.identity_address().to_owned();
    let previous_keys = store
        .key_store()
        .get(&identity)
        .map_err(|error| Error::Remote(format!(
            "failed to snapshot existing primary keys for {identity}: {error}"
        )))?;

    println!(
        "[R2/BOND-COMMIT] committing primary after CSIS read: identity={} type={} rank={} set_size={}",
        identity,
        provisional.address_type(),
        csis.rank
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into()),
        csis.set_size
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into())
    );

    store
        .key_store_mut()
        .update(&identity, provisional.pairing_keys().clone())
        .map_err(|error| Error::Remote(format!(
            "failed to commit primary peer keys for {identity}: {error}"
        )))?;

    let metadata_result = (|| -> TransportResult<()> {
        store.record_member(target_name, &identity, provisional.address_type())?;
        store.update_csis_member(
            target_name,
            &identity,
            csis.set_size,
            csis.sirk,
            csis.rank,
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
                "primary metadata commit failed for {identity}: {metadata_error}; peer key file was rolled back. A recoverable unverified member record may remain."
            ))),
            Err(rollback_error) => Err(Error::Remote(format!(
                "primary metadata commit failed for {identity}: {metadata_error}; key rollback also failed: {rollback_error}"
            ))),
        };
    }

    println!(
        "[R2/BOND-COMMIT] PRIMARY PASS: peer keys and initial CSIS set metadata persisted for {}",
        identity
    );

    Ok(CommittedBondReport {
        identity_address: identity,
        address_type: provisional.address_type(),
        authenticated: provisional.authenticated(),
        rank: csis.rank,
        set_size: csis.set_size,
    })
}
