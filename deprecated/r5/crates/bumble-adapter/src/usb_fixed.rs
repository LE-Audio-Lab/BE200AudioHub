use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use bumble_hci::{
    HciPacket, HCI_ACL_DATA_PACKET, HCI_COMMAND_PACKET, HCI_EVENT_PACKET, HCI_ISO_DATA_PACKET,
    HCI_SYNCHRONOUS_DATA_PACKET,
};
use bumble_transport::{
    select_interface_layout, Error, PacketSink, PacketSource, Result as TransportResult,
    SplitOpenedTransport, UsbEndpointInfo, UsbInterfaceInfo, MAX_HCI_PACKET_SIZE,
};
use rusb::{
    Context, Device, DeviceHandle, Direction, Recipient, RequestType, UsbContext,
};

const BE200_VID: u16 = 0x8087;
const BE200_PID: u16 = 0x0036;
const EVENT_READ_TIMEOUT: Duration = Duration::from_millis(10);
// During ISO TX, Number Of Completed Packets is the inbound traffic that
// advances controller credits. Keep interrupt-IN tight and suspend idle ACL
// bulk-IN polling until a non-ISO host packet returns us to control-plane mode.
const ISO_EVENT_READ_TIMEOUT: Duration = Duration::from_millis(1);
// Do not let an idle ACL bulk-IN read hold up HCI event credits during ISO.
// Number Of Completed Packets arrives on interrupt-IN and directly replenishes
// the controller ISO queue; a 10 ms bulk poll here can miss an entire CIS interval.
const ACL_READ_TIMEOUT: Duration = Duration::from_millis(1);
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Product-local BE200 USB transport.
///
/// Reassemble Event and ACL data from their native HCI headers. A USB read
/// boundary is not an HCI frame boundary. In particular, a timed-out interrupt
/// read may have transferred valid bytes that must reach the event assembler.
pub(crate) fn open_be200_split_transport() -> TransportResult<SplitOpenedTransport> {
    let context = Context::new()?;
    let devices = context.devices()?;

    let mut selected: Option<Device<Context>> = None;
    for device in devices.iter() {
        let Ok(descriptor) = device.device_descriptor() else {
            continue;
        };
        if descriptor.vendor_id() != BE200_VID || descriptor.product_id() != BE200_PID {
            continue;
        }
        if selected.is_some() {
            return Err(Error::InvalidSpec(
                "multiple 8087:0036 devices found; refusing ambiguous BE200 USB attach".into(),
            ));
        }
        selected = Some(device);
    }

    let device = selected.ok_or_else(|| Error::InvalidSpec("BE200 8087:0036 not found".into()))?;
    let interfaces = interface_infos(&device)?;
    let layout = select_interface_layout(&interfaces, false).ok_or_else(|| {
        Error::InvalidSpec("BE200 has no compatible Bluetooth HCI USB interface".into())
    })?;

    let handle = device.open()?;
    match handle.set_auto_detach_kernel_driver(true) {
        Ok(()) | Err(rusb::Error::NotSupported) => {}
        Err(error) => return Err(error.into()),
    }
    if handle.active_configuration().ok() != Some(layout.configuration) {
        handle.set_active_configuration(layout.configuration)?;
    }
    handle.claim_interface(layout.interface)?;
    if layout.alternate != 0 {
        handle.set_alternate_setting(layout.interface, layout.alternate)?;
    }

    println!(
        "[R2/USB] fixed HCI USB framing active: interrupt=0x{:02X} bulk-in=0x{:02X} bulk-out=0x{:02X}; Event+ACL native-header reassembly; partial-timeout preservation",
        layout.interrupt_in, layout.bulk_in, layout.bulk_out
    );
    println!("[R2/USB] ACL lifecycle event tracing enabled before HCI decode; no ACL payloads or pairing keys logged");

    let handle = Arc::new(handle);
    let iso_event_only = Arc::new(AtomicBool::new(false));
    let source = Be200UsbPacketSource {
        handle: handle.clone(),
        interrupt_in: layout.interrupt_in,
        bulk_in: layout.bulk_in,
        event_framer: UsbHciReassembler::event(),
        acl_framer: UsbHciReassembler::acl(),
        pending: VecDeque::new(),
        next_poll_endpoint: 0,
        iso_event_only: iso_event_only.clone(),
    };
    let sink = Be200UsbPacketSink {
        handle,
        bulk_out: layout.bulk_out,
        iso_event_only,
    };

    let mut metadata = BTreeMap::new();
    metadata.insert("vendor_id".into(), format!("{BE200_VID:04x}"));
    metadata.insert("product_id".into(), format!("{BE200_PID:04x}"));
    metadata.insert("bus".into(), device.bus_number().to_string());
    metadata.insert("address".into(), device.address().to_string());
    metadata.insert("be200_usb_framing".into(), "native-header-reassembly".into());
    metadata.insert("be200_interrupt_partial_timeout".into(), "preserve".into());

    Ok(SplitOpenedTransport {
        source: Box::new(source),
        sink: Box::new(sink),
        metadata,
    })
}

fn interface_infos<T: UsbContext>(device: &Device<T>) -> TransportResult<Vec<UsbInterfaceInfo>> {
    let descriptor = device.device_descriptor()?;
    let mut infos = Vec::new();

    for config_index in 0..descriptor.num_configurations() {
        let config = device.config_descriptor(config_index)?;
        for interface in config.interfaces() {
            for setting in interface.descriptors() {
                infos.push(UsbInterfaceInfo {
                    configuration: config.number(),
                    interface: setting.interface_number(),
                    alternate: setting.setting_number(),
                    class: setting.class_code(),
                    subclass: setting.sub_class_code(),
                    protocol: setting.protocol_code(),
                    endpoints: setting
                        .endpoint_descriptors()
                        .map(|endpoint| UsbEndpointInfo {
                            address: endpoint.address(),
                            direction: endpoint.direction(),
                            transfer_type: endpoint.transfer_type(),
                            max_packet_size: endpoint.max_packet_size(),
                        })
                        .collect(),
                });
            }
        }
    }

    Ok(infos)
}

/// rusb 0.9.4 preserves partial TIMEOUT results for bulk reads but not for
/// interrupt reads. Call the same synchronous libusb API while retaining its
/// transferred count. No background transfer survives this function.
fn read_interrupt_preserving_progress<T: UsbContext>(
    handle: &DeviceHandle<T>,
    endpoint: u8,
    buffer: &mut [u8],
    timeout: Duration,
) -> TransportResult<usize> {
    if endpoint & 0x80 == 0 || buffer.is_empty() {
        return Err(Error::InvalidSpec(
            "interrupt receive requires an IN endpoint and a nonempty buffer".into(),
        ));
    }
    let length = i32::try_from(buffer.len()).map_err(|_| {
        Error::InvalidSpec("interrupt receive buffer exceeds libusb length range".into())
    })?;
    let timeout_ms = u32::try_from(timeout.as_millis()).map_err(|_| {
        Error::InvalidSpec("interrupt receive timeout exceeds libusb range".into())
    })?;
    if timeout_ms == 0 {
        return Err(Error::InvalidSpec(
            "interrupt receive requires a finite timeout of at least 1 ms".into(),
        ));
    }

    let mut transferred: i32 = 0;
    // SAFETY: the borrowed handle remains open, buffer is exclusively borrowed
    // for this synchronous call, its writable allocation is at least length
    // bytes, and transferred points to initialized, live i32 storage. Length
    // and timeout conversions are checked. libusb retains none of these
    // pointers after its synchronous function returns.
    let status = unsafe {
        rusb::ffi::libusb_interrupt_transfer(
            handle.as_raw(),
            endpoint,
            buffer.as_mut_ptr(),
            length,
            &mut transferred,
            timeout_ms,
        )
    };
    let count = interrupt_read_result(status, transferred, buffer.len())?;
    if status != 0 && count > 0 {
        println!(
            "[R2/USB] interrupt-IN partial read preserved: endpoint=0x{endpoint:02X} libusb_status={status} bytes={count}; forwarding received bytes"
        );
    }
    Ok(count)
}

fn interrupt_read_result(
    status: i32,
    transferred: i32,
    capacity: usize,
) -> TransportResult<usize> {
    use rusb::constants::{LIBUSB_ERROR_INTERRUPTED, LIBUSB_ERROR_TIMEOUT};

    // Do not consume data from overflow, removal, stall, or other hard errors.
    if status != 0 && status != LIBUSB_ERROR_TIMEOUT && status != LIBUSB_ERROR_INTERRUPTED {
        return Err(Error::Remote(format!(
            "BE200 interrupt-IN failed: libusb_status={status} transferred={transferred}"
        )));
    }
    let count = usize::try_from(transferred).map_err(|_| {
        Error::Remote("libusb interrupt-IN returned a negative byte count".into())
    })?;
    if count > capacity {
        return Err(Error::Remote(format!(
            "libusb interrupt-IN returned {count} bytes for a {capacity}-byte buffer"
        )));
    }
    // TIMEOUT/INTERRUPTED + zero bytes is just an empty polling slice.
    Ok(count)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InboundPacketKind {
    Event,
    Acl,
}

struct UsbHciReassembler {
    kind: InboundPacketKind,
    buffer: Vec<u8>,
}

impl UsbHciReassembler {
    fn event() -> Self {
        Self {
            kind: InboundPacketKind::Event,
            buffer: Vec::new(),
        }
    }

    fn acl() -> Self {
        Self {
            kind: InboundPacketKind::Acl,
            buffer: Vec::new(),
        }
    }

    fn header_len(&self) -> usize {
        match self.kind {
            InboundPacketKind::Event => 2,
            InboundPacketKind::Acl => 4,
        }
    }

    fn packet_type(&self) -> u8 {
        match self.kind {
            InboundPacketKind::Event => HCI_EVENT_PACKET,
            InboundPacketKind::Acl => HCI_ACL_DATA_PACKET,
        }
    }

    fn packet_len_from_header(&self) -> usize {
        match self.kind {
            InboundPacketKind::Event => 2 + usize::from(self.buffer[1]),
            InboundPacketKind::Acl => {
                4 + usize::from(u16::from_le_bytes([self.buffer[2], self.buffer[3]]))
            }
        }
    }

    fn feed(
        &mut self,
        transfer: &[u8],
        pending: &mut VecDeque<HciPacket>,
    ) -> TransportResult<()> {
        if transfer.is_empty() {
            return Ok(());
        }

        self.buffer.extend_from_slice(transfer);
        let mut completed = 0usize;

        loop {
            if self.buffer.len() < self.header_len() {
                break;
            }

            let packet_len = self.packet_len_from_header();
            if packet_len + 1 > MAX_HCI_PACKET_SIZE {
                self.buffer.clear();
                return Err(Error::PacketTooLarge(packet_len + 1));
            }
            if self.buffer.len() < packet_len {
                break;
            }

            let packet_bytes: Vec<u8> = self.buffer.drain(..packet_len).collect();
            let mut h4 = Vec::with_capacity(packet_len + 1);
            h4.push(self.packet_type());
            h4.extend_from_slice(&packet_bytes);
            if self.kind == InboundPacketKind::Event {
                trace_acl_lifecycle_event(&h4);
            }
            pending.push_back(HciPacket::from_bytes(&h4)?);
            completed += 1;
        }

        // Retain every incomplete header, including a single Event header byte
        // following a completed frame. Dropping it would desynchronize the next
        // read. Unknown/malformed data must fail decoding, not be guessed away.
        if completed > 1 {
            println!(
                "[R2/USB] one {} transfer contained {completed} complete HCI frame(s)",
                match self.kind {
                    InboundPacketKind::Event => "interrupt",
                    InboundPacketKind::Acl => "bulk-IN",
                }
            );
        }

        if self.buffer.len() + 1 > MAX_HCI_PACKET_SIZE {
            let buffered = self.buffer.len();
            self.buffer.clear();
            return Err(Error::PacketTooLarge(buffered + 1));
        }

        Ok(())
    }
}

/// Select only non-secret connection/control events. Never dump ACL/SMP data,
/// vendor responses, local key reads, or general Command Complete payloads.
fn is_acl_lifecycle_event(h4: &[u8]) -> bool {
    if h4.len() < 3 || h4[0] != HCI_EVENT_PACKET {
        return false;
    }
    match h4[1] {
        0x05 | 0x08 | 0x10 | 0x30 | 0x59 => true,
        0x3E => h4.len() >= 4 && matches!(h4[3], 0x01 | 0x03 | 0x04 | 0x0A | 0x29),
        0x0E if h4.len() >= 6 => {
            is_lifecycle_opcode(u16::from_le_bytes([h4[4], h4[5]]))
        }
        0x0F if h4.len() >= 7 => {
            is_lifecycle_opcode(u16::from_le_bytes([h4[5], h4[6]]))
        }
        _ => false,
    }
}

fn is_lifecycle_opcode(opcode: u16) -> bool {
    matches!(opcode, 0x0406 | 0x200D | 0x200E | 0x2019 | 0x2043)
}

fn trace_acl_lifecycle_event(h4: &[u8]) {
    if !is_acl_lifecycle_event(h4) {
        return;
    }
    let label = match h4[1] {
        0x05 => "Disconnection Complete",
        0x08 | 0x59 => "Encryption Change",
        0x10 => "Hardware Error",
        0x30 => "Encryption Key Refresh Complete",
        0x0E => "Command Complete",
        0x0F => "Command Status",
        0x3E => match h4[3] {
            0x01 => "LE Connection Complete",
            0x0A => "LE Enhanced Connection Complete",
            0x29 => "LE Enhanced Connection Complete V2",
            0x03 => "LE Connection Update Complete",
            0x04 => "LE Read Remote Features Complete",
            _ => "LE connection event",
        },
        _ => "connection event",
    };
    println!(
        "[R2/HCI-RX] {label} before Host/Device decode: h4={}",
        hex_bytes(h4)
    );
}

struct Be200UsbPacketSource {
    handle: Arc<DeviceHandle<Context>>,
    interrupt_in: u8,
    bulk_in: u8,
    event_framer: UsbHciReassembler,
    acl_framer: UsbHciReassembler,
    pending: VecDeque<HciPacket>,
    next_poll_endpoint: u8,
    iso_event_only: Arc<AtomicBool>,
}

impl Be200UsbPacketSource {
    fn poll_events(&mut self) -> TransportResult<()> {
        let mut transfer = vec![0u8; MAX_HCI_PACKET_SIZE - 1];
        let timeout = if self.iso_event_only.load(Ordering::Acquire) {
            ISO_EVENT_READ_TIMEOUT
        } else {
            EVENT_READ_TIMEOUT
        };
        let count = read_interrupt_preserving_progress(
            self.handle.as_ref(),
            self.interrupt_in,
            &mut transfer,
            timeout,
        )?;
        if count == 0 {
            return Ok(());
        }
        self.event_framer
            .feed(&transfer[..count], &mut self.pending)
    }

    fn poll_acl(&mut self) -> TransportResult<()> {
        let mut transfer = vec![0u8; MAX_HCI_PACKET_SIZE - 1];
        let count = match self
            .handle
            .read_bulk(self.bulk_in, &mut transfer, ACL_READ_TIMEOUT)
        {
            Ok(count) => count,
            Err(rusb::Error::Timeout) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            return Ok(());
        }
        if count > transfer.len() {
            return Err(Error::PacketTooLarge(count + 1));
        }
        self.acl_framer.feed(&transfer[..count], &mut self.pending)
    }

    fn poll_next_endpoint(&mut self) -> TransportResult<()> {
        if self.iso_event_only.load(Ordering::Acquire) {
            return self.poll_events();
        }
        let endpoint = self.next_poll_endpoint;
        self.next_poll_endpoint ^= 1;
        if endpoint == 0 {
            self.poll_events()
        } else {
            self.poll_acl()
        }
    }
}

impl PacketSource for Be200UsbPacketSource {
    fn read_packet(&mut self) -> TransportResult<Option<HciPacket>> {
        if let Some(packet) = self.pending.pop_front() {
            return Ok(Some(packet));
        }

        loop {
            self.poll_next_endpoint()?;
            if let Some(packet) = self.pending.pop_front() {
                return Ok(Some(packet));
            }
        }
    }
}

struct Be200UsbPacketSink {
    handle: Arc<DeviceHandle<Context>>,
    bulk_out: u8,
    iso_event_only: Arc<AtomicBool>,
}

impl PacketSink for Be200UsbPacketSink {
    fn write_packet(&mut self, packet: &HciPacket) -> TransportResult<()> {
        let bytes = packet.to_bytes();
        let (&packet_type, payload) = bytes
            .split_first()
            .ok_or_else(|| Error::InvalidSpec("empty HCI packet".into()))?;

        // Steady-state CIS streaming still requires HCI Event traffic for ISO
        // credits, but it does not need inbound ACL polling. Enter event-only
        // mode on ISO; any later command/ACL packet restores normal receive mode.
        if packet_type == HCI_ISO_DATA_PACKET {
            self.iso_event_only.store(true, Ordering::Release);
        } else {
            self.iso_event_only.store(false, Ordering::Release);
        }

        let count = match packet_type {
            HCI_COMMAND_PACKET => self.handle.write_control(
                rusb::request_type(Direction::Out, RequestType::Class, Recipient::Device),
                0,
                0,
                0,
                payload,
                WRITE_TIMEOUT,
            ),
            HCI_ACL_DATA_PACKET | HCI_ISO_DATA_PACKET => {
                self.handle.write_bulk(self.bulk_out, payload, WRITE_TIMEOUT)
            }
            HCI_SYNCHRONOUS_DATA_PACKET => {
                return Err(Error::Unsupported(
                    "BE200 R2 fixed USB transport does not enable USB SCO/eSCO isochronous I/O"
                        .into(),
                ));
            }
            other => return Err(Error::InvalidPacketType(other)),
        }?;

        if count != payload.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                format!("partial BE200 USB HCI write {count}/{}", payload.len()),
            )
            .into());
        }

        Ok(())
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusb::constants::{
        LIBUSB_ERROR_INTERRUPTED, LIBUSB_ERROR_NO_DEVICE, LIBUSB_ERROR_OVERFLOW,
        LIBUSB_ERROR_TIMEOUT,
    };

    const STATUS: [u8; 6] = [0x0F, 0x04, 0x00, 0x01, 0x43, 0x20];
    const DISCONNECTED: [u8; 6] = [0x05, 0x04, 0x00, 0x00, 0x08, 0x13];

    fn framed_event(bytes: &[u8]) -> Vec<u8> {
        let mut h4 = vec![HCI_EVENT_PACKET];
        h4.extend_from_slice(bytes);
        h4
    }

    #[test]
    fn interrupt_timeout_preserves_received_bytes() {
        assert_eq!(interrupt_read_result(LIBUSB_ERROR_TIMEOUT, 33, 260).unwrap(), 33);
        assert_eq!(interrupt_read_result(LIBUSB_ERROR_TIMEOUT, 0, 260).unwrap(), 0);
        assert_eq!(interrupt_read_result(LIBUSB_ERROR_INTERRUPTED, 7, 260).unwrap(), 7);
        assert_eq!(interrupt_read_result(0, 6, 260).unwrap(), 6);
    }

    #[test]
    fn interrupt_hard_errors_and_invalid_lengths_are_not_consumed() {
        assert!(interrupt_read_result(LIBUSB_ERROR_NO_DEVICE, 6, 260).is_err());
        assert!(interrupt_read_result(LIBUSB_ERROR_OVERFLOW, 260, 260).is_err());
        assert!(interrupt_read_result(0, -1, 260).is_err());
        assert!(interrupt_read_result(LIBUSB_ERROR_TIMEOUT, 261, 260).is_err());
    }

    #[test]
    fn event_header_byte_after_complete_frame_is_retained() {
        let mut framer = UsbHciReassembler::event();
        let mut pending = VecDeque::new();
        let mut first_read = STATUS.to_vec();
        first_read.push(DISCONNECTED[0]);
        framer.feed(&first_read, &mut pending).unwrap();
        assert_eq!(pending.pop_front().unwrap().to_bytes(), framed_event(&STATUS));
        assert_eq!(framer.buffer, [DISCONNECTED[0]]);
        framer.feed(&DISCONNECTED[1..], &mut pending).unwrap();
        assert_eq!(pending.pop_front().unwrap().to_bytes(), framed_event(&DISCONNECTED));
        assert!(framer.buffer.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn event_reassembly_is_independent_of_every_two_chunk_boundary() {
        let mut stream = STATUS.to_vec();
        stream.extend_from_slice(&DISCONNECTED);
        for split in 0..=stream.len() {
            let mut framer = UsbHciReassembler::event();
            let mut pending = VecDeque::new();
            framer.feed(&stream[..split], &mut pending).unwrap();
            framer.feed(&stream[split..], &mut pending).unwrap();
            let actual: Vec<_> = pending.into_iter().map(|packet| packet.to_bytes()).collect();
            assert_eq!(actual, vec![framed_event(&STATUS), framed_event(&DISCONNECTED)]);
            assert!(framer.buffer.is_empty());
        }
    }

    #[test]
    fn event_reassembly_accepts_one_byte_reads() {
        let mut framer = UsbHciReassembler::event();
        let mut pending = VecDeque::new();
        for byte in STATUS {
            framer.feed(&[byte], &mut pending).unwrap();
        }
        assert_eq!(pending.pop_front().unwrap().to_bytes(), framed_event(&STATUS));
        assert!(framer.buffer.is_empty());
    }

    #[test]
    fn lifecycle_trace_excludes_acl_vendor_and_key_responses() {
        assert!(is_acl_lifecycle_event(&framed_event(&STATUS)));
        assert!(is_acl_lifecycle_event(&framed_event(&DISCONNECTED)));
        assert!(!is_acl_lifecycle_event(&[0x02, 0x00, 0x20, 0x01, 0x00, 0x08]));
        assert!(!is_acl_lifecycle_event(&[0x04, 0xFF, 0x01, 0x00]));
        assert!(!is_acl_lifecycle_event(&[0x04, 0x0E, 0x04, 0x01, 0x15, 0xFC, 0x00]));
        assert!(!is_acl_lifecycle_event(&[0x04, 0x3E, 0x01, 0x0D]));
        assert!(!is_acl_lifecycle_event(&[]));
    }
}
