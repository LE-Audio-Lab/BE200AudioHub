use std::time::Duration;

use bumble_transport::{Error, Result as TransportResult};

use crate::{AttachedController, CsisMemberReport, ProvisionalBond};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetMemberVerification {
    Verified(CsisMemberReport),
    Rejected {
        report: CsisMemberReport,
        reason: String,
    },
}

impl SetMemberVerification {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified(_))
    }
}

impl AttachedController {
    /// Authoritatively verify one provisional peer against the expected CSIS
    /// set before any persistent key or member metadata is committed.
    pub fn verify_provisional_set_member(
        &mut self,
        connection_handle: u16,
        provisional: &ProvisionalBond,
        expected_sirk: [u8; 16],
        expected_set_size: Option<u8>,
        timeout: Duration,
    ) -> TransportResult<SetMemberVerification> {
        println!(
            "[R2/SET-VERIFY] begin: handle=0x{connection_handle:04X} provisional_identity={}",
            provisional.identity_address()
        );

        let report = self.read_csis_member_with_ltk(
            connection_handle,
            provisional.ltk(),
            timeout,
        )?;

        let outcome = evaluate_set_member(expected_sirk, expected_set_size, report);
        match &outcome {
            SetMemberVerification::Verified(report) => println!(
                "[R2/SET-VERIFY] PASS: identity={} set_size={} rank={}",
                provisional.identity_address(),
                report
                    .set_size
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "?".into()),
                report
                    .rank
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "?".into())
            ),
            SetMemberVerification::Rejected { reason, .. } => println!(
                "[R2/SET-VERIFY] REJECT: identity={} reason={}",
                provisional.identity_address(),
                reason
            ),
        }
        Ok(outcome)
    }
}

fn evaluate_set_member(
    expected_sirk: [u8; 16],
    expected_set_size: Option<u8>,
    report: CsisMemberReport,
) -> SetMemberVerification {
    if report.sirk != expected_sirk {
        return SetMemberVerification::Rejected {
            report,
            reason: "peer SIRK does not match the expected coordinated set".into(),
        };
    }

    if let (Some(expected), Some(actual)) = (expected_set_size, report.set_size) {
        if expected != actual {
            return SetMemberVerification::Rejected {
                report,
                reason: format!(
                    "peer Set Size {actual} does not match expected Set Size {expected}"
                ),
            };
        }
    }

    if report.rank == Some(0) {
        return SetMemberVerification::Rejected {
            report,
            reason: "peer returned invalid CSIS rank 0".into(),
        };
    }

    SetMemberVerification::Verified(report)
}

pub fn require_verified(
    verification: SetMemberVerification,
) -> TransportResult<CsisMemberReport> {
    match verification {
        SetMemberVerification::Verified(report) => Ok(report),
        SetMemberVerification::Rejected { reason, .. } => Err(Error::Remote(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(sirk: [u8; 16], set_size: Option<u8>, rank: Option<u8>) -> CsisMemberReport {
        CsisMemberReport {
            sirk,
            set_size,
            rank,
        }
    }

    #[test]
    fn matching_sirk_and_size_is_verified() {
        let sirk = [0x11; 16];
        assert!(matches!(
            evaluate_set_member(sirk, Some(2), report(sirk, Some(2), Some(2))),
            SetMemberVerification::Verified(_)
        ));
    }

    #[test]
    fn sirk_mismatch_is_rejected() {
        assert!(matches!(
            evaluate_set_member(
                [0x11; 16],
                Some(2),
                report([0x22; 16], Some(2), Some(2))
            ),
            SetMemberVerification::Rejected { .. }
        ));
    }
}
