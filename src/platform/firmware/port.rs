use std::{
    path::Path,
    time::{Duration, Instant},
};

use rusb::{Device, DeviceDescriptor, DeviceHandle, GlobalContext};

use super::{
    image::load_pinned,
    loader::{
        ensure_with_loaded_firmware, probe_version, FirmwareIo, FirmwareLoadError,
        FirmwareLoadOutcome,
    },
};

const BE200_VID: u16 = 0x8087;
const BE200_PID: u16 = 0x0036;
const HCI_CLASS: (u8, u8, u8) = (0xe0, 0x01, 0x01);

const EVENT_EP: u8 = 0x81;
const BULK_OUT_EP: u8 = 0x02;
const CONTROL_TIMEOUT: Duration = Duration::from_millis(1500);
const BULK_TIMEOUT: Duration = Duration::from_millis(2000);
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_SLICE: Duration = Duration::from_millis(250);
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(60);
const MAX_EVENT_BYTES: usize = 1024;

pub fn ensure_operational(
    sfi_path: &Path,
    ddc_path: &Path,
) -> Result<FirmwareLoadOutcome, FirmwareLoadError> {
    let handle = open_claimed()?;
    let mut port = UsbFirmwarePort::new(handle);

    let result = (|| {
        let current = probe_version(&mut port)?;
        if current.image_type == Some(0x03) {
            return Ok(FirmwareLoadOutcome::AlreadyOperational);
        }

        let firmware = load_pinned(sfi_path, ddc_path)
            .map_err(|error| FirmwareLoadError::Protocol(error.0))?;
        ensure_with_loaded_firmware(&mut port, current, &firmware)
    })();

    let release = port.release();
    match (result, release) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}

struct UsbFirmwarePort {
    handle: DeviceHandle<GlobalContext>,
    pending: Vec<u8>,
    download_started: Option<Instant>,
}

impl UsbFirmwarePort {
    fn new(handle: DeviceHandle<GlobalContext>) -> Self {
        Self {
            handle,
            pending: Vec::new(),
            download_started: None,
        }
    }

    fn release(self) -> Result<(), FirmwareLoadError> {
        self.handle
            .release_interface(0)
            .map_err(io_error)
    }

    fn send_command(&self, opcode: u16, parameters: &[u8]) -> Result<(), FirmwareLoadError> {
        if parameters.len() > u8::MAX as usize {
            return Err(FirmwareLoadError::Protocol(
                "HCI command parameters exceed 255 bytes".into(),
            ));
        }

        let mut bytes = Vec::with_capacity(3 + parameters.len());
        bytes.extend_from_slice(&opcode.to_le_bytes());
        bytes.push(parameters.len() as u8);
        bytes.extend_from_slice(parameters);

        let written = self
            .handle
            .write_control(0x20, 0, 0, 0, &bytes, CONTROL_TIMEOUT)
            .map_err(io_error)?;
        if written != bytes.len() {
            return Err(FirmwareLoadError::Io(format!(
                "short HCI control write {written}/{}",
                bytes.len()
            )));
        }
        Ok(())
    }

    fn next_event(&mut self, timeout: Duration) -> Result<Vec<u8>, FirmwareLoadError> {
        let deadline = Instant::now() + timeout;
        let mut buffer = [0u8; 260];

        loop {
            if let Some(event) = take_event(&mut self.pending)? {
                return Ok(event);
            }
            if Instant::now() >= deadline {
                return Err(FirmwareLoadError::Io(
                    "timed out waiting for HCI event".into(),
                ));
            }

            let slice = deadline
                .saturating_duration_since(Instant::now())
                .min(READ_SLICE);
            match self.handle.read_interrupt(EVENT_EP, &mut buffer, slice) {
                Ok(count) if count > 0 => {
                    self.pending.extend_from_slice(&buffer[..count]);
                    if self.pending.len() > MAX_EVENT_BYTES {
                        return Err(FirmwareLoadError::Protocol(
                            "firmware event stream exceeded bound".into(),
                        ));
                    }
                }
                Ok(_) | Err(rusb::Error::Timeout) => {}
                Err(error) => return Err(io_error(error)),
            }
        }
    }

    fn wait_command_complete(&mut self, opcode: u16) -> Result<Vec<u8>, FirmwareLoadError> {
        let deadline = Instant::now() + EVENT_TIMEOUT;

        loop {
            let event = self.next_event(deadline.saturating_duration_since(Instant::now()))?;

            if event.first() == Some(&0x0e) && event.len() >= 6 {
                let actual = u16::from_le_bytes([event[3], event[4]]);
                if actual == opcode {
                    if event[5] != 0 {
                        return Err(FirmwareLoadError::Protocol(format!(
                            "Command Complete 0x{opcode:04x} status 0x{:02x}",
                            event[5]
                        )));
                    }
                    return Ok(event[6..].to_vec());
                }
            }

            if event.first() == Some(&0x0f) && event.len() >= 6 {
                let actual = u16::from_le_bytes([event[4], event[5]]);
                if actual == opcode && event[2] != 0 {
                    return Err(FirmwareLoadError::Protocol(format!(
                        "Command Status 0x{opcode:04x} status 0x{:02x}",
                        event[2]
                    )));
                }
            }
        }
    }

    fn wait_vendor_event(&mut self, subtype: u8) -> Result<Vec<u8>, FirmwareLoadError> {
        let deadline = Instant::now() + EVENT_TIMEOUT;

        loop {
            let event = self.next_event(deadline.saturating_duration_since(Instant::now()))?;
            if event.first() == Some(&0xff) && event.get(2) == Some(&subtype) {
                return Ok(event);
            }
        }
    }
}

impl FirmwareIo for UsbFirmwarePort {
    fn command_complete(
        &mut self,
        opcode: u16,
        parameters: &[u8],
    ) -> Result<Vec<u8>, FirmwareLoadError> {
        self.send_command(opcode, parameters)?;
        self.wait_command_complete(opcode)
    }

    fn command(&mut self, opcode: u16, parameters: &[u8]) -> Result<(), FirmwareLoadError> {
        self.send_command(opcode, parameters)
    }

    fn bulk_write(&mut self, packet: &[u8]) -> Result<(), FirmwareLoadError> {
        let started = self.download_started.get_or_insert_with(Instant::now);
        if started.elapsed() > DOWNLOAD_DEADLINE {
            return Err(FirmwareLoadError::Io(
                "firmware download exceeded 60-second deadline".into(),
            ));
        }

        let written = self
            .handle
            .write_bulk(BULK_OUT_EP, packet, BULK_TIMEOUT)
            .map_err(io_error)?;
        if written != packet.len() {
            return Err(FirmwareLoadError::Io(format!(
                "short firmware bulk write {written}/{}",
                packet.len()
            )));
        }
        Ok(())
    }

    fn vendor_event(&mut self, subtype: u8) -> Result<Vec<u8>, FirmwareLoadError> {
        self.wait_vendor_event(subtype)
    }
}

fn open_claimed() -> Result<DeviceHandle<GlobalContext>, FirmwareLoadError> {
    let devices = rusb::devices().map_err(io_error)?;
    let mut target: Option<(Device<GlobalContext>, DeviceDescriptor)> = None;

    for device in devices.iter() {
        let Ok(descriptor) = device.device_descriptor() else {
            continue;
        };
        if descriptor.vendor_id() != BE200_VID || descriptor.product_id() != BE200_PID {
            continue;
        }
        if target.is_some() {
            return Err(FirmwareLoadError::Io(
                "multiple 8087:0036 devices found".into(),
            ));
        }
        target = Some((device, descriptor));
    }

    let (device, descriptor) = target.ok_or_else(|| {
        FirmwareLoadError::Io("BE200 8087:0036 not found".into())
    })?;

    if (
        descriptor.class_code(),
        descriptor.sub_class_code(),
        descriptor.protocol_code(),
    ) != HCI_CLASS
    {
        return Err(FirmwareLoadError::Io(
            "BE200 USB descriptor is not Bluetooth HCI e0/01/01".into(),
        ));
    }

    let handle = device.open().map_err(io_error)?;
    handle.claim_interface(0).map_err(io_error)?;
    Ok(handle)
}

fn take_event(pending: &mut Vec<u8>) -> Result<Option<Vec<u8>>, FirmwareLoadError> {
    if pending.len() < 2 {
        return Ok(None);
    }

    let length = 2 + usize::from(pending[1]);
    if length > MAX_EVENT_BYTES {
        return Err(FirmwareLoadError::Protocol(
            "declared HCI event exceeds firmware port bound".into(),
        ));
    }
    if pending.len() < length {
        return Ok(None);
    }

    Ok(Some(pending.drain(..length).collect()))
}

fn io_error(error: rusb::Error) -> FirmwareLoadError {
    FirmwareLoadError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_framer_preserves_split_and_concatenated_events() {
        let first = vec![0x0e, 4, 1, 0x05, 0xfc, 0];
        let second = vec![0xff, 1, 2];
        let mut pending = first[..3].to_vec();

        assert_eq!(take_event(&mut pending).unwrap(), None);

        pending.extend_from_slice(&first[3..]);
        pending.extend_from_slice(&second);

        assert_eq!(take_event(&mut pending).unwrap(), Some(first));
        assert_eq!(take_event(&mut pending).unwrap(), Some(second));
        assert_eq!(take_event(&mut pending).unwrap(), None);
    }

    #[test]
    fn bypass_constants_match_be200_usb_transport() {
        assert_eq!((BE200_VID, BE200_PID), (0x8087, 0x0036));
        assert_eq!(EVENT_EP, 0x81);
        assert_eq!(BULK_OUT_EP, 0x02);
        assert_eq!(HCI_CLASS, (0xe0, 0x01, 0x01));
    }
}
