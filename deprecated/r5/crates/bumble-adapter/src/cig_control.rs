//! R5.2-A controller-only CIG allocation lifecycle.
//!
//! Pinned Bumble-RS models the HCI commands, but currently preserves the
//! LE Set CIG Parameters Command Complete payload as ReturnParameters::Raw.
//! Decode that one standard layout locally; never guess CIS handles.
//!
//! The CIG created here has *no established CIS*. R5.2-A always issues
//! LE Remove CIG before returning; Create CIS and ISO data are not permitted.

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use bumble_hci::{Command, ReturnParameters};
use bumble_transport::{CommandResponse, Error, Result as TransportResult};

use crate::AttachedController;

const CIG_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CIS_CONNECTION_HANDLE: u16 = 0x0EFF;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CisHandleAllocation {
    pub cis_id: u8,
    pub connection_handle: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CigAllocation {
    pub cig_id: u8,
    pub cis: Vec<CisHandleAllocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CigSetupFailure {
    pub details: String,
    /// A timeout, lost response or malformed *successful* completion means
    /// the controller might have allocated the CIG. Try Remove CIG.
    pub may_have_allocated: bool,
}

impl fmt::Display for CigSetupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.details)
    }
}

impl std::error::Error for CigSetupFailure {}

pub fn parse_set_cig_response(
    parameters: &ReturnParameters,
    expected_cig_id: u8,
    expected_cis_ids: &[u8],
) -> TransportResult<CigAllocation> {
    let ReturnParameters::Raw { data } = parameters else {
        return Err(Error::Remote(format!(
            "LE Set CIG Parameters expected raw Command Complete, got {parameters:?}"
        )));
    };
    if data.is_empty() {
        return Err(Error::Remote("LE Set CIG Parameters returned no status byte".into()));
    }
    if data[0] != 0 {
        return Err(Error::Remote(format!(
            "LE Set CIG Parameters rejected with HCI status 0x{:02X}", data[0]
        )));
    }
    // Vol 4, Part E: Status, CIG_ID, CIS_Count, CIS_Connection_Handle[N].
    let expected_length = 3usize.saturating_add(expected_cis_ids.len().saturating_mul(2));
    if data.len() != expected_length {
        return Err(Error::Remote(format!(
            "LE Set CIG Parameters completion length={} expected {} for {} CIS: {data:02X?}",
            data.len(), expected_length, expected_cis_ids.len()
        )));
    }
    if data[1] != expected_cig_id || usize::from(data[2]) != expected_cis_ids.len() {
        return Err(Error::Remote(format!(
            "LE Set CIG Parameters returned CIG={} CIS_count={}, expected CIG={} CIS_count={}",
            data[1], data[2], expected_cig_id, expected_cis_ids.len()
        )));
    }
    if expected_cis_ids.is_empty() || expected_cis_ids.len() > u8::MAX as usize {
        return Err(Error::InvalidSpec("CIG plan requires at least one valid CIS ID".into()));
    }
    let mut ids = BTreeSet::new();
    let mut handles = BTreeSet::new();
    let mut cis = Vec::with_capacity(expected_cis_ids.len());
    for (index, cis_id) in expected_cis_ids.iter().copied().enumerate() {
        let offset = 3 + 2 * index;
        let handle = u16::from_le_bytes([data[offset], data[offset + 1]]);
        if cis_id == 0 || !ids.insert(cis_id) {
            return Err(Error::InvalidSpec(format!("invalid or duplicate requested CIS ID {cis_id}")));
        }
        if handle == 0 || handle > MAX_CIS_CONNECTION_HANDLE || !handles.insert(handle) {
            return Err(Error::Remote(format!(
                "invalid or duplicate returned CIS handle 0x{handle:04X} at CIS ID {cis_id}"
            )));
        }
        cis.push(CisHandleAllocation { cis_id, connection_handle: handle });
    }
    Ok(CigAllocation { cig_id: expected_cig_id, cis })
}

/// R5.2-B resume gate: LE Remove CIG (0x2065) is idempotent *only at
/// this dedicated no-ACL preflight boundary*. Unknown CIG (0x02) is an
/// acceptable empty state. Any active-CIG disallowed (0x0C) is NOT safe.
fn classify_preflight_remove_cig(
    response: &CommandResponse,
    cig_id: u8,
) -> TransportResult<bool> {
    match response {
        CommandResponse::Complete {
            return_parameters: ReturnParameters::LeRemoveCig {
                status: 0,
                cig_id: returned,
            },
            ..
        } if *returned == cig_id => Ok(true),
        CommandResponse::Complete {
            return_parameters: ReturnParameters::Status { status: 0x02 },
            ..
        } => Ok(false),
        other => Err(Error::Remote(format!(
            "CIG {cig_id} preflight cannot prove inactive/absent CIG; LE Remove CIG returned {other:?}; refusing new CIS session"
        ))),
    }
}

impl AttachedController {
    /// Reconcile only this app's CIG ID before any fresh bonded ACL exists.
    /// Needed after an earlier probe left an inactive CIG allocated.
    /// Does not issue HCI Reset or touch pairing/bond material.
    pub fn reconcile_test_cig_pre_acl(&mut self, cig_id: u8) -> TransportResult<bool> {
        if self.device.is_connected() {
            return Err(Error::Remote(
                "cannot reconcile a CIG after LE ACL connection establishment".into()
            ));
        }
        let response = self.host.send_command(
            Command::LeRemoveCig { cig_id },
            CIG_COMMAND_TIMEOUT,
        )?;
        let removed = classify_preflight_remove_cig(&response, cig_id)?;
        if removed {
            println!(
                "[R5.2/RECOVERY] removed leftover inactive CIG={cig_id} before ACL PASS"
            );
        } else {
            println!("[R5.2/RECOVERY] CIG={cig_id} absent before ACL PASS");
        }
        Ok(removed)
    }

    /// HCI LE Set CIG Parameters (0x2062); never HCI LE Create CIS.
    pub fn set_cig_parameters_unestablished(
        &mut self,
        command: Command,
        expected_cig_id: u8,
        expected_cis_ids: &[u8],
    ) -> Result<CigAllocation, CigSetupFailure> {
        if !matches!(&command, Command::LeSetCigParameters { .. }) {
            return Err(CigSetupFailure {
                details: "R5.2-A only permits HCI LE Set CIG Parameters".into(),
                may_have_allocated: false,
            });
        }
        if expected_cis_ids.is_empty()
            || expected_cis_ids.iter().any(|id| *id == 0)
            || expected_cis_ids.iter().copied().collect::<BTreeSet<_>>().len() != expected_cis_ids.len()
        {
            return Err(CigSetupFailure {
                details: "R5.2-A requires unique nonzero CIS IDs".into(),
                may_have_allocated: false,
            });
        }
        let response = self.host.send_command(command, CIG_COMMAND_TIMEOUT)
            .map_err(|error| CigSetupFailure {
                details: format!("HCI LE Set CIG Parameters transport/timeout error: {error}"),
                may_have_allocated: true,
            })?;
        if let Some(status) = response.status() {
            if status != 0 {
                return Err(CigSetupFailure {
                    details: format!("BE200 rejected HCI LE Set CIG Parameters: status=0x{status:02X}"),
                    may_have_allocated: false,
                });
            }
        }
        let Some(parameters) = response.return_parameters() else {
            return Err(CigSetupFailure {
                details: "LE Set CIG Parameters returned Command Status instead of Command Complete".into(),
                may_have_allocated: true,
            });
        };
        let allocation = parse_set_cig_response(parameters, expected_cig_id, expected_cis_ids)
            .map_err(|error| CigSetupFailure {
                details: format!("invalid LE Set CIG Parameters completion: {error}"),
                may_have_allocated: true,
            })?;
        println!(
            "[R5.2/CIG] HCI Set CIG PASS: CIG={} CIS_count={}",
            allocation.cig_id, allocation.cis.len()
        );
        for entry in &allocation.cis {
            println!(
                "[R5.2/CIG] allocation: CIS ID={} handle=0x{:04X} (not established)",
                entry.cis_id, entry.connection_handle
            );
        }
        Ok(allocation)
    }

    /// HCI LE Remove CIG (0x2065). Valid before any CIS is established.
    pub fn remove_unestablished_cig(&mut self, cig_id: u8) -> TransportResult<()> {
        let response = self.host.send_command(Command::LeRemoveCig { cig_id }, CIG_COMMAND_TIMEOUT)?;
        if response.status() != Some(0) {
            return Err(Error::Remote(format!(
                "HCI LE Remove CIG={} failed with status {:?}", cig_id, response.status()
            )));
        }
        match response.return_parameters() {
            Some(ReturnParameters::LeRemoveCig { status: 0, cig_id: returned })
                if *returned == cig_id =>
            {
                println!("[R5.2/CIG] Remove CIG={} PASS", cig_id);
                Ok(())
            }
            other => Err(Error::Remote(format!(
                "HCI LE Remove CIG completion mismatch for CIG={cig_id}: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8], ids: &[u8]) -> TransportResult<CigAllocation> {
        parse_set_cig_response(
            &ReturnParameters::Raw { data: bytes.to_vec() }, 1, ids
        )
    }

    #[test]
    fn preflight_cig_recovery_requires_confirmed_success_or_absence() {
        let response_ok = CommandResponse::Complete {
            num_hci_command_packets: 1,
            return_parameters: ReturnParameters::LeRemoveCig {
                status: 0, cig_id: 1,
            },
        };
        assert_eq!(classify_preflight_remove_cig(&response_ok, 1).unwrap(), true);
        assert!(classify_preflight_remove_cig(&response_ok, 2).is_err());

        let response_missing = CommandResponse::Complete {
            num_hci_command_packets: 1,
            return_parameters: ReturnParameters::Status { status: 0x02 },
        };
        assert_eq!(classify_preflight_remove_cig(&response_missing, 1).unwrap(), false);

        for status in [0x0C, 0x12] {
            let response = CommandResponse::Complete {
                num_hci_command_packets: 1,
                return_parameters: ReturnParameters::Status { status },
            };
            assert!(classify_preflight_remove_cig(&response, 1).is_err());
        }
        assert!(classify_preflight_remove_cig(
            &CommandResponse::Status {
                status: 0,
                num_hci_command_packets: 1,
            },
            1,
        ).is_err());
    }

    #[test]
    fn maps_two_handles_by_cis_id_in_command_order() {
        let report = parse(&[0, 1, 2, 0x02, 0x08, 0x03, 0x08], &[1, 2]).unwrap();
        assert_eq!(report.cig_id, 1);
        assert_eq!(report.cis, vec![
            CisHandleAllocation { cis_id: 1, connection_handle: 0x0802 },
            CisHandleAllocation { cis_id: 2, connection_handle: 0x0803 },
        ]);
    }

    #[test]
    fn rejects_bad_status_wrong_id_length_and_count() {
        for bytes in [
            vec![],
            vec![0x0C],
            vec![0, 2, 2, 2, 8, 3, 8],
            vec![0, 1, 1, 2, 8, 3, 8],
            vec![0, 1, 2, 2, 8],
        ] {
            assert!(parse(&bytes, &[1, 2]).is_err(), "accepted {bytes:02X?}");
        }
    }

    #[test]
    fn rejects_duplicate_or_invalid_cis_handles_and_ids() {
        assert!(parse(&[0, 1, 2, 2, 8, 2, 8], &[1, 2]).is_err());
        assert!(parse(&[0, 1, 2, 0, 0, 3, 8], &[1, 2]).is_err());
        assert!(parse(&[0, 1, 2, 2, 8, 3, 8], &[1, 1]).is_err());
        assert!(parse(&[0, 1, 2, 0, 0x0F, 3, 8], &[1, 2]).is_err());
    }
}
