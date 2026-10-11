//! Intel Read Version TLV decoder. No firmware write/modify operations.
use crate::error::{AppError, Result};

#[derive(Debug, PartialEq, Eq)]
pub struct Version {
    pub image_type: Option<u8>,
    pub cnvi_top: Option<u32>,
    pub cnvr_top: Option<u32>,
    pub tlv_count: usize,
}

fn read_u32_le(bytes: &[u8], name: &str) -> Result<u32> {
    let four: [u8; 4] = bytes.try_into()
        .map_err(|_| AppError::Protocol(format!("Invalid {name} TLV length: {}", bytes.len())))?;
    Ok(u32::from_le_bytes(four))
}

/// Parse Intel vendor Return Parameters *after* the Command Complete status byte.
/// TLV format is [tag: u8, len: u8, value: len bytes].
pub fn parse_read_version_tlv(bytes: &[u8]) -> Result<Version> {
    let mut v = Version { image_type: None, cnvi_top: None, cnvr_top: None, tlv_count: 0 };
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes.len() - cursor < 2 {
            return Err(AppError::Protocol("Truncated Intel TLV header".into()));
        }
        let kind = bytes[cursor];
        let size = bytes[cursor + 1] as usize;
        cursor += 2;
        let Some(end) = cursor.checked_add(size) else {
            return Err(AppError::Protocol("Intel TLV length overflow".into()));
        };
        if end > bytes.len() {
            return Err(AppError::Protocol(format!(
                "Truncated Intel TLV 0x{kind:02X}: claims {size} bytes, only {} available",
                bytes.len() - cursor
            )));
        }
        let value = &bytes[cursor..end];
        match kind {
            0x10 => v.cnvi_top = Some(read_u32_le(value, "CNVi TOP")?),
            0x11 => v.cnvr_top = Some(read_u32_le(value, "CNVr TOP")?),
            0x1c if size == 1 => v.image_type = Some(value[0]),
            0x1c => return Err(AppError::Protocol("Invalid image-type TLV length".into())),
            _ => {}
        }
        v.tlv_count += 1;
        cursor = end;
    }
    Ok(v)
}

pub fn image_description(value: u8) -> &'static str {
    match value {
        0x01 => "Bootloader",
        0x03 => "Operational",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_be200_observed_version_fields() {
        // Subset of the actual 002 Port Cycle (0xFC05) return TLVs.
        let tlv = [
            0x10, 0x04, 0x10, 0x19, 0x00, 0x02,
            0x11, 0x04, 0x10, 0x19, 0x00, 0x02,
            0x1c, 0x01, 0x03,
        ];
        let v = parse_read_version_tlv(&tlv).unwrap();
        assert_eq!(v.cnvi_top, Some(0x02001910));
        assert_eq!(v.cnvr_top, Some(0x02001910));
        assert_eq!(v.image_type, Some(0x03));
        assert_eq!(v.tlv_count, 3);
    }

    #[test]
    fn rejects_truncated_tlv() {
        assert!(parse_read_version_tlv(&[0x10, 0x04, 0x10]).is_err());
        assert!(parse_read_version_tlv(&[0x1c]).is_err());
        assert!(parse_read_version_tlv(&[0x1c, 0x02, 0x01, 0x00]).is_err());
    }
}
