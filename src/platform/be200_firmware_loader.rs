use super::be200_firmware::{
    boot_parameters, secure_send_packet, PinnedFirmware, CNVI_TOP, CNVR_TOP, DSBR_DEFAULT,
    HW_VARIANT, PREBOOT_DDC, SBE_ECDSA,
};

const READ_VERSION: u16 = 0xfc05;
const WRITE_DDC: u16 = 0xfc8b;
const BOOT: u16 = 0xfc01;
const DSBR: u16 = 0xfc0a;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntelVersion {
    pub image_type: Option<u8>,
    pub cnvi_top: Option<u32>,
    pub cnvr_top: Option<u32>,
    pub cnvi_bt: Option<u32>,
    pub secure_boot: Option<u8>,
    pub sbe_type: Option<u8>,
}

impl IntelVersion {
    pub fn hardware_variant(&self) -> Option<u8> {
        self.cnvi_bt.map(|value| ((value >> 16) & 0x3f) as u8)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirmwareLoadError {
    Io(String),
    Protocol(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareLoadOutcome {
    AlreadyOperational,
    Booted,
}

pub trait FirmwareIo {
    fn command_complete(
        &mut self,
        opcode: u16,
        parameters: &[u8],
    ) -> Result<Vec<u8>, FirmwareLoadError>;

    fn command(&mut self, opcode: u16, parameters: &[u8]) -> Result<(), FirmwareLoadError>;

    fn bulk_write(&mut self, packet: &[u8]) -> Result<(), FirmwareLoadError>;

    fn vendor_event(&mut self, subtype: u8) -> Result<Vec<u8>, FirmwareLoadError>;
}

pub fn probe_version(io: &mut impl FirmwareIo) -> Result<IntelVersion, FirmwareLoadError> {
    let bytes = io.command_complete(READ_VERSION, &[0xff])?;
    parse_version_tlv(&bytes)
}

pub fn ensure_with_loaded_firmware(
    io: &mut impl FirmwareIo,
    current: IntelVersion,
    firmware: &PinnedFirmware,
) -> Result<FirmwareLoadOutcome, FirmwareLoadError> {
    match current.image_type {
        Some(0x03) => Ok(FirmwareLoadOutcome::AlreadyOperational),
        Some(0x01) => {
            validate_bootloader(&current)?;
            boot_once(io, firmware)?;
            Ok(FirmwareLoadOutcome::Booted)
        }
        other => Err(FirmwareLoadError::Protocol(format!(
            "unsupported Intel image type {other:?}"
        ))),
    }
}

fn boot_once(
    io: &mut impl FirmwareIo,
    firmware: &PinnedFirmware,
) -> Result<(), FirmwareLoadError> {
    io.command_complete(WRITE_DDC, &PREBOOT_DDC)?;

    for fragment in &firmware.fragments {
        let packet = secure_send_packet(
            fragment.kind,
            &firmware.sfi[fragment.span.clone()],
        )
        .map_err(|error| FirmwareLoadError::Protocol(error.0))?;
        io.bulk_write(&packet)?;
    }

    validate_download_event(&io.vendor_event(0x06)?)?;

    let boot = boot_parameters(firmware.boot_address)
        .map_err(|error| FirmwareLoadError::Protocol(error.0))?;
    io.command(BOOT, &boot)?;
    validate_boot_event(&io.vendor_event(0x02)?)?;

    io.command_complete(DSBR, &DSBR_DEFAULT)?;

    let after = probe_version(io)?;
    if after.image_type != Some(0x03) {
        return Err(FirmwareLoadError::Protocol(
            "controller did not enter Operational image".into(),
        ));
    }

    io.command_complete(WRITE_DDC, &firmware.ddc)?;
    Ok(())
}

fn validate_bootloader(version: &IntelVersion) -> Result<(), FirmwareLoadError> {
    if version.image_type != Some(0x01)
        || version.cnvi_top != Some(CNVI_TOP)
        || version.cnvr_top != Some(CNVR_TOP)
        || version.hardware_variant() != Some(HW_VARIANT)
        || version.secure_boot != Some(1)
        || version.sbe_type != Some(SBE_ECDSA)
    {
        return Err(FirmwareLoadError::Protocol(
            "BE200 bootloader identity/security guard failed".into(),
        ));
    }
    Ok(())
}

fn parse_version_tlv(bytes: &[u8]) -> Result<IntelVersion, FirmwareLoadError> {
    let mut version = IntelVersion {
        image_type: None,
        cnvi_top: None,
        cnvr_top: None,
        cnvi_bt: None,
        secure_boot: None,
        sbe_type: None,
    };

    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let header = bytes
            .get(cursor..cursor + 2)
            .ok_or_else(|| FirmwareLoadError::Protocol("truncated Intel TLV header".into()))?;
        let kind = header[0];
        let length = usize::from(header[1]);
        cursor += 2;
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| FirmwareLoadError::Protocol("truncated Intel TLV value".into()))?;
        let value = &bytes[cursor..end];

        match kind {
            0x10 => version.cnvi_top = Some(read_u32(value)?),
            0x11 => version.cnvr_top = Some(read_u32(value)?),
            0x12 => version.cnvi_bt = Some(read_u32(value)?),
            0x1c if length == 1 => version.image_type = Some(value[0]),
            0x28 if length == 1 => version.secure_boot = Some(value[0]),
            0x2f if length == 1 => version.sbe_type = Some(value[0]),
            0x1c | 0x28 | 0x2f => {
                return Err(FirmwareLoadError::Protocol(
                    "invalid Intel single-byte TLV length".into(),
                ))
            }
            _ => {}
        }
        cursor = end;
    }

    Ok(version)
}

fn read_u32(bytes: &[u8]) -> Result<u32, FirmwareLoadError> {
    let value: [u8; 4] = bytes
        .try_into()
        .map_err(|_| FirmwareLoadError::Protocol("invalid Intel u32 TLV length".into()))?;
    Ok(u32::from_le_bytes(value))
}

fn validate_download_event(event: &[u8]) -> Result<(), FirmwareLoadError> {
    if event.len() != 7
        || event[..3] != [0xff, 0x05, 0x06]
        || event[3] != 0
        || event[6] != 0
    {
        return Err(FirmwareLoadError::Protocol(
            "Intel firmware download event failed".into(),
        ));
    }
    Ok(())
}

fn validate_boot_event(event: &[u8]) -> Result<(), FirmwareLoadError> {
    if event.len() != 9
        || event[..3] != [0xff, 0x07, 0x02]
        || event[3] != 0
    {
        return Err(FirmwareLoadError::Protocol(
            "Intel operational boot event failed".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::*;
    use crate::platform::be200_firmware::SecureFragment;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Action {
        Complete(u16, Vec<u8>),
        Command(u16, Vec<u8>),
        Bulk(Vec<u8>),
        Vendor(u8),
    }

    struct FakeIo {
        actions: Vec<Action>,
        version_replies: Vec<Vec<u8>>,
    }

    impl FirmwareIo for FakeIo {
        fn command_complete(
            &mut self,
            opcode: u16,
            parameters: &[u8],
        ) -> Result<Vec<u8>, FirmwareLoadError> {
            self.actions.push(Action::Complete(opcode, parameters.to_vec()));
            if opcode == READ_VERSION {
                return Ok(self.version_replies.remove(0));
            }
            Ok(Vec::new())
        }

        fn command(&mut self, opcode: u16, parameters: &[u8]) -> Result<(), FirmwareLoadError> {
            self.actions.push(Action::Command(opcode, parameters.to_vec()));
            Ok(())
        }

        fn bulk_write(&mut self, packet: &[u8]) -> Result<(), FirmwareLoadError> {
            self.actions.push(Action::Bulk(packet.to_vec()));
            Ok(())
        }

        fn vendor_event(&mut self, subtype: u8) -> Result<Vec<u8>, FirmwareLoadError> {
            self.actions.push(Action::Vendor(subtype));
            match subtype {
                0x06 => Ok(vec![0xff, 5, 6, 0, 0, 0, 0]),
                0x02 => Ok(vec![0xff, 7, 2, 0, 1, 1, 2, 0xff, 1]),
                _ => unreachable!(),
            }
        }
    }

    fn version_tlv(image: u8) -> Vec<u8> {
        vec![
            0x10, 4, 0x10, 0x19, 0x00, 0x02,
            0x11, 4, 0x10, 0x19, 0x00, 0x02,
            0x12, 4, 0x00, 0x37, 0x1c, 0x00,
            0x1c, 1, image,
            0x28, 1, 1,
            0x2f, 1, 1,
        ]
    }

    fn tiny_firmware() -> PinnedFirmware {
        PinnedFirmware {
            sfi: vec![0xaa, 0xbb],
            ddc: vec![0x03, 0x28, 0x01, 0x18],
            fragments: vec![
                SecureFragment { kind: 0x00, span: Range { start: 0, end: 1 } },
                SecureFragment { kind: 0x01, span: Range { start: 1, end: 2 } },
            ],
            boot_address: 0x0010_0800,
        }
    }

    #[test]
    fn operational_image_is_a_true_noop_after_probe() {
        let current = parse_version_tlv(&version_tlv(3)).unwrap();
        let mut io = FakeIo { actions: Vec::new(), version_replies: Vec::new() };

        assert_eq!(
            ensure_with_loaded_firmware(&mut io, current, &tiny_firmware()),
            Ok(FirmwareLoadOutcome::AlreadyOperational)
        );
        assert!(io.actions.is_empty());
    }

    #[test]
    fn one_shot_loader_preserves_minimal_captured_sequence() {
        let current = parse_version_tlv(&version_tlv(1)).unwrap();
        let mut io = FakeIo {
            actions: Vec::new(),
            version_replies: vec![version_tlv(3)],
        };

        assert_eq!(
            ensure_with_loaded_firmware(&mut io, current, &tiny_firmware()),
            Ok(FirmwareLoadOutcome::Booted)
        );

        assert_eq!(io.actions[0], Action::Complete(WRITE_DDC, PREBOOT_DDC.to_vec()));
        assert!(matches!(io.actions[1], Action::Bulk(_)));
        assert!(matches!(io.actions[2], Action::Bulk(_)));
        assert_eq!(io.actions[3], Action::Vendor(0x06));
        assert_eq!(io.actions[4], Action::Command(BOOT, vec![0, 1, 0, 1, 0, 8, 16, 0]));
        assert_eq!(io.actions[5], Action::Vendor(0x02));
        assert_eq!(io.actions[6], Action::Complete(DSBR, DSBR_DEFAULT.to_vec()));
        assert_eq!(io.actions[7], Action::Complete(READ_VERSION, vec![0xff]));
        assert_eq!(io.actions[8], Action::Complete(WRITE_DDC, vec![0x03, 0x28, 0x01, 0x18]));
    }

    #[test]
    fn bootloader_guard_rejects_wrong_hardware_before_writes() {
        let mut current = parse_version_tlv(&version_tlv(1)).unwrap();
        current.cnvi_top = Some(0);
        let mut io = FakeIo { actions: Vec::new(), version_replies: Vec::new() };

        assert!(ensure_with_loaded_firmware(&mut io, current, &tiny_firmware()).is_err());
        assert!(io.actions.is_empty());
    }
}
