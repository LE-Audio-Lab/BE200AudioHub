//! R1.6: shared operational HCI command / event transport over BE200 WinUSB.
//! EP0 class control OUT; Interrupt IN 0x81. No firmware, USB reset or HCI Reset.
use crate::{driver, error::{AppError, Result}, usb};
use rusb::{DeviceHandle, GlobalContext};
use std::time::{Duration, Instant};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
const EVENT_READ_SLICE: Duration = Duration::from_millis(250);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_EVENT_BUFFER: usize = 2048;

fn protocol(message: impl Into<String>) -> AppError { AppError::Protocol(message.into()) }

pub struct HciSession {
    handle: DeviceHandle<GlobalContext>,
    pending: Vec<u8>,
}

impl HciSession {
    pub fn open() -> Result<Self> {
        driver::verify()?;
        Ok(Self { handle: usb::open_be200_claimed()?, pending: Vec::new() })
    }

    /// Standard operational HCI command, not bootloader Secure Send.
    pub fn send_command(&self, opcode: u16, params: &[u8]) -> Result<()> {
        if params.len() > u8::MAX as usize {
            return Err(protocol("HCI command parameters exceed 255 bytes"));
        }
        let mut wire = Vec::with_capacity(params.len() + 3);
        wire.extend_from_slice(&opcode.to_le_bytes());
        wire.push(params.len() as u8);
        wire.extend_from_slice(params);
        let sent = self.handle.write_control(0x20, 0, 0, 0, &wire, CONTROL_TIMEOUT)?;
        if sent != wire.len() {
            return Err(protocol(format!("Short EP0 HCI command 0x{opcode:04X}: {sent}/{}", wire.len())));
        }
        Ok(())
    }

    /// A complete [event-code, parameter-length, params] or None on timeout.
    /// Preserves fragments and multiple events per interrupt transfer.
    pub fn poll_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let until = Instant::now() + timeout;
        let mut packet = [0u8; 260];
        loop {
            if self.pending.len() >= 2 {
                let len = 2 + self.pending[1] as usize;
                if self.pending.len() >= len {
                    return Ok(Some(self.pending.drain(..len).collect()));
                }
            }
            if self.pending.len() > MAX_EVENT_BUFFER {
                return Err(protocol("HCI event buffer exceeds 2048-byte guard"));
            }
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Ok(None); }
            match self.handle.read_interrupt(0x81, &mut packet, remaining.min(EVENT_READ_SLICE)) {
                Ok(n) if n > 0 => {
                    self.pending.extend_from_slice(&packet[..n]);
                    if self.pending.len() > MAX_EVENT_BUFFER {
                        return Err(protocol("HCI event buffer exceeds 2048-byte guard"));
                    }
                },
                Ok(_) | Err(rusb::Error::Timeout) => {},
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub fn wait_complete(&mut self, opcode: u16) -> Result<Vec<u8>> {
        let until = Instant::now() + COMMAND_TIMEOUT;
        let mut ignored = 0usize;
        while Instant::now() < until {
            let remaining = until.saturating_duration_since(Instant::now());
            let Some(event) = self.poll_event(remaining)? else { break; };
            if let Some(parameters) = parse_command_response(&event, opcode)? {
                return Ok(parameters);
            }
            ignored += 1;
            if ignored > 128 {
                return Err(protocol(format!("Too many unrelated HCI events while awaiting 0x{opcode:04X}")));
            }
        }
        Err(protocol(format!("Timed out awaiting Command Complete for 0x{opcode:04X}")))
    }

    pub fn command_complete(&mut self, opcode: u16, params: &[u8]) -> Result<Vec<u8>> {
        self.send_command(opcode, params)?;
        self.wait_complete(opcode)
    }

    pub fn finish<T>(self, result: Result<T>) -> Result<T> {
        let release = self.handle.release_interface(0);
        match (result, release) {
            (Err(error), Err(release_error)) => {
                eprintln!("Warning: BE200 interface 0 release error: {release_error}");
                Err(error)
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
            (Ok(value), Ok(())) => {
                println!("Release interface 0: OK");
                Ok(value)
            }
        }
    }
}

fn parse_command_response(event: &[u8], expected: u16) -> Result<Option<Vec<u8>>> {
    if event.len() < 2 || event.len() != event[1] as usize + 2 {
        return Err(protocol("Invalid HCI Event framing"));
    }
    match event[0] {
        0x0e => {
            if event.len() < 6 { return Err(protocol("Truncated HCI Command Complete")); }
            let actual = u16::from_le_bytes([event[3], event[4]]);
            if actual != expected { return Ok(None); }
            if event[5] != 0 {
                return Err(protocol(format!(
                    "HCI Command Complete 0x{actual:04X} returned status 0x{:02X}", event[5]
                )));
            }
            Ok(Some(event[6..].to_vec()))
        }
        0x0f => {
            if event.len() < 6 { return Err(protocol("Truncated HCI Command Status")); }
            let actual = u16::from_le_bytes([event[4], event[5]]);
            if actual == expected && event[2] != 0 {
                return Err(protocol(format!(
                    "HCI Command Status 0x{actual:04X} returned status 0x{:02X}", event[2]
                )));
            }
            Ok(None) // successful Command Status is NOT Command Complete
        }
        _ => Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_complete_payload() {
        let packet = [0x0e, 7, 1, 0x60, 0x20, 0, 0xf8, 0, 4];
        assert_eq!(parse_command_response(&packet, 0x2060).unwrap(), Some(vec![0xf8, 0, 4]));
    }
    #[test]
    fn command_status_not_completion() {
        assert_eq!(parse_command_response(&[0x0f, 4, 0, 1, 0x60, 0x20], 0x2060).unwrap(), None);
    }
    #[test]
    fn rejects_failed_status_and_truncation() {
        assert!(parse_command_response(&[0x0e, 4, 1, 0x60, 0x20, 1], 0x2060).is_err());
        assert!(parse_command_response(&[0x0f, 4, 0x0c, 1, 0x60, 0x20], 0x2060).is_err());
        assert!(parse_command_response(&[0x0e, 5, 1, 0x60, 0x20, 0], 0x2060).is_err());
    }
}
