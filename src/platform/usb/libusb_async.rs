use rusb::ffi;

pub const HCI_COMMAND_REQUEST_TYPE: u8 = 0x20; // Host-to-device | Class | Device.
pub const HCI_COMMAND_REQUEST: u8 = 0;
pub const HCI_COMMAND_VALUE: u16 = 0;
pub const HCI_COMMAND_INDEX: u16 = 0;



use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use crate::platform::backend::{RxEndpoint, TransferId, TxEndpoint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncTransferKind {
    Read(RxEndpoint),
    Write(TxEndpoint),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallbackResult {
    pub order: u64,
    pub status: AsyncTransferStatus,
    pub actual_length: usize,
}

/// Heap-stable callback state owned by the backend's pending-transfer table.
///
/// The libusb callback borrows this allocation through user_data and only marks
/// completion. It never frees the libusb_transfer or this state. Reclamation is
/// performed by the owning TransportContext after it observes completion.
pub struct CallbackState {
    pub id: TransferId,
    pub kind: AsyncTransferKind,
    pub buffer: Vec<u8>,
    order: Arc<AtomicU64>,
    result: Option<CallbackResult>,
}

impl CallbackState {
    pub fn new(
        id: TransferId,
        kind: AsyncTransferKind,
        buffer: Vec<u8>,
        order: Arc<AtomicU64>,
    ) -> Self {
        Self {
            id,
            kind,
            buffer,
            order,
            result: None,
        }
    }

    pub fn result(&self) -> Option<CallbackResult> {
        self.result
    }

    pub fn take_result(&mut self) -> Option<CallbackResult> {
        self.result.take()
    }

    pub fn completed_bytes(&self) -> &[u8] {
        let len = self
            .result
            .map(|result| result.actual_length.min(self.buffer.len()))
            .unwrap_or(0);
        &self.buffer[..len]
    }
}

pub extern "system" fn transfer_callback(transfer: *mut ffi::libusb_transfer) {
    if transfer.is_null() {
        return;
    }

    unsafe {
        let state_ptr = (*transfer).user_data.cast::<CallbackState>();
        if state_ptr.is_null() {
            return;
        }
        let state = &mut *state_ptr;

        // A one-shot transfer must receive exactly one terminal callback.
        // A duplicate callback is an invariant violation; keep the first result
        // so ownership/reclamation remains deterministic.
        if state.result.is_some() {
            return;
        }

        let actual_length = (*transfer).actual_length.max(0) as usize;
        state.result = Some(CallbackResult {
            order: state.order.fetch_add(1, Ordering::AcqRel),
            status: AsyncTransferStatus::from_libusb((*transfer).status),
            actual_length,
        });
    }
}

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
    fn callback_marks_state_but_does_not_reclaim_owner_memory() {
        let order = Arc::new(AtomicU64::new(0));
        let mut state = Box::new(CallbackState::new(
            TransferId(7),
            AsyncTransferKind::Read(RxEndpoint::Event),
            vec![0xaa, 0xbb, 0xcc, 0xdd],
            order,
        ));

        let transfer = unsafe { ffi::libusb_alloc_transfer(0) };
        assert!(!transfer.is_null());

        unsafe {
            (*transfer).user_data = (&mut *state as *mut CallbackState).cast();
            (*transfer).buffer = state.buffer.as_mut_ptr();
            (*transfer).length = state.buffer.len() as i32;
            (*transfer).actual_length = 3;
            (*transfer).status = ffi::constants::LIBUSB_TRANSFER_COMPLETED;
        }

        transfer_callback(transfer);

        let result = state.result().unwrap();
        assert_eq!(result.order, 0);
        assert_eq!(result.status, AsyncTransferStatus::Completed);
        assert_eq!(result.actual_length, 3);
        assert_eq!(state.completed_bytes(), &[0xaa, 0xbb, 0xcc]);

        // Owner still controls both allocations after callback.
        unsafe { ffi::libusb_free_transfer(transfer) };
        drop(state);
    }

    #[test]
    fn callback_preserves_partial_bytes_on_timeout_status() {
        let order = Arc::new(AtomicU64::new(10));
        let mut state = Box::new(CallbackState::new(
            TransferId(8),
            AsyncTransferKind::Read(RxEndpoint::Event),
            vec![1, 2, 3, 4, 5],
            order,
        ));
        let transfer = unsafe { ffi::libusb_alloc_transfer(0) };
        assert!(!transfer.is_null());

        unsafe {
            (*transfer).user_data = (&mut *state as *mut CallbackState).cast();
            (*transfer).buffer = state.buffer.as_mut_ptr();
            (*transfer).length = state.buffer.len() as i32;
            (*transfer).actual_length = 2;
            (*transfer).status = ffi::constants::LIBUSB_TRANSFER_TIMED_OUT;
        }

        transfer_callback(transfer);
        assert_eq!(
            state.result(),
            Some(CallbackResult {
                order: 10,
                status: AsyncTransferStatus::TimedOut,
                actual_length: 2,
            })
        );
        assert_eq!(state.completed_bytes(), &[1, 2]);

        unsafe { ffi::libusb_free_transfer(transfer) };
    }

    #[test]
    fn duplicate_callback_keeps_first_terminal_result() {
        let order = Arc::new(AtomicU64::new(0));
        let mut state = Box::new(CallbackState::new(
            TransferId(9),
            AsyncTransferKind::Write(TxEndpoint::Iso),
            vec![9, 9, 9],
            order,
        ));
        let transfer = unsafe { ffi::libusb_alloc_transfer(0) };
        assert!(!transfer.is_null());

        unsafe {
            (*transfer).user_data = (&mut *state as *mut CallbackState).cast();
            (*transfer).actual_length = 3;
            (*transfer).status = ffi::constants::LIBUSB_TRANSFER_COMPLETED;
        }
        transfer_callback(transfer);

        unsafe {
            (*transfer).actual_length = 0;
            (*transfer).status = ffi::constants::LIBUSB_TRANSFER_ERROR;
        }
        transfer_callback(transfer);

        assert_eq!(
            state.result(),
            Some(CallbackResult {
                order: 0,
                status: AsyncTransferStatus::Completed,
                actual_length: 3,
            })
        );

        unsafe { ffi::libusb_free_transfer(transfer) };
    }

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
