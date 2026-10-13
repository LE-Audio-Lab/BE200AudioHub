//! ASCS ASE reads: strict upstream parsing plus an observed, narrow Idle quirk.
//!
//! AOSP Fluoride dispatches Idle from the two-byte ASE status header without
//! length-checking its trailing bytes. The Buds3 Pro left ear repeatedly
//! returned [ASE_ID, IDLE, 0x01, 0x00, 0x00] on characteristic reads.
//! Accept only that exact extra-byte signature in Idle, preserving strict
//! Bumble-RS validation for every other state and suffix.

use std::time::Duration;

use bumble_gatt::{AttTransport, CharacteristicProxy, GattClient};
use bumble_profiles::ascs::AseStatus;
use bumble_transport::{Error, Result as TransportResult};

const MAX_ATTEMPTS: usize = 3;
const RETRY_PAUSE: Duration = Duration::from_millis(80);
const MAX_LOGGED_BYTES: usize = 96;

const OBSERVED_IDLE_TRAILER: [u8; 3] = [0x01, 0x00, 0x00];

/// Device-specific compatibility, not a change to the generic Bumble-RS
/// parser. An ASE in Idle has no standard state-specific parameters. AOSP's
/// state-machine path ignores trailing Idle data, while Bumble-RS rejects it.
/// Constrain that tolerance to the exact five-byte value seen on hardware.
pub(crate) fn parse_ase_status_compat(value: &[u8]) -> bumble_profiles::Result<AseStatus> {
    match AseStatus::from_bytes(value) {
        Ok(status) => Ok(status),
        Err(_)
            if value.len() == 5
                && value[0] != 0
                && value[1] == 0x00
                && value[2..] == OBSERVED_IDLE_TRAILER =>
        {
            let status = AseStatus::from_bytes(&value[..2])?;
            println!(
                "[R4/ASE-COMPAT] accepted known nonstandard Idle trailer: ASE={} raw={value:02X?}",
                status.ase_id
            );
            Ok(status)
        }
        Err(error) => Err(error),
    }
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
/// hardware. Valid data and unknown malformed data use upstream strict decoding;
/// only the observed Idle suffix passes the explicit compatibility gate.
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
    fn accepts_only_the_observed_buds3pro_idle_suffix_without_retry() {
        let mut reads = 0;
        let status = read_ase_with_retry(0x0801, 0x0046, || {
            reads += 1;
            Ok(vec![1, 0, 1, 0, 0])
        }).unwrap();
        assert_eq!(reads, 1);
        assert_eq!(status.ase_id, 1);
        assert_eq!(status.state.0, 0);
    }

    #[test]
    fn does_not_accept_arbitrary_idle_padding_or_zero_ase_id() {
        for raw in [
            vec![1, 0, 1, 0, 1],
            vec![1, 0, 0, 0, 0],
            vec![0, 0, 1, 0, 0],
            vec![1, 0, 1, 0],
            vec![1, 1, 1, 0, 0],
        ] {
            assert!(parse_ase_status_compat(&raw).is_err(), "unexpectedly accepted {raw:02X?}");
        }
    }

    #[test]
    fn does_not_silently_strip_three_idle_trailing_bytes() {
        let mut reads = 0;
        let result = read_ase_with_retry(0x0801, 0x0049, || {
            reads += 1;
            Ok(vec![2, 0, 0xAA, 0xBB, 0xCC])
        });
        assert_eq!(reads, MAX_ATTEMPTS);
        let message = result.unwrap_err().to_string();
        assert!(message.contains("0x0049"));
        assert!(message.contains("AA"));
        assert!(message.contains("malformed"));
    }

    #[test]
    fn valid_reread_recovers_without_accepting_invalid_bytes() {
        let mut reads = 0;
        let status = read_ase_with_retry(0x0801, 0x0046, || {
            reads += 1;
            if reads == 1 { Ok(vec![1, 0, 1, 2, 3]) } else { Ok(vec![1, 0]) }
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
