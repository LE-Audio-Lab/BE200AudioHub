use std::{fs, ops::Range, path::Path};

use sha2::{Digest, Sha256};

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const RSA_HEADER_LEN: usize = 644;
const ECDSA_HEADER_LEN: usize = 320;
const PAYLOAD_OFFSET: usize = RSA_HEADER_LEN + ECDSA_HEADER_LEN;
const CSS_RSA: u32 = 0x0001_0000;
const CSS_ECDSA: u32 = 0x0002_0000;
const MAX_SECURE_SEND_DATA: usize = 252;

pub const SFI_SHA256: &str =
    "939a9b32ad11d8962abfd10cb24d5bd90d7848218af4ea438cf6490243a618d4";
pub const DDC_SHA256: &str =
    "e0fb10fe29f60607d6507ced3f697492d9611f62c4765feaee59c4bd4101d3e7";
pub const CNVI_TOP: u32 = 0x0200_1910;
pub const CNVR_TOP: u32 = 0x0200_1910;
pub const HW_VARIANT: u8 = 0x1c;
pub const SBE_ECDSA: u8 = 0x01;
pub const BOOT_ADDRESS: u32 = 0x0010_0800;
pub const PREBOOT_DDC: [u8; 4] = [0x03, 0x86, 0x01, 0x01];
pub const DSBR_DEFAULT: [u8; 2] = [0x00, 0x00];

const EXPECTED_EMBEDDED_COMMANDS: usize = 3_967;
const EXPECTED_DATA_GROUPS: usize = 3_957;
const EXPECTED_SECURE_FRAGMENTS: usize = 3_960;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareError(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecureFragment {
    pub kind: u8,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct PinnedFirmware {
    pub sfi: Vec<u8>,
    pub ddc: Vec<u8>,
    pub fragments: Vec<SecureFragment>,
    pub boot_address: u32,
}

#[derive(Debug)]
struct Plan {
    fragments: Vec<SecureFragment>,
    boot_address: u32,
    embedded_commands: usize,
    data_groups: usize,
    build: u8,
    week: u8,
    year_since_2000: u8,
}

pub fn load_pinned(sfi_path: &Path, ddc_path: &Path) -> Result<PinnedFirmware, FirmwareError> {
    let sfi = fs::read(sfi_path)
        .map_err(|error| FirmwareError(format!("cannot read SFI: {error}")))?;
    let ddc = fs::read(ddc_path)
        .map_err(|error| FirmwareError(format!("cannot read DDC: {error}")))?;
    verify_pinned(sfi, ddc)
}

fn verify_pinned(sfi: Vec<u8>, ddc: Vec<u8>) -> Result<PinnedFirmware, FirmwareError> {
    if sha256_hex(&sfi) != SFI_SHA256 {
        return Err(FirmwareError("SFI hash mismatch".into()));
    }
    if sha256_hex(&ddc) != DDC_SHA256 {
        return Err(FirmwareError("DDC hash mismatch".into()));
    }
    if ddc.as_slice() != [0x03, 0x28, 0x01, 0x18] {
        return Err(FirmwareError("unexpected DDC record".into()));
    }

    let plan = plan_sfi(&sfi)?;
    if plan.boot_address != BOOT_ADDRESS
        || plan.embedded_commands != EXPECTED_EMBEDDED_COMMANDS
        || plan.data_groups != EXPECTED_DATA_GROUPS
        || plan.fragments.len() != EXPECTED_SECURE_FRAGMENTS
        || (plan.build, plan.week, plan.year_since_2000) != (176, 26, 26)
    {
        return Err(FirmwareError("pinned firmware profile mismatch".into()));
    }

    Ok(PinnedFirmware {
        sfi,
        ddc,
        fragments: plan.fragments,
        boot_address: plan.boot_address,
    })
}

fn plan_sfi(bytes: &[u8]) -> Result<Plan, FirmwareError> {
    if bytes.len() <= PAYLOAD_OFFSET || bytes.len() > MAX_IMAGE_BYTES {
        return Err(FirmwareError("invalid SFI size".into()));
    }
    if read_u32(bytes, 8)? != CSS_RSA
        || bytes.get(RSA_HEADER_LEN).copied() != Some(0x06)
        || read_u32(bytes, RSA_HEADER_LEN + 8)? != CSS_ECDSA
    {
        return Err(FirmwareError("expected RSA+ECDSA SFI header".into()));
    }

    let mut cursor = PAYLOAD_OFFSET;
    let mut group_start = cursor;
    let mut group_len = 0usize;
    let mut commands = 0usize;
    let mut groups = 0usize;
    let mut data_fragments = Vec::new();
    let mut boot = None;

    while cursor < bytes.len() {
        let header = bytes
            .get(cursor..cursor + 3)
            .ok_or_else(|| FirmwareError("truncated embedded HCI header".into()))?;
        let opcode = u16::from_le_bytes([header[0], header[1]]);
        let command_len = 3 + usize::from(header[2]);
        let end = cursor
            .checked_add(command_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| FirmwareError("embedded HCI command exceeds SFI".into()))?;

        if opcode == 0xfc0e {
            if boot.is_some() || header[2] < 7 {
                return Err(FirmwareError("invalid Intel boot parameters".into()));
            }
            boot = Some((
                read_u32(bytes, cursor + 3)?,
                bytes[cursor + 7],
                bytes[cursor + 8],
                bytes[cursor + 9],
            ));
        }

        commands += 1;
        group_len += command_len;
        cursor = end;

        if group_len % 4 == 0 {
            let mut sent = 0usize;
            while sent < group_len {
                let length = (group_len - sent).min(MAX_SECURE_SEND_DATA);
                data_fragments.push(SecureFragment {
                    kind: 0x01,
                    span: group_start + sent..group_start + sent + length,
                });
                sent += length;
            }
            groups += 1;
            group_start = cursor;
            group_len = 0;
        }
    }

    if group_len != 0 {
        return Err(FirmwareError("unaligned SFI payload tail".into()));
    }

    let (boot_address, build, week, year_since_2000) =
        boot.ok_or_else(|| FirmwareError("missing Intel boot parameters".into()))?;

    let mut fragments = vec![
        SecureFragment { kind: 0x00, span: 644..772 },
        SecureFragment { kind: 0x03, span: 772..868 },
        SecureFragment { kind: 0x02, span: 868..964 },
    ];
    fragments.extend(data_fragments);

    Ok(Plan {
        fragments,
        boot_address,
        embedded_commands: commands,
        data_groups: groups,
        build,
        week,
        year_since_2000,
    })
}

pub fn secure_send_packet(kind: u8, payload: &[u8]) -> Result<Vec<u8>, FirmwareError> {
    if payload.is_empty()
        || payload.len() > MAX_SECURE_SEND_DATA
        || !matches!(kind, 0x00 | 0x01 | 0x02 | 0x03)
    {
        return Err(FirmwareError("invalid Secure Send fragment".into()));
    }

    let mut packet = Vec::with_capacity(payload.len() + 4);
    packet.extend_from_slice(&0xfc09u16.to_le_bytes());
    packet.push((payload.len() + 1) as u8);
    packet.push(kind);
    packet.extend_from_slice(payload);
    Ok(packet)
}

pub fn boot_parameters(address: u32) -> Result<[u8; 8], FirmwareError> {
    if address != BOOT_ADDRESS {
        return Err(FirmwareError("unexpected boot address".into()));
    }
    let mut params = [0x00, 0x01, 0x00, 0x01, 0, 0, 0, 0];
    params[4..].copy_from_slice(&address.to_le_bytes());
    Ok(params)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, FirmwareError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| FirmwareError("missing u32 field".into()))?;
    Ok(u32::from_le_bytes(slice.try_into().expect("four bytes")))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_sfi() -> Vec<u8> {
        let mut bytes = vec![0; PAYLOAD_OFFSET];
        bytes[8..12].copy_from_slice(&CSS_RSA.to_le_bytes());
        bytes[644] = 0x06;
        bytes[652..656].copy_from_slice(&CSS_ECDSA.to_le_bytes());
        bytes.extend_from_slice(&[
            0x0e, 0xfc, 7, 0x00, 0x08, 0x10, 0x00, 176, 26, 26,
            0x02, 0xfc, 3, 0, 0, 0,
        ]);
        bytes
    }

    #[test]
    fn planner_keeps_captured_ecdsa_header_order() {
        let plan = plan_sfi(&synthetic_sfi()).unwrap();
        assert_eq!(plan.boot_address, BOOT_ADDRESS);
        assert_eq!(plan.fragments[0], SecureFragment { kind: 0x00, span: 644..772 });
        assert_eq!(plan.fragments[1], SecureFragment { kind: 0x03, span: 772..868 });
        assert_eq!(plan.fragments[2], SecureFragment { kind: 0x02, span: 868..964 });
        assert_eq!(plan.fragments[3], SecureFragment { kind: 0x01, span: 964..980 });
    }

    #[test]
    fn secure_send_wire_and_boot_parameters_are_fixed() {
        assert_eq!(
            secure_send_packet(0x01, &[0xaa, 0xbb]).unwrap(),
            vec![0x09, 0xfc, 0x03, 0x01, 0xaa, 0xbb]
        );
        assert_eq!(
            boot_parameters(BOOT_ADDRESS).unwrap(),
            [0x00, 0x01, 0x00, 0x01, 0x00, 0x08, 0x10, 0x00]
        );
    }

    #[test]
    fn pinned_verifier_rejects_wrong_bytes_before_use() {
        assert!(matches!(
            verify_pinned(synthetic_sfi(), vec![0x03, 0x28, 0x01, 0x18]),
            Err(FirmwareError(message)) if message == "SFI hash mismatch"
        ));
    }
}
