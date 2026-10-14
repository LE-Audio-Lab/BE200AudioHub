//! R5.3 host-to-controller ISO input paths and bounded dual-CIS transmission.
//! Uses the pinned Device's command journal, packetizer and credit queue.
use std::time::{Duration, Instant};
use bumble_hci::metadata::supported_command_names;
use bumble_host::{IsoControlEvent, IsoDataPathParameters, IsoTxSyncInfo};
use bumble_transport::{ExternalHostActivity, ExternalHostState};
use crate::{AttachedController, IsoTxStats, ProbeGuard};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_SLICE: Duration = Duration::from_millis(2);
const INPUT: u8 = 0; // Setup: enumeration, Host -> Controller.
const REMOVE_INPUT_MASK: u8 = 1; // Remove: bitmask, NOT the Setup enumeration.

#[derive(Debug)]
pub struct IsoPathFailure { pub message: String, pub may_have_configured: bool }
impl std::fmt::Display for IsoPathFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for IsoPathFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoStreamReport {
    pub frame_pairs: u64,
    pub elapsed_ms: u64,
    pub max_lateness_us: u64,
    pub tx: IsoTxStats,
    pub tx_sync: [Option<IsoTxSyncInfo>; 2],
}

impl AttachedController {
    /// Fail before allocating paths if explicit teardown is not advertised.
    pub fn require_iso_remove_support(&self) -> Result<(), String> {
        if !supported_command_names(&self.info.supported_commands).contains(&"HCI_LE_REMOVE_ISO_DATA_PATH_COMMAND") {
            return Err("controller does not advertise LE Remove ISO Data Path".into());
        }
        Ok(())
    }
    fn poll_iso_transport(&mut self) -> Result<(), String> {
        self.device.poll(&mut self.host);
        match self.host.state() {
            ExternalHostState::Running => Ok(()),
            state => Err(format!("ISO transport is not running: {state:?}")),
        }
    }
    fn wait_iso_activity(&mut self, timeout: Duration) -> Result<(), String> {
        if timeout.is_zero() { return Ok(()); }
        match self.host.wait_for_device_activity(&mut self.device, timeout)
            .map_err(|e| format!("ISO event wait: {e}"))? {
            ExternalHostActivity::Packet | ExternalHostActivity::Timeout => Ok(()),
            ExternalHostActivity::Ended => Err("ISO transport ended".into()),
        }
    }
    fn require_iso_links(&self, handles: [u16;2], acls: [u16;2]) -> Result<(), String> {
        for (&cis,&acl) in handles.iter().zip(acls.iter()) {
            let state=self.le_acl_state(acl);
            if !state.connected || !state.encrypted || self.device.cis_link(cis).is_none() {
                return Err(format!("ISO link lost: CIS=0x{cis:04X} ACL=0x{acl:04X} encrypted={}",state.encrypted));
            }
        }
        Ok(())
    }

    /// Setup input path 0, HCI data path 0, transparent codec. LC3 encoding is
    /// on the Host; using LC3 coding format here would request controller DSP.
    pub fn setup_iso_input_path(&mut self, handle: u16) -> Result<(), IsoPathFailure> {
        let mut may_have_configured=false;
        let result=(|| -> Result<(),String> {
            self.poll_iso_transport()?;
            if self.device.cis_link(handle).is_none() || self.device.iso_data_path(handle,INPUT).is_some() {
                return Err(format!("CIS 0x{handle:04X} is absent or already has an ISO input path"));
            }
            let stale=self.device.take_iso_control_events();
            if !stale.is_empty() { return Err(format!("stale ISO control journal before Setup: {stale:?}")); }
            if !self.device.setup_iso_data_path(&mut self.host,handle,INPUT) {
                return Err(format!("Device refused Setup ISO Data Path 0x{handle:04X}"));
            }
            may_have_configured=true;
            println!("[R5.3-A/SETUP] handle=0x{handle:04X} direction=0 HCI=0 codec=TRANSPARENT");
            let deadline=Instant::now()+COMMAND_TIMEOUT;
            loop {
                self.poll_iso_transport()?;
                for event in self.device.take_iso_control_events() {
                    match event {
                        IsoControlEvent::DataPathSetup {status,connection_handle,parameters}
                            if connection_handle==handle && parameters==IsoDataPathParameters::hci(INPUT) => {
                            if status!=0 {
                                may_have_configured=false;
                                return Err(format!("Setup ISO Data Path handle=0x{handle:04X} rejected status=0x{status:02X}"));
                            }
                            if self.device.iso_data_path(handle,INPUT)!=Some(&parameters) {
                                return Err("Setup success without matching Device path state".into());
                            }
                            println!("[R5.3-A/SETUP] handle=0x{handle:04X} CommandComplete + Device state PASS");
                            return Ok(());
                        }
                        other => return Err(format!("unexpected Setup ISO completion: {other:?}")),
                    }
                }
                let remaining=deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() { return Err(format!("Setup ISO Data Path 0x{handle:04X} timed out; path may exist")); }
                self.wait_iso_activity(remaining.min(Duration::from_millis(50)))?;
            }
        })();
        result.map_err(|message| IsoPathFailure {message,may_have_configured})
    }

    /// Called before CIS disconnect. Also permits a late successful Setup event
    /// to arrive after a Setup timeout. Never treats an unobserved removal as PASS.
    pub fn remove_iso_input_path(&mut self, handle: u16) -> Result<(), String> {
        let deadline=Instant::now()+COMMAND_TIMEOUT;
        loop {
            self.poll_iso_transport()?;
            for event in self.device.take_iso_control_events() {
                match event {
                    IsoControlEvent::DataPathSetup {connection_handle,..} if connection_handle==handle => {},
                    other => return Err(format!("unexpected ISO event before Remove: {other:?}")),
                }
            }
            if self.device.iso_data_path(handle,INPUT).is_some() { break; }
            if self.device.cis_link(handle).is_none() {
                return Err(format!("CIS 0x{handle:04X} disappeared before explicit ISO Remove could be verified"));
            }
            let remaining=deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err(format!("ISO input path 0x{handle:04X} setup state remains uncertain; explicit Remove unconfirmed")); }
            self.wait_iso_activity(remaining.min(Duration::from_millis(50)))?;
        }
        if !self.device.remove_iso_data_path(&mut self.host,handle,REMOVE_INPUT_MASK) {
            return Err(format!("Device refused Remove ISO input path 0x{handle:04X}"));
        }
        let deadline=Instant::now()+COMMAND_TIMEOUT;
        loop {
            self.poll_iso_transport()?;
            for event in self.device.take_iso_control_events() {
                match event {
                    IsoControlEvent::DataPathRemoved {status,connection_handle,directions}
                        if connection_handle==handle && directions==REMOVE_INPUT_MASK => {
                        if status!=0 { return Err(format!("Remove ISO Data Path 0x{handle:04X}: status=0x{status:02X}")); }
                        if self.device.iso_data_path(handle,INPUT).is_some() { return Err("Remove ISO succeeded but Device path remains".into()); }
                        println!("[R5.3-A/REMOVE] handle=0x{handle:04X} mask=0x01 CommandComplete + Device state PASS");
                        return Ok(());
                    }
                    other => return Err(format!("unexpected ISO Remove completion: {other:?}")),
                }
            }
            let remaining=deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err(format!("Remove ISO Data Path 0x{handle:04X} timed out")); }
            self.wait_iso_activity(remaining.min(Duration::from_millis(50)))?;
        }
    }

    pub fn iso_tx_snapshot(&self) -> Result<IsoTxStats,String> {
        self.iso_audit.lock().map(|a|a.snapshot()).map_err(|_|"ISO audit mutex poisoned".into())
    }

    /// Drain while links still exist. A disconnect implicitly flushes upstream
    /// queues, so checking drained AFTER disconnect would be a false success.
    pub fn drain_iso_output(&mut self, handles: [u16;2]) -> Result<(),String> {
        let deadline=Instant::now()+DRAIN_TIMEOUT;
        loop {
            self.poll_iso_transport()?;
            if handles.iter().any(|&h| self.device.cis_link(h).is_none()) {
                return Err("CIS lost before ISO drain; implicit flush is not completion evidence".into());
            }
            if handles.iter().all(|&h| self.device.iso_output_is_drained(h)) { return Ok(()); }
            let remaining=deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err(format!("ISO credit drain timed out: pending={:?}",self.device.iso_packets_pending())); }
            self.wait_iso_activity(remaining.min(POLL_SLICE))?;
        }
    }

    /// Bounded synthetic First Sound sender.
    ///
    /// AOSP drives LC3 from an independent media tick and leaves HCI flow
    /// control below that clock. Bumble-RS already implements the lower half:
    /// send_iso_sdu() appends to a host queue, while Number Of Completed
    /// Packets only releases controller credits and flushes more queued HCI ISO
    /// packets. Keep a small software backlog here instead of making credit
    /// return timing the 10 ms media clock.
    pub fn transmit_stereo_iso(
        &mut self, handles:[u16;2], acls:[u16;2], frames:&[(&[u8],&[u8])],
    ) -> Result<IsoStreamReport,String> {
        const SOFTWARE_BACKLOG_PAIRS: usize = 10; // 100 ms at one pair / 10 ms.
        const PROGRESS_TIMEOUT: Duration = Duration::from_millis(750);
        if handles[0]==handles[1] || acls[0]==acls[1] || frames.is_empty() || frames.len()>3_000
            || frames.iter().any(|(l,r)|l.len()!=100 || r.len()!=100)
        { return Err("invalid bounded first-sound stereo input".into()); }
        self.require_iso_links(handles,acls)?;
        for handle in handles {
            if self.device.iso_data_path(handle,INPUT)!=Some(&IsoDataPathParameters::hci(INPUT)) {
                return Err(format!("ISO input path 0x{handle:04X} not verified before audio"));
            }
        }
        let capacity=self.device.iso_max_in_flight().ok_or("missing controller ISO credit pool")?;
        if capacity<2 || self.device.iso_packets_pending()!=Some(0) {
            return Err("ISO queue nonempty or too small for stereo startup".into());
        }
        self.iso_audit.lock().map_err(|_|"ISO audit mutex poisoned")?.begin(handles,capacity)?;
        let result=(|| -> Result<IsoStreamReport,String> {
            let start=Instant::now();
            let target_pending=(SOFTWARE_BACKLOG_PAIRS*2).max(capacity);
            let mut next_frame=0usize;
            let mut last_progress=Instant::now();
            let mut last_completed=0u64;
            let mut next_report=100u64;

            println!(
                "[R5.3-C/QUEUE] controller_window={} packets; software_target={} packets (~{}ms); source clock is decoupled from HCI credit events",
                capacity,target_pending,SOFTWARE_BACKLOG_PAIRS*10
            );

            loop {
                ProbeGuard::check_cancelled()?;
                self.require_iso_links(handles,acls)?;

                // Refill an alternating L/R FIFO up to a bounded backlog. Do
                // not wait for a particular credit count before producing the
                // next SDU pair; Bumble's DataPacketQueue owns that flow control.
                while next_frame<frames.len() {
                    let pending=self.device.iso_packets_pending().ok_or("ISO queue disappeared while refilling")?;
                    if pending.saturating_add(2)>target_pending { break; }
                    let (left,right)=frames[next_frame];
                    if !self.device.send_iso_sdu(&mut self.host,handles[0],left) {
                        return Err(format!("left ISO SDU enqueue failed at frame {next_frame}"));
                    }
                    if !self.device.send_iso_sdu(&mut self.host,handles[1],right) {
                        return Err(format!("right ISO SDU enqueue failed at frame {next_frame}"));
                    }
                    next_frame+=1;
                }

                self.poll_iso_transport()?;
                self.require_iso_links(handles,acls)?;
                let tx=self.iso_tx_snapshot()?;
                tx.check()?;
                let completed=tx.completed[0]+tx.completed[1];
                if completed>last_completed {
                    last_completed=completed;
                    last_progress=Instant::now();
                }
                let per_ear=tx.completed[0].min(tx.completed[1]);
                while per_ear>=next_report {
                    println!(
                        "[R5.3-C/DRAIN] completed={:?}/{:?} submitted={:?} host+controller_pending={} queued_pairs={}/{} elapsed={}ms",
                        tx.completed,[frames.len() as u64;2],tx.submitted,
                        self.device.iso_packets_pending().unwrap_or(usize::MAX),next_frame,frames.len(),start.elapsed().as_millis()
                    );
                    next_report+=100;
                }
                let pending=self.device.iso_packets_pending().ok_or("ISO queue disappeared during drain")?;
                if next_frame==frames.len() && pending==0 { break; }
                if last_progress.elapsed()>=PROGRESS_TIMEOUT {
                    return Err(format!(
                        "ISO controller queue made no completion progress for {}ms: queued_pairs={next_frame}/{} submitted={:?} completed={:?} pending={pending}",
                        PROGRESS_TIMEOUT.as_millis(),frames.len(),tx.submitted,tx.completed
                    ));
                }
                self.wait_iso_activity(POLL_SLICE)?;
            }

            self.require_iso_links(handles,acls)?;
            let tx=self.iso_tx_snapshot()?;
            tx.verify_complete(frames.len() as u64)?;
            let elapsed_ms=start.elapsed().as_millis() as u64;
            println!(
                "[R5.3-C/DONE] bounded queued transport PASS: pairs={} elapsed={}ms peak_hci_pending={}",
                frames.len(),elapsed_ms,tx.peak_pending
            );
            // Pinned Bumble-RS has an exact parser bug here: Core defines the
            // LE Read ISO TX Sync Time_Offset field as 24 bits, but this revision
            // reads u32. BE200 returns the valid 12-byte payload, so the parser
            // fails at offset 9 with only three bytes remaining. TX Sync is
            // optional evidence and must not poison a valid stream/cleanup.
            println!("[R5.3/TX-SYNC] skipped: pinned Bumble-RS parses 24-bit Time_Offset as u32");
            Ok(IsoStreamReport {
                frame_pairs:frames.len() as u64,
                elapsed_ms,
                max_lateness_us:0,
                tx,
                tx_sync:[None,None],
            })
        })();
        if let Ok(mut audit)=self.iso_audit.lock() { audit.stop(); }
        if let Err(ref e)=result {
            eprintln!("[R5.3-C/FAIL] {e}; packet audit={:?}",self.iso_tx_snapshot());
        }
        result
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use bumble_hci::{Command,CodingFormat};
    #[test]
    fn setup_and_remove_use_different_direction_encodings() {
        let parameters=IsoDataPathParameters::hci(INPUT);
        assert_eq!(parameters.codec_id,CodingFormat::TRANSPARENT);
        let setup=Command::LeSetupIsoDataPath { connection_handle:0x900,data_path_direction:INPUT,
            data_path_id:0,codec_id:parameters.codec_id,controller_delay:0,codec_configuration:vec![] }.to_bytes();
        let remove=Command::LeRemoveIsoDataPath { connection_handle:0x900,data_path_direction:REMOVE_INPUT_MASK }.to_bytes();
        assert_eq!(&setup[..8], &[1,0x6e,0x20,13,0,9,0,0]);
        assert_eq!(remove,vec![1,0x6f,0x20,3,0,9,1]);
    }
}

#[cfg(test)]
mod scripted_path_tests {
    use super::*;
    use std::sync::{mpsc, Arc, Mutex};
    use bumble_hci::{AclDataPacket, Command, Event, HciPacket, IsoDataPacket, LeMetaEvent, ReturnParameters};
    use bumble_host::{Device, HostTransport};
    use bumble_transport::{ExternalControllerInfo, ExternalHost, PacketSink, PacketSource,
        Result as TransportResult, SplitOpenedTransport};

    #[derive(Default)]
    struct SeedLink { events: Vec<HciPacket> }
    impl HostTransport for SeedLink {
        fn handle_command(&mut self,_:usize,_:Command) {}
        fn send_acl_packet(&mut self,_:usize,_:AclDataPacket)->bool { false }
        fn send_synchronous_data(&mut self,_:usize,_:u16,_:u8,_:&[u8])->bool { false }
        fn send_iso_packet(&mut self,_:usize,_:IsoDataPacket)->bool { false }
        fn drain_host_events(&mut self,_:usize)->Vec<HciPacket> { std::mem::take(&mut self.events) }
    }
    struct Source(mpsc::Receiver<HciPacket>);
    impl PacketSource for Source {
        fn read_packet(&mut self)->TransportResult<Option<HciPacket>> { Ok(self.0.recv().ok()) }
    }
    struct Sink { sender:mpsc::Sender<HciPacket>, setup_status:u8 }
    impl PacketSink for Sink {
        fn write_packet(&mut self,packet:&HciPacket)->TransportResult<()> {
            let HciPacket::Command(command)=packet else { panic!("path-only probe emitted data"); };
            let (handle,status)=match command {
                Command::LeSetupIsoDataPath {connection_handle,data_path_direction,data_path_id,codec_id,controller_delay,codec_configuration} => {
                    assert_eq!(*data_path_direction,0); assert_eq!(*data_path_id,0);
                    assert_eq!(*codec_id,bumble_hci::CodingFormat::TRANSPARENT);
                    assert_eq!(*controller_delay,0); assert!(codec_configuration.is_empty());
                    (*connection_handle,self.setup_status)
                }
                Command::LeRemoveIsoDataPath {connection_handle,data_path_direction} => {
                    assert_eq!(*data_path_direction,1); (*connection_handle,0)
                }
                other=>panic!("unexpected command in path-only test: {other:?}"),
            };
            self.sender.send(HciPacket::Event(Event::CommandComplete {
                num_hci_command_packets:1,command_opcode:command.op_code(),
                return_parameters:ReturnParameters::StatusAndConnectionHandle {status,connection_handle:handle},
            })).unwrap();
            Ok(())
        }
    }
    fn attached(setup_status:u8)->AttachedController {
        let mut device=Device::new(0);
        let mut seed=SeedLink::default();
        for handle in [0x900,0x901] {
            seed.events.push(HciPacket::Event(Event::LeMeta(LeMetaEvent::CisEstablished {
                status:0,connection_handle:handle,cig_sync_delay:1,cis_sync_delay:2,
                transport_latency_c_to_p:3,transport_latency_p_to_c:0,phy_c_to_p:2,phy_p_to_c:1,
                nse:2,bn_c_to_p:1,bn_p_to_c:0,ft_c_to_p:2,ft_p_to_c:0,
                max_pdu_c_to_p:100,max_pdu_p_to_c:0,iso_interval:8,
            })));
        }
        device.poll(&mut seed); let _ = device.take_cis_control_events();
        let (tx,rx)=mpsc::channel();
        let iso_audit=Arc::new(Mutex::new(crate::iso_audit::IsoAudit::default()));
        let transport=SplitOpenedTransport {source:Box::new(Source(rx)),
            sink:Box::new(Sink {sender:tx,setup_status}),metadata:Default::default()};
        let host=ExternalHost::new(crate::iso_audit::wrap_transport(transport,iso_audit.clone()));
        let info=ExternalControllerInfo {
            supported_commands:[0;64],local_version:None,local_le_features:None,local_le_features_max_page:None,
            local_lmp_features:vec![],acl_data_packet_length:0,total_num_acl_data_packets:0,
            le_acl_data_packet_length:0,total_num_le_acl_data_packets:0,
            iso_data_packet_length:248,total_num_iso_data_packets:8,suggested_default_data_length:None,
            number_of_supported_advertising_sets:0,maximum_advertising_data_length:0,
        };
        AttachedController {host,device,info,iso_audit}
    }
    #[test]
    fn two_input_paths_setup_and_remove_against_scripted_hci() {
        let mut c=attached(0);
        c.setup_iso_input_path(0x900).unwrap(); c.setup_iso_input_path(0x901).unwrap();
        assert!(c.device.iso_data_path(0x900,0).is_some());
        assert!(c.device.iso_data_path(0x901,0).is_some());
        c.remove_iso_input_path(0x901).unwrap(); c.remove_iso_input_path(0x900).unwrap();
        assert!(c.device.iso_data_path(0x901,0).is_none());
        assert!(c.device.iso_data_path(0x900,0).is_none());
    }
    #[test]
    fn setup_rejection_is_not_a_ready_path_and_is_not_an_uncertain_write() {
        let mut c=attached(0x0C);
        let failure=c.setup_iso_input_path(0x900).unwrap_err();
        assert!(!failure.may_have_configured);
        assert!(failure.message.contains("0x0C"));
        assert!(c.device.iso_data_path(0x900,0).is_none());
    }
}
