//! Intel Read Version TLV decoder. No firmware write/modify operations.
use crate::error::{AppError, Result};

#[derive(Debug, PartialEq, Eq)]
pub struct Version {
    pub cnvi_bt: Option<u32>,
    pub cnvr_bt: Option<u32>,
    pub secure_boot: Option<u8>,
    pub sbe_type: Option<u8>,
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
    let mut v = Version { image_type: None, cnvi_top: None, cnvr_top: None, cnvi_bt: None, cnvr_bt: None, secure_boot: None, sbe_type: None, tlv_count: 0 };
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
            0x12 => v.cnvi_bt = Some(read_u32_le(value, "CNVi Bluetooth")?),
            0x13 => v.cnvr_bt = Some(read_u32_le(value, "CNVr Bluetooth")?),
            0x28 if size == 1 => v.secure_boot = Some(value[0]),
            0x2f if size == 1 => v.sbe_type = Some(value[0]),
            0x28 | 0x2f => return Err(AppError::Protocol(format!("Invalid Intel security TLV 0x{kind:02X} length {size}"))),
            0x1c if size == 1 => v.image_type = Some(value[0]),
            0x1c => return Err(AppError::Protocol("Invalid image-type TLV length".into())),
            _ => {}
        }
        v.tlv_count += 1;
        cursor = end;
    }
    Ok(v)
}

impl Version {
    /// From Linux btintel.h INTEL_HW_VARIANT(cnvi_bt).
    pub fn hardware_variant(&self) -> Option<u8> {
        self.cnvi_bt.map(|v| ((v >> 16) & 0x3f) as u8)
    }
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
    #[test]
    fn parses_be200_bootloader_security_tlvs_from_003() {
        // Sanitized fields from 003-ColdBoot frame 1956.
        let tlv = [
            0x10,4,0x10,0x19,0x00,0x02,
            0x11,4,0x10,0x19,0x00,0x02,
            0x12,4,0x00,0x37,0x1c,0x00,
            0x1c,1,0x01,
            0x28,1,0x01,
            0x2f,1,0x01
        ];
        let parsed = parse_read_version_tlv(&tlv).unwrap();
        assert_eq!(parsed.image_type, Some(1));
        assert_eq!(parsed.hardware_variant(), Some(0x1c));
        assert_eq!(parsed.sbe_type, Some(1));
        assert_eq!(parsed.secure_boot, Some(1));
        assert_eq!(parsed.cnvi_top, Some(0x0200_1910));
    }

}
