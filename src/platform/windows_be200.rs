use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use rusb::{
    Context, DeviceHandle, Direction, Recipient, RequestType, UsbContext,
};

use super::backend::{
    BackendError, DeviceBackend, RxEndpoint, TransferCompletion, TransferId,
    TxEndpoint,
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
        ffi::c_void,
        ptr,
    };

    use rusb::{constants, ffi};

    use super::*;

    const EVENT_BUFFER_BYTES: usize = 1024;
    const ACL_BUFFER_BYTES: usize = 4096;
    const EVENT_WAIT_SLICE: Duration = Duration::from_millis(20);

    #[derive(Clone, Copy)]
    enum CallbackEndpoint {
        Read(RxEndpoint),
        Write(TxEndpoint),
    }

    struct CallbackState {
        id: TransferId,
        endpoint: CallbackEndpoint,
        buffer: Vec<u8>,
        completions: Arc<Mutex<VecDeque<TransferCompletion>>>,
        active: Arc<Mutex<HashMap<TransferId, usize>>>,
    }

    pub struct WindowsBe200Backend {
        context: Context,
        handle: DeviceHandle<Context>,
        layout: Be200UsbLayout,
        next_id: u64,
        attached: bool,
        completions: Arc<Mutex<VecDeque<TransferCompletion>>>,
        active: Arc<Mutex<HashMap<TransferId, usize>>>,
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

            let mut handle = device.open()?;
            handle.claim_interface(layout.interface)?;

            Ok(Self {
                context,
                handle,
                layout,
                next_id: 1,
                attached: true,
                completions: Arc::new(Mutex::new(VecDeque::new())),
                active: Arc::new(Mutex::new(HashMap::new())),
            })
        }

        fn allocate_id(&mut self) -> TransferId {
            let id = TransferId(self.next_id);
            self.next_id += 1;
            id
        }

        fn pop_completion(&self) -> Option<TransferCompletion> {
            self.completions.lock().ok()?.pop_front()
        }

        fn register_transfer(
            &self,
            id: TransferId,
            transfer: *mut ffi::libusb_transfer,
        ) -> Result<(), BackendError> {
            self.active
                .lock()
                .map_err(|_| BackendError::Failed)?
                .insert(id, transfer as usize);
            Ok(())
        }

        fn unregister_transfer(&self, id: TransferId) {
            if let Ok(mut active) = self.active.lock() {
                active.remove(&id);
            }
        }

        fn submit_read(
            &mut self,
            endpoint: RxEndpoint,
            usb_endpoint: u8,
            transfer_type: u8,
            buffer_len: usize,
        ) -> Result<TransferId, BackendError> {
            let id = self.allocate_id();
            let transfer = unsafe { ffi::libusb_alloc_transfer(0) };
            if transfer.is_null() {
                return Err(BackendError::Failed);
            }

            let mut state = Box::new(CallbackState {
                id,
                endpoint: CallbackEndpoint::Read(endpoint),
                buffer: vec![0; buffer_len],
                completions: self.completions.clone(),
                active: self.active.clone(),
            });

            let buffer = state.buffer.as_mut_ptr();
            let user_data = Box::into_raw(state) as *mut c_void;

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
                        drop(Box::from_raw(user_data as *mut CallbackState));
                        ffi::libusb_free_transfer(transfer);
                        return Err(BackendError::Failed);
                    }
                }
            }

            self.register_transfer(id, transfer)?;
            let rc = unsafe { ffi::libusb_submit_transfer(transfer) };
            if rc != 0 {
                self.unregister_transfer(id);
                unsafe {
                    drop(Box::from_raw(user_data as *mut CallbackState));
                    ffi::libusb_free_transfer(transfer);
                }
                return Err(map_submit_error(rc));
            }
            Ok(id)
        }

        fn submit_bulk_write(
            &mut self,
            endpoint: TxEndpoint,
            bytes: Vec<u8>,
        ) -> Result<TransferId, BackendError> {
            let id = self.allocate_id();
            let transfer = unsafe { ffi::libusb_alloc_transfer(0) };
            if transfer.is_null() {
                return Err(BackendError::Failed);
            }

            let mut state = Box::new(CallbackState {
                id,
                endpoint: CallbackEndpoint::Write(endpoint),
                buffer: bytes,
                completions: self.completions.clone(),
                active: self.active.clone(),
            });
            let buffer = state.buffer.as_mut_ptr();
            let len = state.buffer.len();
            let user_data = Box::into_raw(state) as *mut c_void;

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

            self.register_transfer(id, transfer)?;
            let rc = unsafe { ffi::libusb_submit_transfer(transfer) };
            if rc != 0 {
                self.unregister_transfer(id);
                unsafe {
                    drop(Box::from_raw(user_data as *mut CallbackState));
                    ffi::libusb_free_transfer(transfer);
                }
                return Err(map_submit_error(rc));
            }
            Ok(id)
        }

        fn submit_command(
            &mut self,
            payload: Vec<u8>,
        ) -> Result<TransferId, BackendError> {
            let id = self.allocate_id();
            let transfer = unsafe { ffi::libusb_alloc_transfer(0) };
            if transfer.is_null() {
                return Err(BackendError::Failed);
            }

            let setup_len = constants::LIBUSB_CONTROL_SETUP_SIZE;
            let mut state = Box::new(CallbackState {
                id,
                endpoint: CallbackEndpoint::Write(TxEndpoint::Command),
                buffer: vec![0; setup_len + payload.len()],
                completions: self.completions.clone(),
                active: self.active.clone(),
            });

            unsafe {
                ffi::libusb_fill_control_setup(
                    state.buffer.as_mut_ptr(),
                    rusb::request_type(
                        Direction::Out,
                        RequestType::Class,
                        Recipient::Device,
                    ),
                    0,
                    0,
                    0,
                    payload.len() as u16,
                );
            }
            state.buffer[setup_len..].copy_from_slice(&payload);

            let buffer = state.buffer.as_mut_ptr();
            let user_data = Box::into_raw(state) as *mut c_void;
            unsafe {
                ffi::libusb_fill_control_transfer(
                    transfer,
                    self.handle.as_raw(),
                    buffer,
                    transfer_callback,
                    user_data,
                    0,
                );
            }

            self.register_transfer(id, transfer)?;
            let rc = unsafe { ffi::libusb_submit_transfer(transfer) };
            if rc != 0 {
                self.unregister_transfer(id);
                unsafe {
                    drop(Box::from_raw(user_data as *mut CallbackState));
                    ffi::libusb_free_transfer(transfer);
                }
                return Err(map_submit_error(rc));
            }
            Ok(id)
        }

        fn process_events(&mut self) -> Result<(), BackendError> {
            match self.context.handle_events(Some(EVENT_WAIT_SLICE)) {
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

            let already_pending = self
                .active
                .lock()
                .map_err(|_| BackendError::Failed)?
                .keys()
                .copied()
                .any(|_| false);
            let _ = already_pending;

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
            if let Some(completion) = self.pop_completion() {
                if matches!(completion, TransferCompletion::DeviceGone) {
                    self.attached = false;
                }
                return Some(completion);
            }

            if self.process_events().is_err() {
                return Some(TransferCompletion::Failed { id: None });
            }

            if !self.attached {
                return Some(TransferCompletion::DeviceGone);
            }

            self.pop_completion()
        }

        fn cancel_all(&mut self) {
            let transfers = self
                .active
                .lock()
                .map(|active| active.values().copied().collect::<Vec<_>>())
                .unwrap_or_default();

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
            for _ in 0..100 {
                let empty = self
                    .active
                    .lock()
                    .map(|active| active.is_empty())
                    .unwrap_or(true);
                if empty {
                    break;
                }
                let _ = self.context.handle_events(Some(Duration::from_millis(5)));
            }
            let _ = self.handle.release_interface(self.layout.interface);
        }
    }

    extern "system" fn transfer_callback(transfer: *mut ffi::libusb_transfer) {
        if transfer.is_null() {
            return;
        }

        unsafe {
            let user_data = (*transfer).user_data as *mut CallbackState;
            if user_data.is_null() {
                ffi::libusb_free_transfer(transfer);
                return;
            }

            let state = Box::from_raw(user_data);
            if let Ok(mut active) = state.active.lock() {
                active.remove(&state.id);
            }

            let completion = match (*transfer).status {
                constants::LIBUSB_TRANSFER_COMPLETED => match state.endpoint {
                    CallbackEndpoint::Read(endpoint) => {
                        let actual = usize::try_from((*transfer).actual_length)
                            .unwrap_or(0)
                            .min(state.buffer.len());
                        TransferCompletion::Read {
                            id: state.id,
                            endpoint,
                            bytes: state.buffer[..actual].to_vec(),
                        }
                    }
                    CallbackEndpoint::Write(endpoint) => {
                        let actual =
                            usize::try_from((*transfer).actual_length).unwrap_or(0);
                        TransferCompletion::Write {
                            id: state.id,
                            endpoint,
                            bytes: actual,
                        }
                    }
                },
                constants::LIBUSB_TRANSFER_CANCELLED => {
                    TransferCompletion::Cancelled { id: state.id }
                }
                constants::LIBUSB_TRANSFER_NO_DEVICE => {
                    TransferCompletion::DeviceGone
                }
                _ => TransferCompletion::Failed {
                    id: Some(state.id),
                },
            };

            if let Ok(mut completions) = state.completions.lock() {
                completions.push_back(completion);
            }

            ffi::libusb_free_transfer(transfer);
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
    }
}

#[cfg(windows)]
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
