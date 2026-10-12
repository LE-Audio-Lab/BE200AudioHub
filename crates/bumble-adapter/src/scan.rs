use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bumble_hci::{Command, ReturnParameters};
use bumble_transport::{Error, ExternalHostActivity, Result as TransportResult};

use crate::AttachedController;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const SCAN_POLL_SLICE: Duration = Duration::from_millis(300);
const RESOLVABLE_SET_IDENTIFIER_AD_TYPE: u8 = 0x2E;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanObservation {
    pub address: String,
    pub address_type: u8,
    pub name: Option<String>,
    pub rsi: Option<[u8; 6]>,
    pub rssi: i8,
    pub primary_phy: u8,
    pub secondary_phy: u8,
    pub sid: u8,
    pub is_legacy: bool,
    pub is_connectable: bool,
    pub is_scannable: bool,
    pub data_len: usize,
    /// Raw advertising data exactly as surfaced by Bumble for the retained
    /// observation. R2 forensic scans decode this without changing the normal
    /// candidate-selection path.
    pub raw_data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanSummary {
    pub reports_seen: usize,
    pub observations: Vec<ScanObservation>,
}

impl AttachedController {
    /// R2.2: passive LE 1M scan using the same controller parameters validated
    /// by R1.6, while all command transport and advertisement event processing
    /// now runs through Bumble-RS.
    ///
    /// This method never initiates a connection, sends SCAN_REQ, pairs, or
    /// touches CIS/ISO state.
    pub fn scan_passive_extended(&mut self, seconds: u16) -> TransportResult<ScanSummary> {
        if !(1..=30).contains(&seconds) {
            return Err(Error::InvalidSpec(
                "R2 scan duration must be 1..=30 seconds".into(),
            ));
        }

        successful_command(
            &mut self.host,
            Command::LeSetExtendedScanParameters {
                own_address_type: 0,
                scanning_filter_policy: 0,
                scanning_phys: 0x01,
                scan_types: vec![0],
                scan_intervals: vec![0x0060],
                scan_windows: vec![0x0030],
            },
            COMMAND_TIMEOUT,
        )?;

        self.device.poll(&mut self.host);
        self.device.take_advertisements();

        let duration = seconds
            .checked_mul(100)
            .and_then(|value| value.checked_add(50))
            .ok_or_else(|| Error::InvalidSpec("R2 scan duration overflow".into()))?;

        let outcome = (|| -> TransportResult<ScanSummary> {
            successful_command(
                &mut self.host,
                Command::LeSetExtendedScanEnable {
                    enable: 1,
                    filter_duplicates: 1,
                    duration,
                    period: 0,
                },
                COMMAND_TIMEOUT,
            )?;

            let deadline = Instant::now() + Duration::from_secs(u64::from(seconds));
            let mut reports_seen = 0usize;
            let mut observations = BTreeMap::<String, ScanObservation>::new();

            while Instant::now() < deadline {
                self.device.poll(&mut self.host);
                drain_advertisements(
                    &mut self.device,
                    &mut reports_seen,
                    &mut observations,
                );

                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match self.host.wait_for_device_activity(
                    &mut self.device,
                    remaining.min(SCAN_POLL_SLICE),
                )? {
                    ExternalHostActivity::Packet | ExternalHostActivity::Timeout => {}
                    ExternalHostActivity::Ended => {
                        return Err(Error::Remote(
                            "Bumble HCI transport ended during R2 scan".into(),
                        ))
                    }
                }
            }

            self.device.poll(&mut self.host);
            drain_advertisements(
                &mut self.device,
                &mut reports_seen,
                &mut observations,
            );

            Ok(ScanSummary {
                reports_seen,
                observations: observations.into_values().collect(),
            })
        })();

        let cleanup = successful_command(
            &mut self.host,
            Command::LeSetExtendedScanEnable {
                enable: 0,
                filter_duplicates: 0,
                duration: 0,
                period: 0,
            },
            COMMAND_TIMEOUT,
        )
        .map(|_| ());

        match (outcome, cleanup) {
            (Ok(summary), Ok(())) => Ok(summary),
            (Ok(_), Err(error)) => Err(Error::Remote(format!(
                "R2 scan completed but disable could not be confirmed: {error}"
            ))),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(disable)) => Err(Error::Remote(format!(
                "{error}; additionally failed to disable R2 scan: {disable}"
            ))),
        }
    }
}

fn drain_advertisements(
    device: &mut bumble_host::Device,
    reports_seen: &mut usize,
    observations: &mut BTreeMap<String, ScanObservation>,
) {
    for advertisement in device.take_advertisements() {
        *reports_seen += 1;
        let address = advertisement.address.to_string(true);
        let observation = ScanObservation {
            address: address.clone(),
            address_type: advertisement.address.address_type().0,
            name: display_name(&advertisement.data_bytes),
            rsi: resolvable_set_identifier(&advertisement.data_bytes),
            rssi: advertisement.rssi,
            primary_phy: advertisement.primary_phy,
            secondary_phy: advertisement.secondary_phy,
            sid: advertisement.sid,
            is_legacy: advertisement.is_legacy,
            is_connectable: advertisement.is_connectable,
            is_scannable: advertisement.is_scannable,
            data_len: advertisement.data_bytes.len(),
            raw_data: advertisement.data_bytes.clone(),
        };

        match observations.get(&address) {
            Some(previous) if observation.name.is_none() && previous.name.is_some() => {
                let mut merged = observation;
                merged.name = previous.name.clone();
                if merged.rsi.is_none() {
                    merged.rsi = previous.rsi;
                }
                observations.insert(address, merged);
            }
            Some(previous) if observation.rsi.is_none() && previous.rsi.is_some() => {
                let mut merged = observation;
                merged.rsi = previous.rsi;
                observations.insert(address, merged);
            }
            _ => {
                observations.insert(address, observation);
            }
        }
    }
}

fn display_name(data: &[u8]) -> Option<String> {
    let mut short = None;
    for (kind, value) in ad_structures(data) {
        if !matches!(kind, 0x08 | 0x09) || value.is_empty() {
            continue;
        }
        let safe: String = String::from_utf8_lossy(value)
            .chars()
            .filter(|character| !character.is_control())
            .take(64)
            .collect();
        if safe.is_empty() {
            continue;
        }
        if kind == 0x09 {
            return Some(safe);
        }
        short = Some(safe);
    }
    short
}

fn resolvable_set_identifier(data: &[u8]) -> Option<[u8; 6]> {
    ad_structures(data).find_map(|(kind, value)| {
        if kind != RESOLVABLE_SET_IDENTIFIER_AD_TYPE || value.len() != 6 {
            return None;
        }
        value.try_into().ok()
    })
}

fn ad_structures(data: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    struct AdIter<'a> {
        data: &'a [u8],
        offset: usize,
    }

    impl<'a> Iterator for AdIter<'a> {
        type Item = (u8, &'a [u8]);

        fn next(&mut self) -> Option<Self::Item> {
            while self.offset < self.data.len() {
                let count = usize::from(self.data[self.offset]);
                if count == 0 {
                    self.offset = self.data.len();
                    return None;
                }
                let end = self.offset.checked_add(count + 1)?;
                if end > self.data.len() || count < 1 {
                    self.offset = self.data.len();
                    return None;
                }
                let kind = self.data[self.offset + 1];
                let value = &self.data[self.offset + 2..end];
                self.offset = end;
                return Some((kind, value));
            }
            None
        }
    }

    AdIter { data, offset: 0 }
}

fn successful_command(
    host: &mut bumble_transport::ExternalHost,
    command: Command,
    timeout: Duration,
) -> TransportResult<ReturnParameters> {
    let opcode = command.op_code();
    let response = host.send_command(command, timeout)?;
    if response.status() != Some(0) {
        return Err(Error::Remote(format!(
            "HCI command {opcode:#06x} failed with status {:?}",
            response.status()
        )));
    }
    response.return_parameters().cloned().ok_or_else(|| {
        Error::Remote(format!(
            "HCI command {opcode:#06x} returned Command Status instead of Command Complete"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ad_name_prefers_complete_and_strips_controls() {
        assert_eq!(
            display_name(&[3, 0x08, b'B', b'3', 4, 0x09, b'P', 0x1B, b'X']),
            Some("PX".into())
        );
    }

    #[test]
    fn ad_name_rejects_truncated_structure() {
        assert_eq!(display_name(&[5, 0x09, b'X']), None);
    }

    #[test]
    fn parses_resolvable_set_identifier() {
        let data = [7, 0x2E, 1, 2, 3, 4, 5, 6];
        assert_eq!(resolvable_set_identifier(&data), Some([1, 2, 3, 4, 5, 6]));
    }
}
