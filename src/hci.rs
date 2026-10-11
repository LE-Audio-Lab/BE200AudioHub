//! R1.4: single allow-listed, read-only Intel HCI query (0xFC05).
//! Requires BE200 WinUSB, verified before opening the controller.
//!
//! From BE200 USBPcap 002/003 and Linux btintel:
//!   EP0 Class Control OUT: 20 00 0000 0000 [05 FC 01 FF]
//!   EP81 Interrupt IN: HCI Event, may exceed one 64-byte USB packet.
use std::time::{Duration, Instant};
use crate::{error::{AppError, Result}, intel, usb};

const INTEL_READ_VERSION: u16 = 0xfc05;
const COMMAND: [u8; 4] = [0x05, 0xfc, 0x01, 0xff];
const CONTROL_TIMEOUT: Duration = Duration::from_millis(1500);
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_SLICE_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Default)]
struct EventAssembler { pending: Vec<u8> }

impl EventAssembler {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.pending.extend_from_slice(chunk);
        if self.pending.len() > 1024 {
            return Err(AppError::Protocol("HCI event stream exceeded 1024-byte guard".into()));
        }
        let mut results = Vec::new();
        while self.pending.len() >= 2 {
            let required = 2 + usize::from(self.pending[1]);
            if self.pending.len() < required { break; }
            results.push(self.pending.drain(..required).collect());
        }
        Ok(results)
    }
}

enum ReadVersionEvent<'a> {
    Match { status: u8, tlv: &'a [u8] },
    Other,
}

/// Parse an HCI Command Complete (event code 0x0E) for exactly 0xFC05.
fn match_version_complete(event: &[u8]) -> ReadVersionEvent<'_> {
    // [event=0x0e, plen, numHciCmdPackets, opcodeLo, opcodeHi, status, TLVs...]
    if event.len() < 6 || event[0] != 0x0e {
        return ReadVersionEvent::Other;
    }
    let opcode = u16::from_le_bytes([event[3], event[4]]);
    if opcode != INTEL_READ_VERSION {
        return ReadVersionEvent::Other;
    }
    ReadVersionEvent::Match { status: event[5], tlv: &event[6..] }
}

pub fn read_intel_version() -> Result<()> {
    let handle = usb::open_be200_claimed()?;
    println!("Sending Intel HCI Read Version (0xFC05, TLV request 0xFF)...");
    let result = (|| -> Result<()> {
        // Standard Bluetooth USB HCI class request, recipient: device.
        // Only one explicitly allow-listed query. No HCI Reset or firmware commands.
        let written = handle.write_control(0x20, 0x00, 0, 0, &COMMAND, CONTROL_TIMEOUT)?;
        if written != COMMAND.len() {
            return Err(AppError::Protocol(format!(
                "Short HCI control write: {written}/{} bytes", COMMAND.len()
            )));
        }

        // Use max HCI event receive size (as btusb does), not the endpoint's
        // 64-byte max *packet* size. A BE200 version event can be ~96 bytes
        // and USB should be allowed to finish it in one submitted transfer.
        let mut packet = [0u8; 260];
        let mut stream = EventAssembler::default();
        let started = Instant::now();
        let mut ignored_events = 0usize;
        while started.elapsed() < EVENT_TIMEOUT {
            let received = match handle.read_interrupt(0x81, &mut packet, READ_SLICE_TIMEOUT) {
                Ok(n) => n,
                Err(rusb::Error::Timeout) => continue,
                Err(e) => return Err(e.into()),
            };
            if received == 0 { continue; }
            for event in stream.push(&packet[..received])? {
                if let ReadVersionEvent::Match { status, tlv } = match_version_complete(&event) {
                    if status != 0 {
                        return Err(AppError::Protocol(format!(
                            "Intel Read Version returned HCI status 0x{status:02X}"
                        )));
                    }
                    let version = intel::parse_read_version_tlv(tlv)?;
                    println!("Intel HCI Read Version: success (status 0x00)");
                    match version.image_type {
                        Some(t) => println!("  Image: {} (0x{t:02X})", intel::image_description(t)),
                        None => println!("  Image: not provided in response"),
                    }
                    if let Some(v) = version.cnvi_top { println!("  CNVi TOP: 0x{v:08X}"); }
                    if let Some(v) = version.cnvr_top { println!("  CNVr TOP: 0x{v:08X}"); }
                    if let Some(v) = version.cnvi_bt { println!("  CNVi Bluetooth: 0x{v:08X}"); }
                    if let Some(v) = version.hardware_variant() { println!("  Hardware variant: 0x{v:02X}"); }
                    if let Some(v) = version.secure_boot { println!("  Secure Boot: 0x{v:02X}"); }
                    if let Some(v) = version.sbe_type { println!("  SBE Type: 0x{v:02X}"); }
                    println!("  TLV records: {}", version.tlv_count);
                    println!("  Ignored unrelated HCI events: {ignored_events}");
                    log::info!(target:"be200hub::hci", "0xFC05 OK; image={:?}, CNVi={:?}, CNVr={:?}",
                        version.image_type, version.cnvi_top, version.cnvr_top);
                    return Ok(());
                }
                ignored_events += 1;
                log::debug!(target:"be200hub::hci", "ignoring event 0x{:02X}, {} bytes",
                    event[0], event.len());
            }
        }
        Err(AppError::Protocol(format!(
            "Timed out awaiting Intel 0xFC05 Command Complete after {}ms (ignored {ignored_events} event(s)); no retry/reset was attempted",
            EVENT_TIMEOUT.as_millis()
        )))
    })();
    // A failed transaction must not leave interface 0 claimed; dropping the handle
    // also closes it, but explicitly release and report errors on a success path.
    let released = handle.release_interface(0);
    match (result, released) {
        (Err(e), _) => Err(e),
        (Ok(()), Err(e)) => Err(e.into()),
        (Ok(()), Ok(())) => { println!("Release interface 0: OK"); Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_complete() -> Vec<u8> {
        // Synthetic HCI event with three fields copied from 002 capture.
        let mut event = vec![0x0e, 19, 1, 0x05, 0xfc, 0];
        event.extend_from_slice(&[
            0x10, 4, 0x10, 0x19, 0, 2,
            0x11, 4, 0x10, 0x19, 0, 2,
            0x1c, 1, 0x03
        ]);
        event
    }

    #[test]
    fn matches_intel_command_complete() {
        let event = example_complete();
        match match_version_complete(&event) {
            ReadVersionEvent::Match { status: 0, tlv } => {
                assert_eq!(intel::parse_read_version_tlv(tlv).unwrap().image_type, Some(3));
            }
            _ => panic!("Intel 0xFC05 Command Complete not matched"),
        }
    }

    #[test]
    fn reassembles_split_interrupt_event() {
        let event = example_complete();
        let mut asm = EventAssembler::default();
        assert!(asm.push(&event[..4]).unwrap().is_empty());
        assert!(asm.push(&event[4..17]).unwrap().is_empty());
        assert_eq!(asm.push(&event[17..]).unwrap(), vec![event]);
    }

    #[test]
    fn reassembles_version_response_larger_than_64_bytes() {
        let mut event = vec![0x0e, 0, 1, 0x05, 0xfc, 0, 0x1c, 1, 3, 0x7e, 80];
        event.extend_from_slice(&[0u8; 80]); // unknown, well-formed vendor TLV
        event[1] = (event.len() - 2) as u8;
        assert!(event.len() > 64);
        let mut asm = EventAssembler::default();
        assert!(asm.push(&event[..64]).unwrap().is_empty());
        let parsed = asm.push(&event[64..]).unwrap();
        assert_eq!(parsed, vec![event.clone()]);
        match match_version_complete(&parsed[0]) {
            ReadVersionEvent::Match { status: 0, tlv } => {
                assert_eq!(intel::parse_read_version_tlv(tlv).unwrap().image_type, Some(3));
            }
            _ => panic!("version event should be complete"),
        }
    }

    #[test]
    fn handles_multiple_events_per_interrupt_read() {
        let event = example_complete();
        let mut two = event.clone();
        two.extend_from_slice(&event);
        let mut asm = EventAssembler::default();
        assert_eq!(asm.push(&two).unwrap(), vec![event.clone(), event]);
    }

    #[test]
    fn ignores_other_opcode() {
        let mut event = example_complete();
        event[3] = 0x03; // standard HCI Reset
        event[4] = 0x0c;
        assert!(matches!(match_version_complete(&event), ReadVersionEvent::Other));
    }

    #[test]
    fn rejects_oversized_garbage_stream() {
        let mut asm = EventAssembler::default();
        assert!(asm.push(&[0x0e; 1025]).is_err());
    }
}
