use std::time::Duration;

use rusb::{Context, DeviceHandle, UsbContext};

use super::{
    backend::{
        BackendError, DeviceBackend, RxEndpoint, TransferCompletion, TransferId,
        TxEndpoint,
    },
    libusb_async::{
        hci_command_control_buffer, transfer_callback, AsyncTransferKind,
        AsyncTransferStatus, CallbackState,
    },
};

pub const BE200_VID: u16 = 0x8087;
pub const BE200_PID: u16 = 0x0036;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Be200UsbLayout {
    pub interface: u8,
    pub event_in: u8,
    pub acl_in: u8,
    pub bulk_out: u8,
}

impl Default for Be200UsbLayout {
    fn default() -> Self {
        Self {
            interface: 0,
            event_in: 0x81,
            acl_in: 0x82,
            bulk_out: 0x02,
        }
    }
}

#[derive(Debug)]
pub enum OpenError {
    UnsupportedPlatform,
    NotFound,
    Usb(rusb::Error),
}

impl From<rusb::Error> for OpenError {
    fn from(value: rusb::Error) -> Self {
        Self::Usb(value)
    }
}

#[cfg(windows)]
mod imp {
    use std::{
        collections::{HashMap, VecDeque},
        ffi::c_void,
        sync::{
            atomic::AtomicU64,
            Arc,
        },
    };

    use rusb::{constants, ffi};

    use super::*;

    const EVENT_BUFFER_BYTES: usize = 1024;
    const ACL_BUFFER_BYTES: usize = 4096;
    const EVENT_WAIT_SLICE: Duration = Duration::from_millis(20);

    struct PendingTransfer {
        transfer: usize,
        state: Box<CallbackState>,
    }

    pub struct WindowsBe200Backend {
        context: Context,
        handle: DeviceHandle<Context>,
        layout: Be200UsbLayout,
        next_id: u64,
        attached: bool,
        completion_order: Arc<AtomicU64>,
        completions: VecDeque<TransferCompletion>,
        active: HashMap<TransferId, PendingTransfer>,
        pending_reads: HashMap<RxEndpoint, TransferId>,
    }

    impl WindowsBe200Backend {
        pub fn open_default() -> Result<Self, OpenError> {
            Self::open(BE200_VID, BE200_PID, Be200UsbLayout::default())
        }

        pub fn open(
            vid: u16,
            pid: u16,
            layout: Be200UsbLayout,
        ) -> Result<Self, OpenError> {
            let context = Context::new()?;
            let devices = context.devices()?;
            let device = devices
                .iter()
                .find(|device| {
                    device
                        .device_descriptor()
                        .map(|descriptor| {
                            descriptor.vendor_id() == vid
                                && descriptor.product_id() == pid
                        })
                        .unwrap_or(false)
                })
                .ok_or(OpenError::NotFound)?;

            let handle = device.open()?;
            handle.claim_interface(layout.interface)?;

            Ok(Self {
                context,
                handle,
                layout,
                next_id: 1,
                attached: true,
                completion_order: Arc::new(AtomicU64::new(0)),
                completions: VecDeque::new(),
                active: HashMap::new(),
                pending_reads: HashMap::new(),
            })
        }

        fn allocate_id(&mut self) -> TransferId {
            let id = TransferId(self.next_id);
            self.next_id += 1;
            id
        }

        fn allocate_transfer(&self) -> Result<*mut ffi::libusb_transfer, BackendError> {
            let transfer = unsafe { ffi::libusb_alloc_transfer(0) };
            if transfer.is_null() {
                Err(BackendError::Failed)
            } else {
                Ok(transfer)
            }
        }

        fn submit_read(
            &mut self,
            endpoint: RxEndpoint,
            usb_endpoint: u8,
            transfer_type: u8,
            buffer_len: usize,
        ) -> Result<TransferId, BackendError> {
            if self.pending_reads.contains_key(&endpoint) {
                return Err(BackendError::AlreadyPending(endpoint));
            }

            let id = self.allocate_id();
            let transfer = self.allocate_transfer()?;
            let mut state = Box::new(CallbackState::new(
                id,
                AsyncTransferKind::Read(endpoint),
                vec![0; buffer_len],
                self.completion_order.clone(),
            ));

            let buffer = state.buffer.as_mut_ptr();
            let user_data = (&mut *state as *mut CallbackState).cast::<c_void>();

            unsafe {
                match transfer_type {
                    constants::LIBUSB_TRANSFER_TYPE_INTERRUPT => {
                        ffi::libusb_fill_interrupt_transfer(
                            transfer,
                            self.handle.as_raw(),
                            usb_endpoint,
                            buffer,
                            buffer_len as i32,
                            transfer_callback,
                            user_data,
                            0,
                        );
                    }
                    constants::LIBUSB_TRANSFER_TYPE_BULK => {
                        ffi::libusb_fill_bulk_transfer(
                            transfer,
                            self.handle.as_raw(),
                            usb_endpoint,
                            buffer,
                            buffer_len as i32,
                            transfer_callback,
                            user_data,
                            0,
                        );
                    }
                    _ => {
                        ffi::libusb_free_transfer(transfer);
                        return Err(BackendError::Failed);
                    }
                }
            }

            let rc = unsafe { ffi::libusb_submit_transfer(transfer) };
            if rc != 0 {
                unsafe { ffi::libusb_free_transfer(transfer) };
                return Err(map_submit_error(rc));
            }

            self.pending_reads.insert(endpoint, id);
            self.active.insert(
                id,
                PendingTransfer {
                    transfer: transfer as usize,
                    state,
                },
            );
            Ok(id)
        }

        fn submit_bulk_write(
            &mut self,
            endpoint: TxEndpoint,
            bytes: Vec<u8>,
        ) -> Result<TransferId, BackendError> {
            let id = self.allocate_id();
            let transfer = self.allocate_transfer()?;
            let mut state = Box::new(CallbackState::new(
                id,
                AsyncTransferKind::Write(endpoint),
                bytes,
                self.completion_order.clone(),
            ));
            let buffer = state.buffer.as_mut_ptr();
            let len = state.buffer.len();
            let user_data = (&mut *state as *mut CallbackState).cast::<c_void>();

            unsafe {
                ffi::libusb_fill_bulk_transfer(
                    transfer,
                    self.handle.as_raw(),
                    self.layout.bulk_out,
                    buffer,
                    len as i32,
                    transfer_callback,
                    user_data,
                    0,
                );
            }

            let rc = unsafe { ffi::libusb_submit_transfer(transfer) };
            if rc != 0 {
                unsafe { ffi::libusb_free_transfer(transfer) };
                return Err(map_submit_error(rc));
            }

            self.active.insert(
                id,
                PendingTransfer {
                    transfer: transfer as usize,
                    state,
                },
            );
            Ok(id)
        }

        fn submit_command(
            &mut self,
            payload: Vec<u8>,
        ) -> Result<TransferId, BackendError> {
            let id = self.allocate_id();
            let transfer = self.allocate_transfer()?;
            let buffer =
                hci_command_control_buffer(&payload).map_err(|_| BackendError::Failed)?;
            let mut state = Box::new(CallbackState::new(
                id,
                AsyncTransferKind::Write(TxEndpoint::Command),
                buffer,
                self.completion_order.clone(),
            ));
            let buffer_ptr = state.buffer.as_mut_ptr();
            let user_data = (&mut *state as *mut CallbackState).cast::<c_void>();

            unsafe {
                ffi::libusb_fill_control_transfer(
                    transfer,
                    self.handle.as_raw(),
                    buffer_ptr,
                    transfer_callback,
                    user_data,
                    0,
                );
            }

            let rc = unsafe { ffi::libusb_submit_transfer(transfer) };
            if rc != 0 {
                unsafe { ffi::libusb_free_transfer(transfer) };
                return Err(map_submit_error(rc));
            }

            self.active.insert(
                id,
                PendingTransfer {
                    transfer: transfer as usize,
                    state,
                },
            );
            Ok(id)
        }

        fn harvest_completions(&mut self) {
            let mut ready = self
                .active
                .iter()
                .filter_map(|(id, pending)| {
                    pending
                        .state
                        .result()
                        .map(|result| (*id, result.order))
                })
                .collect::<Vec<_>>();
            ready.sort_unstable_by_key(|(_, order)| *order);

            for (id, _) in ready {
                let Some(pending) = self.active.remove(&id) else {
                    continue;
                };
                let result = pending
                    .state
                    .result()
                    .expect("ready transfer must have callback result");

                if let AsyncTransferKind::Read(endpoint) = pending.state.kind {
                    if self.pending_reads.get(&endpoint) == Some(&id) {
                        self.pending_reads.remove(&endpoint);
                    }
                }

                let completion = completion_from_callback(&pending.state, result);
                if matches!(completion, TransferCompletion::DeviceGone) {
                    self.attached = false;
                }
                self.completions.push_back(completion);

                unsafe {
                    ffi::libusb_free_transfer(
                        pending.transfer as *mut ffi::libusb_transfer,
                    );
                }
            }
        }

        fn process_events(&mut self) -> Result<(), BackendError> {
            let event_result = self.context.handle_events(Some(EVENT_WAIT_SLICE));
            self.harvest_completions();

            match event_result {
                Ok(()) | Err(rusb::Error::Interrupted) => Ok(()),
                Err(rusb::Error::NoDevice) => {
                    self.attached = false;
                    Ok(())
                }
                Err(_) => Err(BackendError::Failed),
            }
        }
    }

    impl DeviceBackend for WindowsBe200Backend {
        fn arm_read(
            &mut self,
            endpoint: RxEndpoint,
        ) -> Result<TransferId, BackendError> {
            if !self.attached {
                return Err(BackendError::DeviceGone);
            }

            match endpoint {
                RxEndpoint::Event => self.submit_read(
                    endpoint,
                    self.layout.event_in,
                    constants::LIBUSB_TRANSFER_TYPE_INTERRUPT,
                    EVENT_BUFFER_BYTES,
                ),
                RxEndpoint::Acl => self.submit_read(
                    endpoint,
                    self.layout.acl_in,
                    constants::LIBUSB_TRANSFER_TYPE_BULK,
                    ACL_BUFFER_BYTES,
                ),
                RxEndpoint::Iso => Err(BackendError::Failed),
            }
        }

        fn submit_write(
            &mut self,
            endpoint: TxEndpoint,
            bytes: Vec<u8>,
        ) -> Result<TransferId, BackendError> {
            if !self.attached {
                return Err(BackendError::DeviceGone);
            }

            match endpoint {
                TxEndpoint::Command => self.submit_command(bytes),
                TxEndpoint::Acl | TxEndpoint::Iso => {
                    self.submit_bulk_write(endpoint, bytes)
                }
            }
        }

        fn next_completion(&mut self) -> Option<TransferCompletion> {
            if let Some(completion) = self.completions.pop_front() {
                return Some(completion);
            }

            if !self.attached && self.active.is_empty() {
                return None;
            }

            if self.process_events().is_err() {
                return Some(TransferCompletion::Failed { id: None });
            }

            self.completions.pop_front()
        }

        fn cancel_all(&mut self) {
            let transfers = self
                .active
                .values()
                .map(|pending| pending.transfer)
                .collect::<Vec<_>>();

            for transfer in transfers {
                unsafe {
                    let _ = ffi::libusb_cancel_transfer(
                        transfer as *mut ffi::libusb_transfer,
                    );
                }
            }
        }

        fn is_attached(&self) -> bool {
            self.attached
        }
    }

    impl Drop for WindowsBe200Backend {
        fn drop(&mut self) {
            self.cancel_all();

            // libusb requires cancelled transfers to reach a terminal callback
            // before their allocations can be reclaimed safely.
            for _ in 0..200 {
                if self.active.is_empty() {
                    break;
                }
                let _ = self.context.handle_events(Some(Duration::from_millis(5)));
                self.harvest_completions();
            }

            if !self.active.is_empty() {
                // A wedged driver/backend is already outside normal lifecycle.
                // Leaking the still-owned callback/transfer allocations is safer
                // than freeing memory libusb may still reference.
                let leaked = std::mem::take(&mut self.active);
                std::mem::forget(leaked);
            }

            let _ = self.handle.release_interface(self.layout.interface);
        }
    }

    fn completion_from_callback(
        state: &CallbackState,
        result: super::super::libusb_async::CallbackResult,
    ) -> TransferCompletion {
        match result.status {
            AsyncTransferStatus::Completed => completed_transfer(state, result.actual_length),
            AsyncTransferStatus::TimedOut
                if matches!(state.kind, AsyncTransferKind::Read(_))
                    && result.actual_length > 0 =>
            {
                // Preserve partial USB progress. The endpoint reassembler owns
                // packet boundaries, so a timeout after bytes arrived is data,
                // not a reason to discard those bytes.
                completed_transfer(state, result.actual_length)
            }
            AsyncTransferStatus::Cancelled => {
                TransferCompletion::Cancelled { id: state.id }
            }
            AsyncTransferStatus::NoDevice => TransferCompletion::DeviceGone,
            AsyncTransferStatus::Error
            | AsyncTransferStatus::TimedOut
            | AsyncTransferStatus::Stall
            | AsyncTransferStatus::Overflow
            | AsyncTransferStatus::Unknown(_) => TransferCompletion::Failed {
                id: Some(state.id),
            },
        }
    }

    fn completed_transfer(
        state: &CallbackState,
        actual_length: usize,
    ) -> TransferCompletion {
        match state.kind {
            AsyncTransferKind::Read(endpoint) => {
                let actual = actual_length.min(state.buffer.len());
                TransferCompletion::Read {
                    id: state.id,
                    endpoint,
                    bytes: state.buffer[..actual].to_vec(),
                }
            }
            AsyncTransferKind::Write(endpoint) => TransferCompletion::Write {
                id: state.id,
                endpoint,
                bytes: actual_length,
            },
        }
    }

    fn map_submit_error(code: i32) -> BackendError {
        if code == constants::LIBUSB_ERROR_NO_DEVICE {
            BackendError::DeviceGone
        } else {
            BackendError::Failed
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::platform::libusb_async::{CallbackResult, CallbackState};

        #[test]
        fn verified_be200_layout_matches_experiment_descriptor() {
            let layout = Be200UsbLayout::default();
            assert_eq!(layout.interface, 0);
            assert_eq!(layout.event_in, 0x81);
            assert_eq!(layout.acl_in, 0x82);
            assert_eq!(layout.bulk_out, 0x02);
        }

        #[test]
        fn submit_error_maps_device_loss_separately() {
            assert_eq!(
                map_submit_error(constants::LIBUSB_ERROR_NO_DEVICE),
                BackendError::DeviceGone
            );
            assert_eq!(
                map_submit_error(constants::LIBUSB_ERROR_BUSY),
                BackendError::Failed
            );
        }

        #[test]
        fn partial_timeout_read_becomes_progress_not_failure() {
            let state = CallbackState::new(
                TransferId(1),
                AsyncTransferKind::Read(RxEndpoint::Event),
                vec![1, 2, 3, 4],
                Arc::new(AtomicU64::new(0)),
            );
            let completion = completion_from_callback(
                &state,
                CallbackResult {
                    order: 0,
                    status: AsyncTransferStatus::TimedOut,
                    actual_length: 2,
                },
            );
            assert_eq!(
                completion,
                TransferCompletion::Read {
                    id: TransferId(1),
                    endpoint: RxEndpoint::Event,
                    bytes: vec![1, 2],
                }
            );
        }
    }
}

#[cfg(windows)]
#[allow(unused_imports)]
pub use imp::WindowsBe200Backend;

#[cfg(not(windows))]
pub struct WindowsBe200Backend;

#[cfg(not(windows))]
impl WindowsBe200Backend {
    pub fn open_default() -> Result<Self, OpenError> {
        Err(OpenError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod common_tests {
    use super::*;

    #[test]
    fn target_identity_is_stable() {
        assert_eq!((BE200_VID, BE200_PID), (0x8087, 0x0036));
    }
}
