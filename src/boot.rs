//! Intel BE200 R1.5b — one guarded cold-boot firmware load.
//! Transport behavior grounded in USBPcap experiment 003 + Linux btintel/btusb.
//! This module intentionally has NO arbitrary HCI command injector and NO reset-to-bootloader.
use crate::{driver, error::{AppError, Result}, firmware, intel, usb};
use rusb::{DeviceHandle, GlobalContext};
use std::time::{Duration, Instant};

const CTRL_TIMEOUT: Duration = Duration::from_millis(1500);
const BULK_TIMEOUT: Duration = Duration::from_millis(2000);
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const RX_SLICE: Duration = Duration::from_millis(250);
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(60);
const MAX_IGNORED_EVENTS: usize = 128;

fn fail(message: impl Into<String>) -> AppError {
    AppError::Protocol(message.into())
}

/// HCI event receive framing survives short USB reads and concatenated events.
/// No packet bytes are logged by default (Bluetooth address/host identity privacy).
struct IntelTransport {
    handle: DeviceHandle<GlobalContext>,
    pending: Vec<u8>,
}

impl IntelTransport {
    fn new(handle: DeviceHandle<GlobalContext>) -> Self {
        Self { handle, pending: Vec::new() }
    }

    fn command(&self, opcode: u16, params: &[u8]) -> Result<()> {
        if params.len() > 255 {
            return Err(fail("HCI command parameters too large"));
        }
        let mut bytes = Vec::with_capacity(3 + params.len());
        bytes.extend_from_slice(&opcode.to_le_bytes());
        bytes.push(params.len() as u8);
        bytes.extend_from_slice(params);
        let written = self.handle.write_control(0x20, 0, 0, 0, &bytes, CTRL_TIMEOUT)?;
        if written != bytes.len() {
            return Err(fail(format!(
                "Short HCI control write for 0x{opcode:04X}: {written}/{}",
                bytes.len()
            )));
        }
        Ok(())
    }

    fn next_event(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut read_buf = [0u8; 260];
        loop {
            if self.pending.len() >= 2 {
                let expected = usize::from(self.pending[1]) + 2;
                if self.pending.len() >= expected {
                    return Ok(self.pending.drain(..expected).collect());
                }
            }
            if self.pending.len() > 1024 {
                return Err(fail("HCI event receive overflow (1024-byte limit)"));
            }
            if Instant::now() >= deadline {
                return Err(fail(format!(
                    "Timed out waiting for HCI event after {}ms",
                    timeout.as_millis()
                )));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let slice = remaining.min(RX_SLICE);
            match self.handle.read_interrupt(0x81, &mut read_buf, slice) {
                Ok(n) if n > 0 => self.pending.extend_from_slice(&read_buf[..n]),
                Ok(_) | Err(rusb::Error::Timeout) => {},
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn wait_complete(&mut self, opcode: u16) -> Result<Vec<u8>> {
        let deadline = Instant::now() + EVENT_TIMEOUT;
        let mut ignored = 0;
        while Instant::now() < deadline {
            let event = self.next_event(deadline.saturating_duration_since(Instant::now()))?;
            if event[0] == 0x0e && event.len() >= 6 {
                let actual = u16::from_le_bytes([event[3], event[4]]);
                if actual == opcode {
                    if event[5] != 0 {
                        return Err(fail(format!(
                            "HCI Command Complete 0x{opcode:04X} status 0x{:02X}",
                            event[5]
                        )));
                    }
                    return Ok(event[6..].to_vec());
                }
            }
            // Command Status for this command is an error if nonzero.
            if event[0] == 0x0f && event.len() >= 6 {
                let actual = u16::from_le_bytes([event[4], event[5]]);
                if actual == opcode && event[2] != 0 {
                    return Err(fail(format!(
                        "HCI Command Status 0x{opcode:04X} failed: 0x{:02X}",
                        event[2]
                    )));
                }
            }
            ignored += 1;
            if ignored > MAX_IGNORED_EVENTS {
                return Err(fail("Too many unrelated HCI events while awaiting Command Complete"));
            }
        }
        Err(fail(format!("No Command Complete for 0x{opcode:04X}")))
    }

    fn read_version(&mut self) -> Result<intel::Version> {
        self.command(0xfc05, &[0xff])?;
        let parameters = self.wait_complete(0xfc05)?;
        intel::parse_read_version_tlv(&parameters)
    }

    fn secure_send(&self, kind: u8, content: &[u8]) -> Result<()> {
        let frame = firmware::secure_send_packet(kind, content)?;
        let sent = self.handle.write_bulk(0x02, &frame, BULK_TIMEOUT)?;
        if sent != frame.len() {
            return Err(fail(format!(
                "Short Intel 0xFC09 Bulk OUT: sent {sent}/{} bytes",
                frame.len()
            )));
        }
        Ok(())
    }

    fn wait_vendor(&mut self, subtype: u8) -> Result<Vec<u8>> {
        let deadline = Instant::now() + EVENT_TIMEOUT;
        let mut ignored = 0usize;
        while Instant::now() < deadline {
            let event = self.next_event(deadline.saturating_duration_since(Instant::now()))?;
            if event[0] == 0xff && event.len() >= 3 {
                match event[2] {
                    0x06 => {
                        let outcome = parse_download_event(&event)?;
                        if outcome != (0,0) {
                            return Err(fail(format!(
                                "Intel Secure Send failure result/status 0x{:02X}/0x{:02X}",
                                outcome.0,outcome.1
                            )));
                        }
                        if subtype == 0x06 { return Ok(event); }
                        // Unexpected duplicate completion before boot done; do not
                        // mistake it for the operational boot notification.
                    },
                    0x02 if subtype == 0x02 => {
                        parse_boot_event(&event)?;
                        return Ok(event);
                    },
                    _ => {}
                }
            }
            ignored += 1;
            if ignored > MAX_IGNORED_EVENTS {
                return Err(fail("Too many unrelated events while awaiting Intel vendor event"));
            }
        }
        Err(fail(format!("Intel event 0xFF/0x{subtype:02X} not received")))
    }

    fn release(self, outcome: Result<()>) -> Result<()> {
        let result = self.handle.release_interface(0);
        match (outcome, result) {
            (Err(e), release) => {
                if let Err(usb_error) = release {
                    eprintln!("Warning: failed to release BE200 interface 0: {usb_error}");
                }
                Err(e)
            }
            (Ok(()), Err(e)) => Err(e.into()),
            (Ok(()), Ok(())) => {
                println!("Release interface 0: OK");
                Ok(())
            }
        }
    }
}

fn parse_download_event(event: &[u8]) -> Result<(u8,u8)> {
    // Event 003 frame 9908: FF 05 06 00 00 00 00
    // Result structure after subtype: result, opcode (u16 LE), status.
    if event.len() != 7 || event[0] != 0xff || event[1] != 5 || event[2] != 6 {
        return Err(fail("Malformed Intel 0xFF/0x06 firmware download event"));
    }
    let _opcode = u16::from_le_bytes([event[4], event[5]]);
    Ok((event[3],event[6]))
}

fn parse_boot_event(event: &[u8]) -> Result<()> {
    // Event 003 frame 9916: FF 07 02 00 01 01 02 FF 01
    if event.len() != 9 || event[0] != 0xff || event[1] != 7 || event[2] != 2 {
        return Err(fail("Malformed Intel 0xFF/0x02 operational boot event"));
    }
    if event[3] != 0 {
        return Err(fail(format!("Intel boot event unexpected leading byte 0x{:02X}",event[3])));
    }
    Ok(())
}

fn validate_bootloader(v: &intel::Version) -> Result<()> {
    if v.image_type != Some(0x01) {
        return Err(fail(format!(
            "Refusing firmware transfer: controller is {} (image {:?}), NOT Bootloader 0x01",
            v.image_type.map(intel::image_description).unwrap_or("unidentified"),
            v.image_type
        )));
    }
    if v.cnvi_top != Some(firmware::BE200_CNVI_TOP)
       || v.cnvr_top != Some(firmware::BE200_CNVR_TOP)
       || v.hardware_variant() != Some(firmware::BE200_HW_VARIANT) {
        return Err(fail(format!(
            "Refusing firmware transfer: BE200 0291/0291 variant mismatch: CNVi={:?}, CNVr={:?}, hw_variant={:?}",
            v.cnvi_top, v.cnvr_top, v.hardware_variant()
        )));
    }
    if v.sbe_type != Some(firmware::BE200_SBE_ECDSA) || v.secure_boot != Some(1) {
        return Err(fail(format!(
            "Refusing firmware transfer: expected secure_boot=1,SBE=ECDSA(1); got secure_boot={:?},SBE={:?}",
            v.secure_boot, v.sbe_type
        )));
    }
    Ok(())
}

/// Read-only firmware/controller compatibility check. Does not touch power
/// and does not send Secure Send, preboot DDC, boot or reset opcodes.
pub fn preflight(sfi_path: &str, ddc_path: &str) -> Result<()> {
    let fw = firmware::load_pinned_be200_pair(sfi_path, ddc_path)?;
    driver::verify()?;
    println!("R1.5b Windows OEM firmware preflight (READ ONLY)");
    println!("  Profile: {}", firmware::BE200_WINDOWS_FW_PROFILE);
    println!("  Secure Send replay: {} packets, {} bytes, SHA256 {}",
        fw.fragments.len(), fw.replay_bytes, fw.replay_sha256);
    let mut controller = IntelTransport::new(usb::open_be200_claimed()?);
    let outcome = (|| -> Result<()> {
        let version = controller.read_version()?;
        println!("  Live image: {:?} ({})", version.image_type,
            version.image_type.map(intel::image_description).unwrap_or("unknown"));
        println!("  CNVi TOP: {:?}, CNVr TOP: {:?}, HW variant: {:?}",
            version.cnvi_top, version.cnvr_top, version.hardware_variant());
        println!("  Secure Boot: {:?}, SBE type: {:?}",
            version.secure_boot, version.sbe_type);
        if version.image_type == Some(3) {
            println!("  Result: ALREADY OPERATIONAL. No firmware download is needed now.");
            println!("  Cold-boot experiment requires separately controlled BE200 power and a new read-only preflight.");
            return Ok(());
        }
        validate_bootloader(&version)?;
        println!("  Result: BOOTLOADER READY — matching BE200, signed-image profile and ECDSA SBE.");
        println!("  Firmware has NOT been sent. Use the separately confirmed one-shot boot command only after operator approval.");
        Ok(())
    })();
    controller.release(outcome)
}

/// First *hardware* test is always an explicit operator action. No automatic
/// USB reset, brownout, downgrade or retry. Once an error happens, abort.
pub fn boot_once(sfi_path: &str, ddc_path: &str) -> Result<()> {
    let fw = firmware::load_pinned_be200_pair(sfi_path, ddc_path)?;
    // Confirm PnP service WinUSB, exact BE200, before opening hardware.
    driver::verify()?;

    println!("R1.5b — BE200 Windows OEM cold boot, exactly ONE attempt");
    println!("Profile: {}", firmware::BE200_WINDOWS_FW_PROFILE);
    println!("Pinned SFI: {}", firmware::BE200_SFI_SHA256);
    println!("003-ColdBoot replay SHA256: {} ({} bytes)",
        fw.replay_sha256, fw.replay_bytes);
    println!("Firmware fragments: {} (ECDSA header: 3, DATA: {})",
        fw.fragments.len(), fw.fragments.len() - 3);
    println!("No hardware power control, no retry and no reset-to-bootloader.");
    let mut usb = IntelTransport::new(usb::open_be200_claimed()?);
    let outcome = (|| -> Result<()> {
        let before = usb.read_version()?;
        println!("Before: Intel image={:?}, CNVi TOP={:?}, CNVr TOP={:?}, variant={:?}, secure_boot={:?}, sbe_type={:?}",
            before.image_type, before.cnvi_top, before.cnvr_top,
            before.hardware_variant(), before.secure_boot, before.sbe_type);
        validate_bootloader(&before)?;
        // 003-ColdBoot frame 1957: Windows writes DDC id 0x0186
        // before beginning firmware Secure Send. It returned HCI success.
        println!("Bootloader / hardware / ECDSA checks passed.");
        usb.command(0xfc8b, &firmware::BE200_WINDOWS_PREBOOT_DDC)?;
        usb.wait_complete(0xfc8b)?;
        println!("Windows preboot DDC 0x0186 accepted. Starting Secure Send...");

        let started = Instant::now();
        for (index, part) in fw.fragments.iter().enumerate() {
            if started.elapsed() > DOWNLOAD_DEADLINE {
                return Err(fail("Firmware transfer exceeded 60-second global deadline. No automatic retry."));
            }
            usb.secure_send(part.kind, &fw.sfi[part.span.clone()])?;
            if (index + 1) % 500 == 0 {
                println!("  Secure Send: {}/{} fragments", index+1, fw.fragments.len());
            }
        }
        println!("  Secure Send: {}/{} fragments completed at USB transport",
            fw.fragments.len(), fw.fragments.len());
        println!("Waiting for Intel 0xFF/0x06 firmware result...");
        usb.wait_vendor(0x06)?;
        println!("Firmware download completion: accepted by controller.");

        // This command has no normal Command Complete in Intel bootloader
        // mode; the 0xFF/0x02 vendor event is authoritative instead.
        let reset = firmware::boot_command(fw.boot_address)?;
        usb.command(0xfc01, &reset)?;
        println!("Boot requested at 0x{:08X}; waiting for Intel 0xFF/0x02...",
            fw.boot_address);
        usb.wait_vendor(0x02)?;
        println!("Operational boot event received.");

        // Windows 003 frame 9917: Intel DSBR default 0xFC0A 00 00.
        usb.command(0xfc0a, &firmware::BE200_WINDOWS_DSBR_DEFAULT)?;
        usb.wait_complete(0xfc0a)?;
        println!("Intel DSBR defaults applied (0xFC0A).");

        let after = usb.read_version()?;
        if after.image_type != Some(0x03) {
            return Err(fail(format!(
                "Firmware boot event arrived, but Intel Read Version image={:?} (expected Operational 0x03)",
                after.image_type
            )));
        }
        println!("BE200 Operational Firmware confirmed by HCI 0xFC05.");

        // DDC is a normal HCI command once operational.
        // Linux btintel treats DDC as optional, but report any error
        // instead of silently declaring a full successful bring-up.
        usb.command(0xfc8b, &fw.ddc)?;
        usb.wait_complete(0xfc8b)?;
        println!("Intel 0xFC8B DDC record applied.");
        println!("R1.5b RESULT: independent Windows OEM Bootloader -> Operational boot succeeded.");
        log::info!(target:"be200hub::boot","independent BE200 Intel signed-firmware boot succeeded");
        Ok(())
    })();
    if outcome.is_err() {
        eprintln!("No automatic retry or power cycle. Preserve error output and check device before proceeding.");
    }
    usb.release(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe_version() -> intel::Version {
        intel::Version {
            image_type: Some(1),
            cnvi_top: Some(0x02001910),
            cnvr_top: Some(0x02001910),
            cnvi_bt: Some(0x001c3700),
            cnvr_bt: None,
            secure_boot: Some(1),
            sbe_type: Some(1),
            tlv_count: 6,
        }
    }

    #[test]
    fn rejects_operational_and_wrong_security_or_hardware() {
        let mut v = safe_version();
        assert!(validate_bootloader(&v).is_ok());
        v.image_type = Some(3);
        assert!(validate_bootloader(&v).is_err());
        v.image_type = Some(1);
        v.sbe_type = Some(0);
        assert!(validate_bootloader(&v).is_err());
        v.sbe_type = Some(1);
        v.cnvi_top = Some(0);
        assert!(validate_bootloader(&v).is_err());
    }

    #[test]
    fn parses_actual_003_vendor_download_and_boot_events() {
        assert_eq!(parse_download_event(&[0xff,5,6,0,0,0,0]).unwrap(), (0,0));
        assert!(parse_download_event(&[0xff,5,6,1,0,0,0]).unwrap() != (0,0));
        assert!(parse_boot_event(&[0xff,7,2,0,1,1,2,0xff,1]).is_ok());
        assert!(parse_boot_event(&[0xff,7,2,1,1,1,2,0xff,1]).is_err());
    }
    #[test]
    fn windows_003_minimal_oem_initialization_bytes() {
        assert_eq!(firmware::BE200_WINDOWS_PREBOOT_DDC, [0x03,0x86,0x01,0x01]);
        assert_eq!(firmware::BE200_WINDOWS_DSBR_DEFAULT, [0x00,0x00]);
        assert_eq!(firmware::BE200_WINDOWS_WIRE_FRAGMENTS, 3960);
        assert_eq!(firmware::BE200_WINDOWS_WIRE_BYTES, 996036);
    }

}
