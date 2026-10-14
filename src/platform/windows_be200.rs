use std::{
    collections::HashMap,
    ptr::NonNull,
    sync::{
        atomic::AtomicU64,
        Arc,
    },
    time::Duration,
};

use rusb::{
    Context, Device, DeviceHandle, Direction, Error as UsbError, TransferType,
    UsbContext,
};

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

const EVENT_TRANSFER_BYTES: usize = 4 * 1024;
const BULK_TRANSFER_BYTES: usize = 65_539;
const EVENT_PUMP_SLICE: Duration = Duration::from_millis(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Be200UsbLayout {
    pub configuration: u8,
    pub interface: u8,
    pub alternate: u8,
    pub event_in: u8,
    pub acl_in: u8,
    pub bulk_out: u8,
}

#[derive(Debug)]
pub enum OpenError {
    UnsupportedPlatform,
    NotFound,
    Ambiguous,
    Usb(rusb::Error),
    UnsupportedLayout,
}

impl From<rusb::Error> for OpenError {
    fn from(value: rusb::Error) -> Self {
        Self::Usb(value)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    struct PendingTransfer {
        raw: NonNull<rusb::ffi::libusb_transfer>,
        state: Box<CallbackState>,
    }

    impl PendingTransfer {
        unsafe fn reclaim(self) {
            rusb::ffi::libusb_free_transfer(self.raw.as_ptr());
            drop(self.state);
        }
    }

    pub struct WindowsBe200Backend {
        context: Context,
        handle: DeviceHandle<Context>,
        layout: Be200UsbLayout,
        next_id: u64,
        callback_order: Arc<AtomicU64>,
        pending: HashMap<TransferId, PendingTransfer>,
        attached: bool,
    }

    impl WindowsBe200Backend {
        pub fn open_default() -> Result<Self, OpenError> {
            let context = Context::new()?;
            let devices = context.devices()?;
            let mut selected: Option<Device<Context>> = None;

            for device in devices.iter() {
                let descriptor = match device.device_descriptor() {
                    Ok(descriptor) => descriptor,
                    Err(_) => continue,
                };
                if descriptor.vendor_id() != BE200_VID
                    || descriptor.product_id() != BE200_PID
                {
                    continue;
                }
                if selected.is_some() {
                    return Err(OpenError::Ambiguous);
                }
                selected = Some(device);
            }

            let device = selected.ok_or(OpenError::NotFound)?;
            let layout = discover_layout(&device)?;
            let handle = device.open()?;

            match handle.set_auto_detach_kernel_driver(true) {
                Ok(()) | Err(UsbError::NotSupported) => {}
                Err(error) => return Err(OpenError::Usb(error)),
            }

            if handle.active_configuration().ok() != Some(layout.configuration) {
                handle.set_active_configuration(layout.configuration)?;
            }

            handle.claim_interface(layout.interface)?;
            if layout.alternate != 0 {
                handle.set_alternate_setting(layout.interface, layout.alternate)?;
            }

            Ok(Self {
                context,
                handle,
                layout,
                next_id: 1,
                callback_order: Arc::new(AtomicU64::new(0)),
                pending: HashMap::new(),
                attached: true,
            })
        }

        pub fn layout(&self) -> Be200UsbLayout {
            self.layout
        }

        fn allocate_id(&mut self) -> TransferId {
            let id = TransferId(self.next_id);
            self.next_id += 1;
            id
        }

        fn has_pending_read(&self, endpoint: RxEndpoint) -> bool {
            self.pending
                .values()
                .any(|pending| pending.state.kind == AsyncTransferKind::Read(endpoint))
        }

        fn submit_read(
            &mut self,
            endpoint: RxEndpoint,
            usb_endpoint: u8,
            transfer_type: TransferType,
            capacity: usize,
        ) -> Result<TransferId, BackendError> {
            if !self.attached {
                return Err(BackendError::DeviceGone);
            }
            if self.has_pending_read(endpoint) {
                return Err(BackendError::AlreadyPending(endpoint));
            }

            let id = self.allocate_id();
            let mut state = Box::new(CallbackState::new(
                id,
                AsyncTransferKind::Read(endpoint),
                vec![0u8; capacity],
                self.callback_order.clone(),
            ));
            let raw = allocate_transfer()?;

            unsafe {
                let user_data = (&mut *state as *mut CallbackState).cast();
                match transfer_type {
                    TransferType::Interrupt => rusb::ffi::libusb_fill_interrupt_transfer(
                        raw.as_ptr(),
                        self.handle.as_raw(),
                        usb_endpoint,
                        state.buffer.as_mut_ptr(),
                        state.buffer.len() as i32,
                        transfer_callback,
                        user_data,
                        0,
                    ),
                    TransferType::Bulk => rusb::ffi::libusb_fill_bulk_transfer(
                        raw.as_ptr(),
                        self.handle.as_raw(),
                        usb_endpoint,
                        state.buffer.as_mut_ptr(),
                        state.buffer.len() as i32,
                        transfer_callback,
                        user_data,
                        0,
                    ),
                    _ => {
                        rusb::ffi::libusb_free_transfer(raw.as_ptr());
                        return Err(BackendError::Failed);
                    }
                }

                let rc = rusb::ffi::libusb_submit_transfer(raw.as_ptr());
                if rc < 0 {
                    rusb::ffi::libusb_free_transfer(raw.as_ptr());
                    return Err(map_libusb_code(rc));
                }
            }

            self.pending.insert(id, PendingTransfer { raw, state });
            Ok(id)
        }

        fn submit_write_inner(
            &mut self,
            endpoint: TxEndpoint,
            buffer: Vec<u8>,
        ) -> Result<TransferId, BackendError> {
            if !self.attached {
                return Err(BackendError::DeviceGone);
            }

            let id = self.allocate_id();
            let mut state = Box::new(CallbackState::new(
                id,
                AsyncTransferKind::Write(endpoint),
                buffer,
                self.callback_order.clone(),
            ));
            let raw = allocate_transfer()?;

            unsafe {
                let user_data = (&mut *state as *mut CallbackState).cast();
                match endpoint {
                    TxEndpoint::Command => rusb::ffi::libusb_fill_control_transfer(
                        raw.as_ptr(),
                        self.handle.as_raw(),
                        state.buffer.as_mut_ptr(),
                        transfer_callback,
                        user_data,
                        0,
                    ),
                    TxEndpoint::Acl | TxEndpoint::Iso => {
                        rusb::ffi::libusb_fill_bulk_transfer(
                            raw.as_ptr(),
                            self.handle.as_raw(),
                            self.layout.bulk_out,
                            state.buffer.as_mut_ptr(),
                            state.buffer.len() as i32,
                            transfer_callback,
                            user_data,
                            0,
                        )
                    }
                }

                let rc = rusb::ffi::libusb_submit_transfer(raw.as_ptr());
                if rc < 0 {
                    rusb::ffi::libusb_free_transfer(raw.as_ptr());
                    return Err(map_libusb_code(rc));
                }
            }

            self.pending.insert(id, PendingTransfer { raw, state });
            Ok(id)
        }

        fn pop_completed(&mut self) -> Option<TransferCompletion> {
            let id = self
                .pending
                .iter()
                .filter_map(|(id, pending)| {
                    pending
                        .state
                        .result()
                        .map(|result| (*id, result.order))
                })
                .min_by_key(|(_, order)| *order)
                .map(|(id, _)| id)?;

            let mut pending = self.pending.remove(&id)?;
            let result = pending.state.result()?;
            let kind = pending.state.kind;
            let bytes = pending.state.completed_bytes().to_vec();
            let _ = pending.state.take_result();

            let completion = match result.status {
                AsyncTransferStatus::Completed => match kind {
                    AsyncTransferKind::Read(endpoint) => TransferCompletion::Read {
                        id,
                        endpoint,
                        bytes,
                    },
                    AsyncTransferKind::Write(endpoint) => TransferCompletion::Write {
                        id,
                        endpoint,
                        bytes: logical_write_length(
                            endpoint,
                            &pending.state.buffer,
                            result.actual_length,
                        ),
                    },
                },
                AsyncTransferStatus::Cancelled => {
                    TransferCompletion::Cancelled { id }
                }
                AsyncTransferStatus::NoDevice => {
                    self.attached = false;
                    TransferCompletion::DeviceGone
                }
                AsyncTransferStatus::TimedOut
                    if matches!(kind, AsyncTransferKind::Read(_))
                        && result.actual_length > 0 =>
                {
                    let AsyncTransferKind::Read(endpoint) = kind else {
                        unreachable!()
                    };
                    TransferCompletion::Read {
                        id,
                        endpoint,
                        bytes,
                    }
                }
                _ => TransferCompletion::Failed { id: Some(id) },
            };

            unsafe { pending.reclaim() };
            Some(completion)
        }

        fn pump_events_once(&mut self) -> Result<(), BackendError> {
            match self.context.handle_events(Some(EVENT_PUMP_SLICE)) {
                Ok(()) | Err(UsbError::Interrupted) => Ok(()),
                Err(UsbError::NoDevice) => {
                    self.attached = false;
                    Err(BackendError::DeviceGone)
                }
                Err(_) => Err(BackendError::Failed),
            }
        }

        fn reclaim_completed_without_delivery(&mut self) {
            let ids = self
                .pending
                .iter()
                .filter_map(|(id, pending)| pending.state.result().map(|_| *id))
                .collect::<Vec<_>>();

            for id in ids {
                if let Some(pending) = self.pending.remove(&id) {
                    unsafe { pending.reclaim() };
                }
            }
        }

        fn cancel_and_reclaim_all(&mut self) {
            for pending in self.pending.values() {
                unsafe {
                    let _ =
                        rusb::ffi::libusb_cancel_transfer(pending.raw.as_ptr());
                }
            }

            while !self.pending.is_empty() {
                let _ = self.pump_events_once();
                self.reclaim_completed_without_delivery();
            }
        }
    }

    impl DeviceBackend for WindowsBe200Backend {
        fn arm_read(
            &mut self,
            endpoint: RxEndpoint,
        ) -> Result<TransferId, BackendError> {
            match endpoint {
                RxEndpoint::Event => self.submit_read(
                    endpoint,
                    self.layout.event_in,
                    TransferType::Interrupt,
                    EVENT_TRANSFER_BYTES,
                ),
                RxEndpoint::Acl => self.submit_read(
                    endpoint,
                    self.layout.acl_in,
                    TransferType::Bulk,
                    BULK_TRANSFER_BYTES,
                ),
                // Downlink-only product scope: no controller->host ISO RX path is
                // enabled until native bulk-IN demultiplexing is explicitly defined.
                RxEndpoint::Iso => Err(BackendError::Failed),
            }
        }

        fn submit_write(
            &mut self,
            endpoint: TxEndpoint,
            bytes: Vec<u8>,
        ) -> Result<TransferId, BackendError> {
            let buffer = match endpoint {
                TxEndpoint::Command => {
                    hci_command_control_buffer(&bytes)
                        .map_err(|_| BackendError::Failed)?
                }
                TxEndpoint::Acl | TxEndpoint::Iso => bytes,
            };
            self.submit_write_inner(endpoint, buffer)
        }

        fn next_completion(&mut self) -> Option<TransferCompletion> {
            if let Some(completion) = self.pop_completed() {
                return Some(completion);
            }
            if self.pending.is_empty() {
                return None;
            }

            if let Err(error) = self.pump_events_once() {
                return Some(match error {
                    BackendError::DeviceGone => TransferCompletion::DeviceGone,
                    _ => TransferCompletion::Failed { id: None },
                });
            }

            self.pop_completed()
        }

        fn cancel_all(&mut self) {
            self.cancel_and_reclaim_all();
        }

        fn is_attached(&self) -> bool {
            self.attached
        }
    }

    impl Drop for WindowsBe200Backend {
        fn drop(&mut self) {
            self.cancel_and_reclaim_all();
            let _ = self.handle.release_interface(self.layout.interface);
        }
    }

    fn discover_layout(
        device: &Device<Context>,
    ) -> Result<Be200UsbLayout, OpenError> {
        let descriptor = device.device_descriptor()?;

        for config_index in 0..descriptor.num_configurations() {
            let config = device.config_descriptor(config_index)?;
            for interface in config.interfaces() {
                for setting in interface.descriptors() {
                    // USB Bluetooth HCI: Wireless Controller / RF Controller /
                    // Bluetooth Programming Interface.
                    if setting.class_code() != 0xe0
                        || setting.sub_class_code() != 0x01
                        || setting.protocol_code() != 0x01
                    {
                        continue;
                    }

                    let mut event_in = None;
                    let mut acl_in = None;
                    let mut bulk_out = None;

                    for endpoint in setting.endpoint_descriptors() {
                        match (endpoint.transfer_type(), endpoint.direction()) {
                            (TransferType::Interrupt, Direction::In) => {
                                event_in.get_or_insert(endpoint.address());
                            }
                            (TransferType::Bulk, Direction::In) => {
                                acl_in.get_or_insert(endpoint.address());
                            }
                            (TransferType::Bulk, Direction::Out) => {
                                bulk_out.get_or_insert(endpoint.address());
                            }
                            _ => {}
                        }
                    }

                    if let (Some(event_in), Some(acl_in), Some(bulk_out)) =
                        (event_in, acl_in, bulk_out)
                    {
                        return Ok(Be200UsbLayout {
                            configuration: config.number(),
                            interface: setting.interface_number(),
                            alternate: setting.setting_number(),
                            event_in,
                            acl_in,
                            bulk_out,
                        });
                    }
                }
            }
        }

        Err(OpenError::UnsupportedLayout)
    }

    fn allocate_transfer(
    ) -> Result<NonNull<rusb::ffi::libusb_transfer>, BackendError> {
        NonNull::new(unsafe { rusb::ffi::libusb_alloc_transfer(0) })
            .ok_or(BackendError::Failed)
    }

    fn logical_write_length(
        endpoint: TxEndpoint,
        buffer: &[u8],
        actual_length: usize,
    ) -> usize {
        match endpoint {
            TxEndpoint::Command => actual_length.min(
                buffer
                    .len()
                    .saturating_sub(
                        rusb::ffi::constants::LIBUSB_CONTROL_SETUP_SIZE,
                    ),
            ),
            TxEndpoint::Acl | TxEndpoint::Iso => {
                actual_length.min(buffer.len())
            }
        }
    }

    fn map_libusb_code(code: i32) -> BackendError {
        match code {
            rusb::ffi::constants::LIBUSB_ERROR_NO_DEVICE => {
                BackendError::DeviceGone
            }
            _ => BackendError::Failed,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn target_identity_is_stable() {
            assert_eq!((BE200_VID, BE200_PID), (0x8087, 0x0036));
        }

        #[test]
        fn command_write_length_excludes_control_setup_header() {
            let buffer = hci_command_control_buffer(&[1, 2, 3]).unwrap();
            assert_eq!(
                logical_write_length(TxEndpoint::Command, &buffer, 3),
                3
            );
            assert_eq!(
                logical_write_length(TxEndpoint::Command, &buffer, 100),
                3
            );
        }

        #[test]
        fn bulk_write_length_is_clamped_to_payload() {
            let buffer = vec![1, 2, 3, 4];
            assert_eq!(
                logical_write_length(TxEndpoint::Iso, &buffer, 2),
                2
            );
            assert_eq!(
                logical_write_length(TxEndpoint::Iso, &buffer, 10),
                4
            );
        }

        #[test]
        fn submit_error_maps_device_loss_separately() {
            assert_eq!(
                map_libusb_code(
                    rusb::ffi::constants::LIBUSB_ERROR_NO_DEVICE
                ),
                BackendError::DeviceGone
            );
            assert_eq!(
                map_libusb_code(rusb::ffi::constants::LIBUSB_ERROR_BUSY),
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
