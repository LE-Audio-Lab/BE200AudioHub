//! R1.6a: operational HCI, LE controller capabilities and ISO buffer inquiry.
//! Only allowlisted read-only HCI commands are sent (no Reset or radio scan).
use crate::{error::{AppError, Result}, hci_wire::HciSession, intel};

const READ_LOCAL_VERSION: u16 = 0x1001;
const READ_SUPPORTED_COMMANDS: u16 = 0x1002;
const LE_READ_FEATURES: u16 = 0x2003;
const LE_READ_STATES: u16 = 0x201c;
const LE_READ_BUFFER_V2: u16 = 0x2060;

fn bad(message: impl Into<String>) -> AppError { AppError::Protocol(message.into()) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalVersion {
    hci_version: u8,
    hci_revision: u16,
    manufacturer: u16,
    lmp_version: u8,
    lmp_subversion: u16,
}
fn parse_version(data: &[u8]) -> Result<LocalVersion> {
    if data.len() != 8 {
        return Err(bad(format!("Read Local Version returned {} bytes, expected 8", data.len())));
    }
    Ok(LocalVersion {
        hci_version: data[0],
        hci_revision: u16::from_le_bytes([data[1], data[2]]),
        lmp_version: data[3],
        manufacturer: u16::from_le_bytes([data[4], data[5]]),
        lmp_subversion: u16::from_le_bytes([data[6], data[7]]),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BufferV2 {
    le_acl_len: u16,
    le_acl_packets: u8,
    iso_len: u16,
    iso_packets: u8,
}
fn parse_buffer_v2(data: &[u8]) -> Result<BufferV2> {
    if data.len() != 6 {
        return Err(bad(format!("LE Buffer Size V2 returned {} bytes, expected 6", data.len())));
    }
    Ok(BufferV2 {
        le_acl_len: u16::from_le_bytes([data[0], data[1]]),
        le_acl_packets: data[2],
        iso_len: u16::from_le_bytes([data[3], data[4]]),
        iso_packets: data[5],
    })
}
fn parse_u64(data: &[u8], name: &str) -> Result<u64> {
    let data: [u8; 8] = data.try_into()
        .map_err(|_| bad(format!("{name}: expected exactly 8 bytes, got {}", data.len())))?;
    Ok(u64::from_le_bytes(data))
}
fn bit(mask: u64, position: u8) -> bool { (mask & (1u64 << position)) != 0 }

/// Intel's 0xFC05 is read-only and already verified in R1.4. Never rely
/// on Windows Bluetooth Settings to infer whether firmware is operational.
pub fn ensure_operational(hci: &mut HciSession) -> Result<()> {
    let tlvs = hci.command_complete(0xfc05, &[0xff])?;
    let version = intel::parse_read_version_tlv(&tlvs)?;
    if version.image_type != Some(3) {
        return Err(bad(format!(
            "Controller not Operational (Intel image {:?}); run the separately guarded firmware procedure",
            version.image_type
        )));
    }
    Ok(())
}

pub fn extended_scan_supported(commands: &[u8]) -> Result<bool> {
    if commands.len() != 64 { return Err(bad("HCI Supported Commands requires 64 bytes")); }
    // Bluetooth Core Vol 4 Part E, Supported Commands octet 37 bits 5 & 6.
    Ok((commands[37] & 0x60) == 0x60)
}

pub fn read_supported_commands(hci: &mut HciSession) -> Result<Vec<u8>> {
    let commands = hci.command_complete(READ_SUPPORTED_COMMANDS, &[])?;
    extended_scan_supported(&commands)?;
    Ok(commands)
}

pub fn query() -> Result<()> {
    let mut hci = HciSession::open()?;
    let result = (|| -> Result<()> {
        ensure_operational(&mut hci)?;
        let local = parse_version(&hci.command_complete(READ_LOCAL_VERSION, &[])?)?;
        let supported = read_supported_commands(&mut hci)?;
        let features = parse_u64(&hci.command_complete(LE_READ_FEATURES, &[])?, "LE Features")?;
        let states = parse_u64(&hci.command_complete(LE_READ_STATES, &[])?, "LE Supported States")?;
        let buffer = parse_buffer_v2(&hci.command_complete(LE_READ_BUFFER_V2, &[])?)?;

        println!("BE200AudioHub R1.6a — independent HCI capabilities");
        println!("  HCI version: 0x{:02X}, revision: 0x{:04X}", local.hci_version, local.hci_revision);
        println!("  LMP version: 0x{:02X}, subversion: 0x{:04X}", local.lmp_version, local.lmp_subversion);
        println!("  Controller manufacturer: {}", local.manufacturer);
        println!("  LE Features (raw): 0x{features:016X}");
        println!("    LE 2M PHY: {}", bit(features, 8));
        println!("    LE Extended Advertising: {}", bit(features, 12));
        println!("    CIS Central: {}", bit(features, 28));
        println!("    CIS Peripheral: {}", bit(features, 29));
        println!("    ISO Broadcaster: {}", bit(features, 30));
        println!("    Synchronized Receiver: {}", bit(features, 31));
        println!("  LE Supported States: 0x{states:016X}");
        println!("  Extended Scan Parameters/Enable commands: {}",
            extended_scan_supported(&supported)?);
        println!("  LE ACL buffer: {} bytes x {} packets", buffer.le_acl_len, buffer.le_acl_packets);
        println!("  LE ISO buffer: {} bytes x {} packets", buffer.iso_len, buffer.iso_packets);
        if !bit(features, 28) || buffer.iso_len == 0 || buffer.iso_packets == 0 {
            println!("  WARNING: no confirmed usable CIS Central / HCI ISO transport capacity.");
        }
        println!("  No scanning, connections, controller reset, or firmware writes.");
        log::info!(target:"be200hub::hci", "capabilities: CIS Central={} ISO len={} packets={}",
            bit(features,28), buffer.iso_len, buffer.iso_packets);
        Ok(())
    })();
    hci.finish(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_and_iso_buffer_decode() {
        let v = parse_version(&[0x0c,0x34,0x12,0x0c,0x02,0x00,0x78,0x56]).unwrap();
        assert_eq!(v.manufacturer, 2);
        assert_eq!(v.hci_revision, 0x1234);
        let b = parse_buffer_v2(&[0xfb,0,12,0xf8,0,8]).unwrap();
        assert_eq!((b.le_acl_len,b.le_acl_packets,b.iso_len,b.iso_packets), (251,12,248,8));
    }
    #[test]
    fn supported_commands_scan_bits() {
        let mut bits = [0u8;64];
        assert!(!extended_scan_supported(&bits).unwrap());
        bits[37] = 0x60;
        assert!(extended_scan_supported(&bits).unwrap());
        assert!(extended_scan_supported(&bits[..63]).is_err());
    }
    #[test]
    fn features_bit_positions_and_truncation() {
        let features = (1u64 << 28) | (1u64 << 12);
        assert!(bit(features,28));
        assert!(!bit(features,29));
        assert_eq!(parse_u64(&features.to_le_bytes(), "features").unwrap(), features);
        assert!(parse_u64(&[0;7], "features").is_err());
        assert!(parse_buffer_v2(&[0;5]).is_err());
    }
}
