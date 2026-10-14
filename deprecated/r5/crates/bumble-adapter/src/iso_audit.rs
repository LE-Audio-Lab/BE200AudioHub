//! Packet-boundary evidence for R5.3. No PCM, LC3 bytes, ACL payloads or keys logged.
//!
//! The upstream queue deliberately ignores completion-counter errors. Keep an
//! independent, strict ledger so an overflow/implicit disconnect flush cannot
//! masquerade as successful ISO delivery. HCI completion is NOT audibility.
use std::sync::{Arc, Mutex};
use bumble_hci::{Event, HciPacket, IsoDataPacket};
use bumble_transport::{Error, PacketSink, PacketSource, Result as TransportResult, SplitOpenedTransport};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IsoTxStats {
    pub handles: [u16; 2],
    pub submitted: [u64; 2],
    pub completed: [u64; 2],
    pub completion_events: u64,
    pub peak_pending: u64,
    pub failure: Option<String>,
}
impl IsoTxStats {
    pub fn pending(&self) -> u64 {
        self.submitted.iter().sum::<u64>().saturating_sub(self.completed.iter().sum::<u64>())
    }
    pub fn check(&self) -> Result<(), String> {
        match &self.failure { Some(e) => Err(e.clone()), None => Ok(()) }
    }
    pub fn verify_complete(&self, frames: u64) -> Result<(), String> {
        self.check()?;
        if frames == 0 || self.submitted != [frames; 2] || self.completed != [frames; 2] {
            return Err(format!("ISO packet evidence incomplete: expected={frames}/ear submitted={:?} completed={:?}", self.submitted, self.completed));
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct IsoAudit {
    active: bool,
    capacity: u64,
    next_sequence: [u16; 2],
    stats: IsoTxStats,
}
pub(crate) type SharedIsoAudit = Arc<Mutex<IsoAudit>>;

impl IsoAudit {
    pub(crate) fn begin(&mut self, handles: [u16;2], capacity: usize) -> Result<(), String> {
        if self.active || handles[0] == handles[1] || capacity < 2
            || handles.iter().any(|&h| h > 0x0EFF)
        { return Err("invalid or already active stereo ISO audit".into()); }
        self.active = true;
        self.capacity = capacity as u64;
        self.next_sequence = [0;2];
        self.stats = IsoTxStats { handles, ..IsoTxStats::default() };
        Ok(())
    }
    pub(crate) fn snapshot(&self) -> IsoTxStats { self.stats.clone() }
    pub(crate) fn stop(&mut self) { self.active = false; }
    fn fail(&mut self, error: String) {
        if self.active && self.stats.failure.is_none() { self.stats.failure = Some(error); }
    }
    fn submit(&mut self, packet: &IsoDataPacket) -> Result<(), String> {
        if !self.active { return Ok(()); }
        self.stats.check()?;
        let side = self.stats.handles.iter().position(|h| *h == packet.connection_handle)
            .ok_or_else(|| format!("unplanned ISO handle 0x{:04X}", packet.connection_handle))?;
        if packet.pb_flag != 2 || packet.ts_flag != 0 || packet.time_stamp.is_some()
            || packet.packet_sequence_number != Some(self.next_sequence[side])
            || packet.iso_sdu_length != Some(100) || packet.packet_status_flag != Some(0)
            || packet.iso_sdu_fragment.len() != 100 || packet.data_total_length != 104
        { return Err(format!("invalid R5.3 ISO envelope/sequence on handle 0x{:04X}", packet.connection_handle)); }
        if self.stats.pending() >= self.capacity {
            return Err("ISO USB submission would exceed reported controller packet credits".into());
        }
        // Reserve BEFORE the synchronous USB write: the reader thread may see
        // its completion before write_packet returns. Any write error poisons
        // this run; it is never retried and never counted as a successful probe.
        self.stats.submitted[side] += 1;
        self.next_sequence[side] = self.next_sequence[side].wrapping_add(1);
        self.stats.peak_pending = self.stats.peak_pending.max(self.stats.pending());
        Ok(())
    }
    fn observe(&mut self, packet: &HciPacket) {
        if !self.active { return; }
        if let HciPacket::Event(Event::NumberOfCompletedPackets { connection_handles, num_completed_packets }) = packet {
            if connection_handles.len() != num_completed_packets.len() {
                self.fail("malformed Number Of Completed Packets vector lengths".into());
                return;
            }
            for (&handle, &count) in connection_handles.iter().zip(num_completed_packets) {
                let Some(side) = self.stats.handles.iter().position(|h| *h == handle) else { continue; };
                let count = u64::from(count);
                if count > self.stats.submitted[side].saturating_sub(self.stats.completed[side]) {
                    self.fail(format!("ISO credit overflow handle=0x{handle:04X} completed={count} outstanding={}",
                        self.stats.submitted[side].saturating_sub(self.stats.completed[side])));
                    return;
                }
                if count > 0 { self.stats.completion_events += 1; }
                self.stats.completed[side] += count;
            }
        }
    }
}

struct AuditedSource { inner: Box<dyn PacketSource + Send>, audit: SharedIsoAudit }
impl PacketSource for AuditedSource {
    fn read_packet(&mut self) -> TransportResult<Option<HciPacket>> {
        let result = self.inner.read_packet();
        let mut audit = self.audit.lock().map_err(|_| Error::Remote("ISO audit mutex poisoned".into()))?;
        match &result {
            Ok(Some(packet)) => audit.observe(packet),
            Ok(None) => audit.fail("USB reader ended during ISO probe".into()),
            Err(e) => audit.fail(format!("USB reader failed during ISO probe: {e}")),
        }
        result
    }
}
struct AuditedSink { inner: Box<dyn PacketSink + Send>, audit: SharedIsoAudit }
impl PacketSink for AuditedSink {
    fn write_packet(&mut self, packet: &HciPacket) -> TransportResult<()> {
        if let HciPacket::IsoData(iso) = packet {
            let mut audit = self.audit.lock().map_err(|_| Error::Remote("ISO audit mutex poisoned".into()))?;
            if let Err(e) = audit.submit(iso) {
                audit.fail(e.clone()); return Err(Error::Remote(e));
            }
        }
        let result = self.inner.write_packet(packet);
        if let Err(ref e) = result {
            if let Ok(mut audit) = self.audit.lock() { audit.fail(format!("USB write failed: {e}")); }
        }
        result
    }
    fn flush(&mut self) -> TransportResult<()> { self.inner.flush() }
}
pub(crate) fn wrap_transport(transport: SplitOpenedTransport, audit: SharedIsoAudit) -> SplitOpenedTransport {
    SplitOpenedTransport {
        source: Box::new(AuditedSource { inner: transport.source, audit: audit.clone() }),
        sink: Box::new(AuditedSink { inner: transport.sink, audit }),
        metadata: transport.metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(handle: u16, seq: u16) -> IsoDataPacket {
        IsoDataPacket { connection_handle: handle, pb_flag:2, ts_flag:0, data_total_length:104,
            time_stamp:None, packet_sequence_number:Some(seq), iso_sdu_length:Some(100),
            packet_status_flag:Some(0), iso_sdu_fragment:vec![0;100] }
    }
    fn complete(handle: u16, count: u16) -> HciPacket {
        HciPacket::Event(Event::NumberOfCompletedPackets { connection_handles:vec![handle], num_completed_packets:vec![count] })
    }
    #[test]
    fn framing_is_one_108_byte_native_usb_packet_without_h4_type() {
        let bytes=HciPacket::IsoData(packet(0x0900,0)).to_bytes();
        assert_eq!(&bytes[..9], &[5,0,0x29,0x68,0,0,0,100,0]);
        assert_eq!(bytes[1..].len(), 108);
    }
    #[test]
    fn completions_are_per_handle_not_an_aggregate_success_counter() {
        let mut a=IsoAudit::default(); a.begin([0x900,0x901],8).unwrap();
        a.submit(&packet(0x900,0)).unwrap(); a.submit(&packet(0x901,0)).unwrap();
        a.observe(&complete(0x0800,99)); // ACL credit, unrelated.
        a.observe(&complete(0x901,1)); assert!(a.stats.verify_complete(1).is_err());
        a.observe(&complete(0x900,1)); a.stats.verify_complete(1).unwrap();
        a.observe(&complete(0x900,1)); assert!(a.stats.check().is_err());
    }
    #[test]
    fn duplicate_sequence_and_overfull_submission_are_rejected() {
        let mut a=IsoAudit::default(); a.begin([0x900,0x901],2).unwrap();
        a.submit(&packet(0x900,0)).unwrap();
        assert!(a.submit(&packet(0x900,0)).is_err());
        a.submit(&packet(0x901,0)).unwrap();
        assert!(a.submit(&packet(0x900,1)).is_err());
    }
    #[test]
    fn sequence_wrap_is_valid_and_write_failure_cannot_pass() {
        let mut a=IsoAudit::default(); a.begin([0x900,0x901],2).unwrap();
        a.next_sequence[0]=u16::MAX;
        a.submit(&packet(0x900,u16::MAX)).unwrap(); a.observe(&complete(0x900,1));
        a.submit(&packet(0x900,0)).unwrap();
        a.fail("partial USB write".into()); assert!(a.stats.check().is_err());
    }
}
