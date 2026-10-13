//! ASCS ASE reads: AOSP-style Idle header handling, strict non-Idle parsing.
//!
//! Buds3 Pro has returned both [ASE_ID, 0, 1, 0, 0] and [ASE_ID, 0, 0, 0, 0]
//! on GATT reads. The ASE ID + Idle header is meaningful; any trailing Idle
//! bytes are ignored and logged. Non-Idle states still use the full Bumble-RS
//! parser so Codec/QoS state payloads cannot silently lose information.

use std::time::Duration;

use bumble_gatt::{AttTransport, CharacteristicProxy, GattClient};
use bumble_profiles::ascs::AseStatus;
use bumble_transport::{Error, Result as TransportResult};

const MAX_ATTEMPTS: usize = 3;
const RETRY_PAUSE: Duration = Duration::from_millis(80);
const MAX_LOGGED_BYTES: usize = 96;

/// Idle carries only ASE ID and state. Mirror Fluoride's Idle dispatch by
/// decoding those two bytes and ignoring any trailing payload. The fallback
/// is not applied to Codec/QoS/other states; those stay strictly parsed.
/// Keep full raw bytes visible (bounded for logging) so unexpected devices
/// remain diagnosable rather than silently normalizing their responses.
pub(crate) fn parse_ase_status_compat(value: &[u8]) -> bumble_profiles::Result<AseStatus> {
    if value.len() > 2 && value[0] != 0 && value[1] == 0x00 {
        let status = AseStatus::from_bytes(&value[..2])?;
        let captured = &value[..value.len().min(MAX_LOGGED_BYTES)];
        println!(
            "[R4/ASE-COMPAT] ignored {} trailing Idle byte(s): ASE={} len={} raw={captured:02X?}",
            value.len() - 2, status.ase_id, value.len()
        );
        return Ok(status);
    }
    AseStatus::from_bytes(value)
}

/// Re-issue only a malformed ASE *value* read; real ATT transport errors
/// propagate directly. Used by R3 discovery and R4.2 transaction readbacks.
pub(crate) fn read_ase_checked(
    acl_handle: u16,
    characteristic: &CharacteristicProxy,
    client: &mut GattClient,
    transport: &mut impl AttTransport,
) -> TransportResult<AseStatus> {
    read_ase_with_retry(acl_handle, characteristic.handle, || {
        client
            .read_value(transport, characteristic.handle, false)
            .map_err(|error| Error::Remote(format!(
                "GATT ASE read 0x{:04X} failed: {error}", characteristic.handle
            )))
    })
}

/// Injected read function makes retry behavior verifiable without Bluetooth
/// hardware. Idle status is decoded from the required header only; all other
/// states retain upstream strict decoding and malformed-value retries.
fn read_ase_with_retry(
    acl_handle: u16,
    value_handle: u16,
    mut read_value: impl FnMut() -> TransportResult<Vec<u8>>,
) -> TransportResult<AseStatus> {
    for attempt in 1..=MAX_ATTEMPTS {
        let value = read_value()?;
        match parse_ase_status_compat(&value) {
            Ok(status) => {
                if attempt > 1 {
                    println!(
                        "[R4/ASE-READ] recovered after {attempt} reads: ACL=0x{acl_handle:04X} value=0x{value_handle:04X} ASE={} state=0x{:02X}",
                        status.ase_id, status.state.0
                    );
                }
                return Ok(status);
            }
            Err(error) => {
                let captured = &value[..value.len().min(MAX_LOGGED_BYTES)];
                println!(
                    "[R4/ASE-READ] malformed status: ACL=0x{acl_handle:04X} value=0x{value_handle:04X} attempt={attempt}/{MAX_ATTEMPTS} len={} raw={captured:02X?} error={error}",
                    value.len()
                );
                if attempt == MAX_ATTEMPTS {
                    return Err(Error::Remote(format!(
                        "ASE status remained malformed after {MAX_ATTEMPTS} independent GATT reads on ACL 0x{acl_handle:04X}, value 0x{value_handle:04X}; last length={} raw={captured:02X?}; parser: {error}",
                        value.len()
                    )));
                }
                std::thread::sleep(RETRY_PAUSE);
            }
        }
    }
    unreachable!("nonzero finite number of ASE read attempts")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_idle_on_first_read() {
        let mut reads = 0;
        let status = read_ase_with_retry(0x0801, 0x0046, || {
            reads += 1;
            Ok(vec![1, 0])
        }).unwrap();
        assert_eq!(reads, 1);
        assert_eq!(status.ase_id, 1);
        assert_eq!(status.state.0, 0);
    }

    #[test]
    fn accepts_both_observed_buds3pro_idle_suffixes_without_retry() {
        for payload in [vec![1, 0, 1, 0, 0], vec![1, 0, 0, 0, 0]] {
            let mut reads = 0;
            let status = read_ase_with_retry(0x0801, 0x0046, || {
                reads += 1;
                Ok(payload.clone())
            }).unwrap();
            assert_eq!(reads, 1);
            assert_eq!(status.ase_id, 1);
            assert_eq!(status.state.0, 0);
        }
    }

    #[test]
    fn rejects_zero_ase_id_missing_header_and_non_idle_trailing_bytes() {
        for raw in [
            vec![0, 0, 1, 0, 0],
            vec![1],
            vec![1, 1, 1, 0, 0],
            vec![1, 2, 1, 0, 0],
        ] {
            assert!(parse_ase_status_compat(&raw).is_err(), "unexpectedly accepted {raw:02X?}");
        }
    }

    #[test]
    fn ignores_idle_trailing_bytes_without_retries() {
        let mut reads = 0;
        let status = read_ase_with_retry(0x0801, 0x0049, || {
            reads += 1;
            Ok(vec![2, 0, 0xAA, 0xBB, 0xCC, 0xDD])
        }).unwrap();
        assert_eq!(reads, 1);
        assert_eq!(status.ase_id, 2);
        assert_eq!(status.state.0, 0);
    }

    #[test]
    fn valid_reread_recovers_without_accepting_invalid_bytes() {
        let mut reads = 0;
        let status = read_ase_with_retry(0x0801, 0x0046, || {
            reads += 1;
            if reads == 1 { Ok(vec![1, 1, 1, 2, 3]) } else { Ok(vec![1, 0]) }
        }).unwrap();
        assert_eq!(reads, 2);
        assert_eq!(status.ase_id, 1);
        assert_eq!(status.state.0, 0);
    }

    #[test]
    fn transport_error_is_not_hidden_by_parser_retry() {
        let mut reads = 0;
        let result = read_ase_with_retry(0x0801, 0x0046, || {
            reads += 1;
            Err(Error::Remote("ATT bearer lost".into()))
        });
        assert_eq!(reads, 1);
        assert!(result.unwrap_err().to_string().contains("ATT bearer lost"));
    }
}
