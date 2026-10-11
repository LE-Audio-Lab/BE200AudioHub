//! Offline Intel Bluetooth SFI/DDC inspector.
//! R1.5a does not access USB, send HCI, install drivers, or validate signatures.
//! Formats derived from public Linux drivers/bluetooth/btintel.c.
use crate::error::{AppError, Result};
use std::{fs, path::Path};

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const RSA_HEADER_LEN: usize = 644;
const ECDSA_HEADER_LEN: usize = 320;
const HYBRID_HEADER_LEN: usize = 2116;
const CSS_VERSION_OFFSET: usize = 8;
const CSS_RSA: u32 = 0x0001_0000;
const CSS_ECDSA: u32 = 0x0002_0000;
const CSS_HYBRID: u32 = 0x0006_9700;
const MAX_SECURE_SEND_DATA: usize = 252;

fn failure(message: impl Into<String>) -> AppError {
    AppError::Firmware(message.into())
}

fn read_u32_le(bytes: &[u8], offset: usize, label: &str) -> Result<u32> {
    let slice = bytes.get(offset..offset + 4)
        .ok_or_else(|| failure(format!("Missing {label} at offset {offset}")))?;
    Ok(u32::from_le_bytes(slice.try_into().expect("four bytes")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderKind {
    Rsa,
    RsaEcdsa,
    Hybrid,
}
impl HeaderKind {
    fn label(self) -> &'static str {
        match self {
            Self::Rsa => "RSA",
            Self::RsaEcdsa => "RSA + ECDSA",
            Self::Hybrid => "Hybrid ECDSA + LMS",
        }
    }
    fn payload_offset(self) -> usize {
        match self {
            Self::Rsa => RSA_HEADER_LEN,
            Self::RsaEcdsa => RSA_HEADER_LEN + ECDSA_HEADER_LEN,
            Self::Hybrid => HYBRID_HEADER_LEN,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BootParameters {
    address: u32,
    build: u8,
    week: u8,
    year_since_2000: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DataFragment {
    /// Source file offset, not an HCI wire offset.
    offset: usize,
    length: usize,
}

#[derive(Debug)]
struct SfiPlan {
    header: HeaderKind,
    payload_offset: usize,
    embedded_commands: usize,
    aligned_groups: usize,
    data_fragments: Vec<DataFragment>,
    boot_parameters: Option<BootParameters>,
}

/// Validate the file structure and derive DATA fragments only.
/// Signature/header fragment choice also depends on runtime Intel SBE type;
/// this function must not be treated as a ready-to-replay command stream.
fn plan_sfi(bytes: &[u8]) -> Result<SfiPlan> {
    if bytes.len() < RSA_HEADER_LEN || bytes.len() > MAX_IMAGE_BYTES {
        return Err(failure(format!("Invalid SFI size {} bytes", bytes.len())));
    }

    let css_version = read_u32_le(bytes, CSS_VERSION_OFFSET, "CSS header version")?;
    let header = match css_version {
        CSS_RSA => {
            if bytes.len() >= RSA_HEADER_LEN + ECDSA_HEADER_LEN
                && bytes[RSA_HEADER_LEN] == 0x06
                && read_u32_le(bytes, RSA_HEADER_LEN + CSS_VERSION_OFFSET, "ECDSA CSS version")?
                    == CSS_ECDSA
            {
                HeaderKind::RsaEcdsa
            } else {
                HeaderKind::Rsa
            }
        }
        CSS_HYBRID => HeaderKind::Hybrid,
        version => {
            return Err(failure(format!("Unsupported SFI CSS version 0x{version:08X}")));
        }
    };
    let payload_offset = header.payload_offset();
    if bytes.len() <= payload_offset {
        return Err(failure(format!(
            "{} SFI has no embedded firmware commands after offset {payload_offset}",
            header.label()
        )));
    }

    let mut cursor = payload_offset;
    let mut group_start = cursor;
    let mut group_length = 0usize;
    let mut embedded_commands = 0usize;
    let mut aligned_groups = 0usize;
    let mut fragments = Vec::new();
    let mut boot_parameters = None;

    while cursor < bytes.len() {
        if bytes.len() - cursor < 3 {
            return Err(failure(format!("Truncated embedded HCI command header at {cursor}")));
        }
        let opcode = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
        let param_len = usize::from(bytes[cursor + 2]);
        let length = 3 + param_len;
        let Some(end) = cursor.checked_add(length) else {
            return Err(failure("Embedded command length overflow"));
        };
        if end > bytes.len() {
            return Err(failure(format!(
                "Embedded opcode 0x{opcode:04X} at {cursor} extends beyond SFI end"
            )));
        }

        // The firmware includes the Intel Write Boot Parameters (0xFC0E)
        // command, from which the controller-specific boot address is obtained.
        if opcode == 0xFC0E {
            if param_len < 7 || boot_parameters.is_some() {
                return Err(failure("Unexpected or duplicate Intel Write Boot Parameters command"));
            }
            boot_parameters = Some(BootParameters {
                address: read_u32_le(bytes, cursor + 3, "Intel boot address")?,
                build: bytes[cursor + 7],
                week: bytes[cursor + 8],
                year_since_2000: bytes[cursor + 9],
            });
        }

        cursor = end;
        embedded_commands += 1;
        group_length += length;

        // Linux btintel accumulates complete embedded HCI commands until the
        // group is 4-byte aligned, then submits <=252-byte Secure Send pieces.
        if group_length % 4 == 0 {
            let mut sent = 0usize;
            while sent < group_length {
                let len = (group_length - sent).min(MAX_SECURE_SEND_DATA);
                fragments.push(DataFragment {
                    offset: group_start + sent,
                    length: len,
                });
                sent += len;
            }
            aligned_groups += 1;
            group_start = cursor;
            group_length = 0;
        }
    }

    if group_length != 0 {
        return Err(failure(format!(
            "SFI ends with {group_length} bytes of unaligned embedded commands"
        )));
    }
    if fragments.is_empty() {
        return Err(failure("No firmware payload fragments found"));
    }
    let covered: usize = fragments.iter().map(|part| part.length).sum();
    if covered != bytes.len() - payload_offset {
        return Err(failure("DATA fragment plan did not cover full SFI payload"));
    }
    Ok(SfiPlan {
        header,
        payload_offset,
        embedded_commands,
        aligned_groups,
        data_fragments: fragments,
        boot_parameters,
    })
}

#[derive(Debug)]
struct DdcRecord {
    offset: usize,
    id: u16,
    value_length: usize,
}

fn parse_ddc(bytes: &[u8]) -> Result<Vec<DdcRecord>> {
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return Err(failure(format!("Invalid DDC size {} bytes", bytes.len())));
    }
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        // Intel DDC: [length (ID + value)][2-byte ID][value...]
        let payload_len = usize::from(bytes[offset]);
        let total = payload_len + 1;
        if payload_len < 2 || total > bytes.len() - offset {
            return Err(failure(format!("Malformed DDC record at offset {offset}")));
        }
        let id = u16::from_le_bytes([bytes[offset + 1], bytes[offset + 2]]);
        records.push(DdcRecord {
            offset,
            id,
            value_length: payload_len - 2,
        });
        offset += total;
    }
    Ok(records)
}

fn read_file(path: &str, label: &str) -> Result<Vec<u8>> {
    let bytes = fs::read(path).map_err(|error| {
        failure(format!("Cannot read {label} '{}': {error}", Path::new(path).display()))
    })?;
    Ok(bytes)
}

pub fn inspect(sfi_path: &str, ddc_path: Option<&str>) -> Result<()> {
    let sfi = read_file(sfi_path, "SFI")?;
    let plan = plan_sfi(&sfi)?;

    println!("BE200AudioHub R1.5a — OFFLINE firmware inspection");
    println!("SFI: {}", Path::new(sfi_path).display());
    println!("  bytes: {}", sfi.len());
    println!("  CSS/header: {}", plan.header.label());
    println!("  embedded-command start: {}", plan.payload_offset);
    println!("  embedded HCI commands: {}", plan.embedded_commands);
    println!("  aligned payload groups: {}", plan.aligned_groups);
    println!("  planned Secure Send DATA fragments: {}", plan.data_fragments.len());
    let payload_bytes: usize = plan.data_fragments.iter().map(|fragment| fragment.length).sum();
    println!("  planned DATA bytes: {payload_bytes}");
    if let (Some(first), Some(last)) =
        (plan.data_fragments.first(), plan.data_fragments.last())
    {
        println!("  DATA fragment span: [{}, {})",
            first.offset, last.offset + last.length);
    }

    if let Some(boot) = plan.boot_parameters {
        println!("  embedded boot address: 0x{:08X}", boot.address);
        println!("  firmware build: {} week {} year {}",
            boot.build, boot.week, u16::from(boot.year_since_2000) + 2000);
    } else {
        println!("  embedded boot parameters: not found (requires further review)");
    }

    // The selected secure boot engine (SBE) is a controller property.
    // A dual RSA/ECDSA header alone does not establish which fragments to
    // transmit. R1.5a intentionally does not generate or execute wire writes.
    println!("  SBE/header-send selection: NOT PERFORMED");
    println!("  cryptographic signature verification: NOT PERFORMED");

    if let Some(path) = ddc_path {
        let ddc = read_file(path, "DDC")?;
        let records = parse_ddc(&ddc)?;
        println!("DDC: {}", Path::new(path).display());
        println!("  bytes: {}", ddc.len());
        println!("  records: {}", records.len());
        for record in &records {
            println!("  DDC offset={} id=0x{:04X} value_bytes={}",
                record.offset, record.id, record.value_length);
        }
    }
    println!("USB/HCI activity: NONE. Firmware write: NONE.");
    log::info!(target: "be200hub::firmware",
        "offline SFI parsed: {} bytes, {} HCI commands, {} DATA fragments",
        sfi.len(), plan.embedded_commands, plan.data_fragments.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dual_header() -> Vec<u8> {
        let mut sfi = vec![0u8; 964];
        sfi[8..12].copy_from_slice(&CSS_RSA.to_le_bytes());
        sfi[644] = 0x06;
        sfi[652..656].copy_from_slice(&CSS_ECDSA.to_le_bytes());
        // 0xFC0E with 7 data bytes: 10 bytes total.
        sfi.extend_from_slice(&[
            0x0e, 0xfc, 7, 0x00, 0x08, 0x10, 0x00, 0x91, 0x24, 0x1a,
        ]);
        // Second command, 6 bytes total, makes the group 4-byte aligned.
        sfi.extend_from_slice(&[0x02, 0xfc, 3, 0, 0, 0]);
        sfi
    }

    #[test]
    fn parses_dual_header_and_embedded_boot_address() {
        let plan = plan_sfi(&sample_dual_header()).unwrap();
        assert_eq!(plan.header, HeaderKind::RsaEcdsa);
        assert_eq!(plan.payload_offset, 964);
        assert_eq!(plan.embedded_commands, 2);
        assert_eq!(plan.aligned_groups, 1);
        assert_eq!(plan.data_fragments, vec![DataFragment {
            offset: 964, length: 16,
        }]);
        assert_eq!(plan.boot_parameters.unwrap().address, 0x0010_0800);
    }

    #[test]
    fn rejects_truncated_and_misaligned_payloads() {
        let mut truncated = sample_dual_header();
        truncated.extend_from_slice(&[0x00, 0x01]);
        assert!(plan_sfi(&truncated).is_err());

        let mut misaligned = sample_dual_header();
        misaligned.extend_from_slice(&[0x03, 0xfc, 0]);
        assert!(plan_sfi(&misaligned).is_err());
    }

    #[test]
    fn parses_tiny_be200_ddc_record() {
        let records = parse_ddc(&[0x03, 0x28, 0x01, 0x18]).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 0x0128);
        assert_eq!(records[0].value_length, 1);
        assert!(parse_ddc(&[0x04, 0x28, 0x01, 0x18]).is_err());
    }
}
