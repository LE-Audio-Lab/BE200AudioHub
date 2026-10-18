use std::collections::BTreeMap;

use bumble_host::Advertisement;

const MAX_SCAN_OBSERVATIONS: usize = 256;
const RESOLVABLE_SET_IDENTIFIER_AD_TYPE: u8 = 0x2e;

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
    pub raw_data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanSummary {
    pub reports_seen: usize,
    pub observations: Vec<ScanObservation>,
    pub truncated: bool,
}

#[derive(Default)]
pub(crate) struct ScanAccumulator {
    reports_seen: usize,
    observations: BTreeMap<String, ScanObservation>,
    truncated: bool,
}

impl ScanAccumulator {
    pub fn reset(&mut self) {
        self.reports_seen = 0;
        self.observations.clear();
        self.truncated = false;
    }

    pub fn push(&mut self, advertisement: Advertisement) {
        self.reports_seen = self.reports_seen.saturating_add(1);

        let address = advertisement.address.to_string(true);
        let mut observation = ScanObservation {
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
            raw_data: advertisement.data_bytes,
        };

        if let Some(previous) = self.observations.get(&address) {
            if observation.name.is_none() {
                observation.name = previous.name.clone();
            }
            if observation.rsi.is_none() {
                observation.rsi = previous.rsi;
            }
            self.observations.insert(address, observation);
            return;
        }

        if self.observations.len() >= MAX_SCAN_OBSERVATIONS {
            self.truncated = true;
            return;
        }

        self.observations.insert(address, observation);
    }

    pub fn summary(&self) -> ScanSummary {
        ScanSummary {
            reports_seen: self.reports_seen,
            observations: self.observations.values().cloned().collect(),
            truncated: self.truncated,
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
                if count < 1 || end > self.data.len() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_name_wins_over_short_name() {
        let data = [
            0x04, 0x08, b'B', b'3', b'P',
            0x06, 0x09, b'B', b'u', b'd', b's', b'3',
        ];
        assert_eq!(display_name(&data), Some("Buds3".into()));
    }

    #[test]
    fn rsi_is_extracted_only_from_exact_six_byte_structure() {
        let data = [
            0x07, 0x2e, 1, 2, 3, 4, 5, 6,
            0x03, 0x2e, 9, 9,
        ];
        assert_eq!(
            resolvable_set_identifier(&data),
            Some([1, 2, 3, 4, 5, 6])
        );
    }

    #[test]
    fn malformed_ad_structure_terminates_safely() {
        let data = [0x05, 0x09, b'A'];
        assert_eq!(display_name(&data), None);
        assert_eq!(resolvable_set_identifier(&data), None);
    }
}
