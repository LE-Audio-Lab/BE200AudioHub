use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
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
const READ_TIMEOUT: Duration = Duration::from_millis(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Product-local BE200 USB transport.
///
/// Pinned Bumble-RS prepends one H4 packet type to an entire USB transfer.
/// Bluetooth USB does not guarantee one HCI frame per transfer: one transfer
/// may contain multiple complete frames, and a frame may continue across
/// transfers. Linux btusb reassembles Event and ACL streams from their native
/// HCI headers; R2 follows the same model here.
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
        "[R2/USB] fixed HCI USB framing active: interrupt=0x{:02X} bulk-in=0x{:02X} bulk-out=0x{:02X}; Event+ACL native-header reassembly",
        layout.interrupt_in, layout.bulk_in, layout.bulk_out
    );

    let handle = Arc::new(handle);
    let source = Be200UsbPacketSource {
        handle: handle.clone(),
        interrupt_in: layout.interrupt_in,
        bulk_in: layout.bulk_in,
        event_framer: UsbHciReassembler::event(),
        acl_framer: UsbHciReassembler::acl(),
        pending: VecDeque::new(),
        next_poll_endpoint: 0,
    };
    let sink = Be200UsbPacketSink {
        handle,
        bulk_out: layout.bulk_out,
    };

    let mut metadata = BTreeMap::new();
    metadata.insert("vendor_id".into(), format!("{BE200_VID:04x}"));
    metadata.insert("product_id".into(), format!("{BE200_PID:04x}"));
    metadata.insert("bus".into(), device.bus_number().to_string());
    metadata.insert("address".into(), device.address().to_string());
    metadata.insert("be200_usb_framing".into(), "native-header-reassembly".into());

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
            pending.push_back(HciPacket::from_bytes(&h4)?);
            completed += 1;
        }

        // Match Linux btusb_recv_intr(): once an Event frame completed in this
        // USB chunk, a leftover shorter than HCI_EVENT_HDR_SIZE is treated as
        // an unexpected continuation/controller quirk and dropped instead of
        // becoming a fresh H4 packet type on the next iteration.
        if self.kind == InboundPacketKind::Event
            && completed > 0
            && !self.buffer.is_empty()
            && self.buffer.len() < self.header_len()
        {
            println!(
                "[R2/USB] dropping unexpected Event continuation: {} byte(s) {}",
                self.buffer.len(),
                hex_bytes(&self.buffer)
            );
            self.buffer.clear();
        }

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

struct Be200UsbPacketSource {
    handle: Arc<DeviceHandle<Context>>,
    interrupt_in: u8,
    bulk_in: u8,
    event_framer: UsbHciReassembler,
    acl_framer: UsbHciReassembler,
    pending: VecDeque<HciPacket>,
    next_poll_endpoint: u8,
}

impl Be200UsbPacketSource {
    fn poll_events(&mut self) -> TransportResult<()> {
        let mut transfer = vec![0u8; MAX_HCI_PACKET_SIZE - 1];
        let count = match self
            .handle
            .read_interrupt(self.interrupt_in, &mut transfer, READ_TIMEOUT)
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
        self.event_framer
            .feed(&transfer[..count], &mut self.pending)
    }

    fn poll_acl(&mut self) -> TransportResult<()> {
        let mut transfer = vec![0u8; MAX_HCI_PACKET_SIZE - 1];
        let count = match self
            .handle
            .read_bulk(self.bulk_in, &mut transfer, READ_TIMEOUT)
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
}

impl PacketSink for Be200UsbPacketSink {
    fn write_packet(&mut self, packet: &HciPacket) -> TransportResult<()> {
        let bytes = packet.to_bytes();
        let (&packet_type, payload) = bytes
            .split_first()
            .ok_or_else(|| Error::InvalidSpec("empty HCI packet".into()))?;

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
