//! R5.2-B: externally controlled dual CIS establishment and teardown.
//!
//! The pinned Bumble-RS HCI command and Device::cis_control_events() parser
//! are authoritative. This adapter only bridges ExternalHost to the real
//! BE200 controller; it never reads pairing material, configures ISO Data Paths
//! or sends ISO payloads.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use bumble_hci::Command;
use bumble_host::{CisControlEvent, CisLinkInfo};
use bumble_transport::{CommandResponse, ExternalHostActivity};

use crate::{AttachedController, CigAllocation};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const EVENT_SLICE: Duration = Duration::from_millis(250);
// HCI Disconnect *command* Reason must be a remote-facing error code
// permitted by Core Vol 4, Part E, 7.1.6. 0x16 is reserved here for the
// *local Disconnection Complete event*, not the command parameter.
// Our first BE200 R5.2-B HIL sent 0x16 and both Disconnect Command Status
// events returned 0x12 (Invalid HCI Command Parameters).
const DISCONNECT_REASON_REMOTE_USER_TERMINATED: u8 = 0x13;

fn cis_disconnect_command(connection_handle: u16) -> Command {
    Command::Disconnect {
        connection_handle,
        reason: DISCONNECT_REASON_REMOTE_USER_TERMINATED,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EstablishedCis {
    pub cis_id: u8,
    pub acl_handle: u16,
    pub link: CisLinkInfo,
}

#[derive(Clone, Debug)]
struct CisEventGate {
    expected: BTreeMap<u16, (u8, u16)>,
    established: BTreeMap<u16, EstablishedCis>,
}

impl CisEventGate {
    fn new(allocation: &CigAllocation, mappings: &[(u8, u16)]) -> Result<Self, String> {
        if allocation.cis.len() != 2 || mappings.len() != 2 {
            return Err("R5.2-B requires exactly two allocated CIS and two ACL mappings".into());
        }
        let mut expected = BTreeMap::new();
        let mut cis_ids = BTreeSet::new();
        let mut acl_handles = BTreeSet::new();
        for entry in &allocation.cis {
            let acl = mappings.iter().find(|(id, _)| *id == entry.cis_id)
                .ok_or_else(|| format!("CIS {} has no ACL member mapping", entry.cis_id))?.1;
            if !cis_ids.insert(entry.cis_id) || !acl_handles.insert(acl) || acl == entry.connection_handle
                || entry.connection_handle == 0
                || expected.insert(entry.connection_handle, (entry.cis_id, acl)).is_some()
            {
                return Err("CIS IDs, allocated handles and ACL mappings must be distinct".into());
            }
        }
        Ok(Self { expected, established: BTreeMap::new() })
    }

    fn command_vectors(&self) -> (Vec<u16>, Vec<u16>) {
        let cis = self.expected.keys().copied().collect();
        let acl = self.expected.values().map(|(_, acl)| *acl).collect();
        (cis, acl)
    }

    fn accept(&mut self, event: CisControlEvent) -> Result<(), String> {
        match event {
            CisControlEvent::Established { status, link } => {
                let (cis_id, acl) = *self.expected.get(&link.connection_handle)
                    .ok_or_else(|| format!(
                        "unsolicited CIS Established for unknown handle=0x{:04X}",
                        link.connection_handle
                    ))?;
                if status != 0 {
                    return Err(format!(
                        "CIS ID={cis_id} handle=0x{:04X} establishment FAILED: HCI status=0x{status:02X}",
                        link.connection_handle
                    ));
                }
                let handle = link.connection_handle;
                if self.established.contains_key(&handle) {
                    return Err(format!("duplicate LE CIS Established for handle=0x{handle:04X}"));
                }
                self.established.insert(handle, EstablishedCis {
                    cis_id, acl_handle: acl, link,
                });
                println!(
                    "[R5.2/CIS] CIS={} ACL=0x{:04X} handle=0x{:04X} Established event PASS",
                    cis_id, acl, handle
                );
                Ok(())
            }
            other => Err(format!("unexpected stale CIS control event: {other:?}")),
        }
    }

    fn complete(&self) -> bool {
        self.expected.len() == self.established.len()
    }

    fn into_ordered(self, allocation: &CigAllocation) -> Vec<EstablishedCis> {
        allocation.cis.iter()
            .filter_map(|entry| self.established.get(&entry.connection_handle).cloned())
            .collect()
    }
}

impl AttachedController {
    /// Submit a single HCI LE Create CIS command and wait for BOTH async
    /// LE CIS Established events. HCI Command Status is not establishment
    /// evidence. On error, the caller must still clean up any partial CIS.
    pub fn establish_dual_cis(
        &mut self,
        allocation: &CigAllocation,
        mappings: &[(u8, u16)],
        timeout: Duration,
    ) -> Result<Vec<EstablishedCis>, String> {
        if timeout.is_zero() {
            return Err("dual CIS timeout must be nonzero".into());
        }
        let mut gate = CisEventGate::new(allocation, mappings)?;
        for (_, acl) in gate.expected.values() {
            let state = self.le_acl_state(*acl);
            if !state.connected || !state.encrypted {
                return Err(format!(
                    "cannot Create CIS: ACL=0x{acl:04X} is not connected + encrypted"
                ));
            }
        }
        self.device.poll(&mut self.host);
        let stale = self.device.take_cis_control_events();
        if !stale.is_empty() {
            return Err(format!(
                "unexpected {} pending CIS control event(s) before Create CIS: {stale:?}",
                stale.len()
            ));
        }

        let (cis_connection_handle, acl_connection_handle) = gate.command_vectors();
        println!(
            "[R5.2/CIS] sending HCI LE Create CIS for {} CIS/ACL mapping(s): {:?} -> {:?}",
            cis_connection_handle.len(), cis_connection_handle, acl_connection_handle
        );
        let response = self.host.send_command(
            Command::LeCreateCis { cis_connection_handle, acl_connection_handle },
            COMMAND_TIMEOUT,
        ).map_err(|e| format!("HCI LE Create CIS command failed/timed out (may have started): {e}"))?;
        match response {
            CommandResponse::Status { status: 0, .. } => {}
            CommandResponse::Status { status, .. } => {
                return Err(format!("HCI LE Create CIS rejected: Command Status=0x{status:02X}"));
            }
            other => return Err(format!(
                "HCI LE Create CIS unexpectedly returned Command Complete rather than Command Status: {other:?}"
            )),
        }
        println!("[R5.2/CIS] LE Create CIS Command Status: SUCCESS (waiting for async events)");
        let deadline = Instant::now() + timeout;
        while !gate.complete() {
            self.device.poll(&mut self.host);
            for event in self.device.take_cis_control_events() {
                gate.accept(event)?;
            }
            if gate.complete() { break; }
            for (_, acl) in gate.expected.values() {
                if !self.device.is_connected_on_handle(*acl) {
                    return Err(format!(
                        "ACL=0x{acl:04X} disconnected while establishing both CIS"
                    ));
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "timed out waiting for two LE CIS Established events: got {}/2",
                    gate.established.len()
                ));
            }
            match self.host.wait_for_device_activity(&mut self.device, remaining.min(EVENT_SLICE))
                .map_err(|e| format!("HCI activity wait during CIS establish failed: {e}"))?
            {
                ExternalHostActivity::Packet | ExternalHostActivity::Timeout => {}
                ExternalHostActivity::Ended => return Err("controller transport ended during CIS establishment".into()),
            }
        }
        for handle in gate.established.keys() {
            if self.device.cis_link(*handle).is_none() {
                return Err(format!(
                    "CIS=0x{handle:04X} has Established event, but no active Device CIS link"
                ));
            }
        }
        let links = gate.into_ordered(allocation);
        for entry in &links {
            let link = &entry.link;
            println!(
                "[R5.2/CIS] CIS={} handle=0x{:04X}: PHY={}/{} latency_CtoP={}us NSE={} BN={} FT={} MaxPDU={} interval_units={}",
                entry.cis_id, link.connection_handle, link.phy_c_to_p,
                link.phy_p_to_c, link.transport_latency_c_to_p,
                link.nse, link.bn_c_to_p, link.ft_c_to_p,
                link.max_pdu_c_to_p, link.iso_interval
            );
        }
        Ok(links)
    }

    /// Disconnect all observed established CIS handles from the allocation,
    /// in reverse order, and wait for the actual Disconnection Complete to
    /// remove each Device CIS link. A never-established CIS is not disconnected.
    pub fn disconnect_established_cis(
        &mut self,
        allocation: &CigAllocation,
        timeout: Duration,
    ) -> Result<(), String> {
        if timeout.is_zero() { return Err("CIS teardown timeout must be nonzero".into()); }
        let mut errors = Vec::new();
        for entry in allocation.cis.iter().rev() {
            self.device.poll(&mut self.host);
            let handle = entry.connection_handle;
            if self.device.cis_link(handle).is_none() {
                println!(
                    "[R5.2/CIS] CIS={} handle=0x{handle:04X}: already disconnected or never established",
                    entry.cis_id
                );
                continue;
            }
            let response = self.host.send_command(
                cis_disconnect_command(handle),
                COMMAND_TIMEOUT,
            );
            match response {
                Ok(CommandResponse::Status { status: 0, .. }) => {}
                Ok(other) => {
                    errors.push(format!(
                        "CIS={} Disconnect Command Status mismatch: {other:?}", entry.cis_id
                    ));
                    continue;
                }
                Err(error) => {
                    errors.push(format!(
                        "CIS={} HCI Disconnect command failed: {error}", entry.cis_id
                    ));
                    continue;
                }
            }
            let deadline = Instant::now() + timeout;
            while self.device.cis_link(handle).is_some() {
                self.device.poll(&mut self.host);
                if self.device.cis_link(handle).is_none() { break; }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    errors.push(format!(
                        "CIS={} handle=0x{handle:04X} Disconnection Complete timeout",
                        entry.cis_id
                    ));
                    break;
                }
                match self.host.wait_for_device_activity(&mut self.device, remaining.min(EVENT_SLICE)) {
                    Ok(ExternalHostActivity::Packet | ExternalHostActivity::Timeout) => {}
                    Ok(ExternalHostActivity::Ended) => {
                        errors.push("controller transport ended during CIS teardown".into());
                        break;
                    }
                    Err(error) => {
                        errors.push(format!("CIS teardown HCI event wait: {error}"));
                        break;
                    }
                }
            }
            if self.device.cis_link(handle).is_none() {
                println!(
                    "[R5.2/CIS] CIS={} handle=0x{handle:04X}: disconnected PASS",
                    entry.cis_id
                );
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bumble_host::CisLinkInfo;

    fn allocation() -> CigAllocation {
        CigAllocation {
            cig_id: 1,
            cis: vec![
                crate::CisHandleAllocation { cis_id: 1, connection_handle: 0x0900 },
                crate::CisHandleAllocation { cis_id: 2, connection_handle: 0x0901 },
            ],
        }
    }

    fn link(handle: u16) -> CisLinkInfo {
        CisLinkInfo {
            connection_handle: handle,
            cig_sync_delay: 40_000,
            cis_sync_delay: 20_000,
            transport_latency_c_to_p: 35_000,
            transport_latency_p_to_c: 0,
            phy_c_to_p: 2, phy_p_to_c: 1,
            nse: 2, bn_c_to_p: 1, bn_p_to_c: 0,
            ft_c_to_p: 2, ft_p_to_c: 0,
            max_pdu_c_to_p: 100, max_pdu_p_to_c: 0,
            iso_interval: 8,
        }
    }

    #[test]
    fn cis_disconnect_command_uses_remote_facing_reason_0x13_not_event_reason_0x16() {
        let bytes = cis_disconnect_command(0x0900).to_bytes();
        // H4 Command, HCI_Disconnect opcode 0x0406, 3 bytes: handle + reason.
        assert_eq!(bytes, [0x01, 0x06, 0x04, 0x03, 0x00, 0x09, 0x13]);
        assert_ne!(bytes[6], 0x16);
    }

    #[test]
    fn gate_maps_out_of_order_establishment_to_original_cis_ids() {
        let allocation = allocation();
        let mut gate = CisEventGate::new(&allocation, &[(1, 0x0801), (2, 0x0800)]).unwrap();
        let (cis, acl) = gate.command_vectors();
        assert_eq!(cis, vec![0x0900, 0x0901]);
        assert_eq!(acl, vec![0x0801, 0x0800]);
        gate.accept(CisControlEvent::Established { status: 0, link: link(0x0901) }).unwrap();
        assert!(!gate.complete());
        gate.accept(CisControlEvent::Established { status: 0, link: link(0x0900) }).unwrap();
        assert!(gate.complete());
        assert_eq!(gate.into_ordered(&allocation).iter().map(|entry| entry.cis_id).collect::<Vec<_>>(), vec![1,2]);
    }

    #[test]
    fn gate_rejects_bad_status_duplicate_and_unexpected_handle() {
        let allocation = allocation();
        let mappings = [(1,0x0801),(2,0x0800)];
        let mut gate = CisEventGate::new(&allocation, &mappings).unwrap();
        assert!(gate.accept(CisControlEvent::Established { status: 0x3E, link: link(0x0900) }).is_err());
        assert!(gate.accept(CisControlEvent::Established { status: 0, link: link(0x0999) }).is_err());
        gate.accept(CisControlEvent::Established { status: 0, link: link(0x0900) }).unwrap();
        assert!(gate.accept(CisControlEvent::Established { status: 0, link: link(0x0900) }).is_err());
    }

    #[test]
    fn gate_refuses_missing_and_duplicate_acl_mappings() {
        let allocation=allocation();
        assert!(CisEventGate::new(&allocation, &[(1,0x0800),(2,0x0800)]).is_err());
        assert!(CisEventGate::new(&allocation, &[(1,0x0800),(3,0x0801)]).is_err());
    }
}
