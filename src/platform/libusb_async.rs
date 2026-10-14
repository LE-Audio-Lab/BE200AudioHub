use rusb::ffi;

pub const HCI_COMMAND_REQUEST_TYPE: u8 = 0x20; // Host-to-device | Class | Device.
pub const HCI_COMMAND_REQUEST: u8 = 0;
pub const HCI_COMMAND_VALUE: u16 = 0;
pub const HCI_COMMAND_INDEX: u16 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncTransferStatus {
    Completed,
    Error,
    TimedOut,
    Cancelled,
    Stall,
    NoDevice,
    Overflow,
    Unknown(i32),
}

impl AsyncTransferStatus {
    pub fn from_libusb(status: i32) -> Self {
        match status {
            ffi::constants::LIBUSB_TRANSFER_COMPLETED => Self::Completed,
            ffi::constants::LIBUSB_TRANSFER_ERROR => Self::Error,
            ffi::constants::LIBUSB_TRANSFER_TIMED_OUT => Self::TimedOut,
            ffi::constants::LIBUSB_TRANSFER_CANCELLED => Self::Cancelled,
            ffi::constants::LIBUSB_TRANSFER_STALL => Self::Stall,
            ffi::constants::LIBUSB_TRANSFER_NO_DEVICE => Self::NoDevice,
            ffi::constants::LIBUSB_TRANSFER_OVERFLOW => Self::Overflow,
            other => Self::Unknown(other),
        }
    }
}

/// Build the single buffer required by libusb async control transfers:
/// 8-byte USB setup packet followed by the HCI Command payload.
pub fn hci_command_control_buffer(payload: &[u8]) -> Result<Vec<u8>, ControlBufferError> {
    if payload.len() > u16::MAX as usize {
        return Err(ControlBufferError::PayloadTooLarge(payload.len()));
    }

    let mut buffer = vec![0u8; ffi::constants::LIBUSB_CONTROL_SETUP_SIZE + payload.len()];
    unsafe {
        ffi::libusb_fill_control_setup(
            buffer.as_mut_ptr(),
            HCI_COMMAND_REQUEST_TYPE,
            HCI_COMMAND_REQUEST,
            HCI_COMMAND_VALUE,
            HCI_COMMAND_INDEX,
            payload.len() as u16,
        );
    }
    buffer[ffi::constants::LIBUSB_CONTROL_SETUP_SIZE..].copy_from_slice(payload);
    Ok(buffer)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlBufferError {
    PayloadTooLarge(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_libusb_transfer_terminal_status() {
        assert_eq!(
            AsyncTransferStatus::from_libusb(ffi::constants::LIBUSB_TRANSFER_COMPLETED),
            AsyncTransferStatus::Completed
        );
        assert_eq!(
            AsyncTransferStatus::from_libusb(ffi::constants::LIBUSB_TRANSFER_CANCELLED),
            AsyncTransferStatus::Cancelled
        );
        assert_eq!(
            AsyncTransferStatus::from_libusb(ffi::constants::LIBUSB_TRANSFER_NO_DEVICE),
            AsyncTransferStatus::NoDevice
        );
        assert_eq!(
            AsyncTransferStatus::from_libusb(12345),
            AsyncTransferStatus::Unknown(12345)
        );
    }

    #[test]
    fn command_control_buffer_matches_be200_hci_usb_class_request() {
        let payload = [0x03, 0x0c, 0x00]; // HCI Reset command bytes, no H4 type.
        let buffer = hci_command_control_buffer(&payload).unwrap();

        assert_eq!(buffer.len(), ffi::constants::LIBUSB_CONTROL_SETUP_SIZE + payload.len());
        assert_eq!(buffer[0], HCI_COMMAND_REQUEST_TYPE);
        assert_eq!(buffer[1], HCI_COMMAND_REQUEST);
        assert_eq!(&buffer[2..4], &HCI_COMMAND_VALUE.to_le_bytes());
        assert_eq!(&buffer[4..6], &HCI_COMMAND_INDEX.to_le_bytes());
        assert_eq!(&buffer[6..8], &(payload.len() as u16).to_le_bytes());
        assert_eq!(
            &buffer[ffi::constants::LIBUSB_CONTROL_SETUP_SIZE..],
            payload
        );
    }
}
