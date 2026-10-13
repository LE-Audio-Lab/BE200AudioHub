//! R5.1 read-only snapshot of the BE200's *reported* ISO capabilities.
//!
//! This only interprets the ExternalControllerInfo cached at operational
//! attach. It never sends a controller command or configures a CIG/CIS.

use bumble_hci::metadata::supported_command_names;

use crate::AttachedController;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsoControllerCapabilities {
    pub cis_central: bool,
    pub le_2m_phy: bool,
    pub set_cig_parameters: bool,
    pub create_cis: bool,
    pub remove_cig: bool,
    pub setup_iso_data_path: bool,
    pub iso_data_packet_length: u16,
    pub iso_packet_count: u8,
}

impl IsoControllerCapabilities {
    /// Capability gate for the first-sound, one-HCI-ISO-packet-per-SDU path.
    /// An ISO first/complete packet (without HCI timestamp) has a 4-byte SDU
    /// information header inside its HCI ISO_Data field; see pinned Bumble-RS
    /// IsoDataPacket::to_bytes(). Fragmentation is not implemented yet.
    pub fn validate_for_unfragmented_cis(&self, max_sdu: u16) -> Result<(), String> {
        if !self.cis_central {
            return Err("controller does not advertise LE CIS Central".into());
        }
        if !self.set_cig_parameters || !self.create_cis || !self.remove_cig {
            return Err(format!(
                "controller lacks required CIG/CIS HCI command(s): SetCig={} CreateCis={} RemoveCig={}",
                self.set_cig_parameters, self.create_cis, self.remove_cig
            ));
        }
        if !self.setup_iso_data_path {
            return Err("controller does not advertise LE Setup ISO Data Path".into());
        }
        if self.iso_packet_count == 0 || self.iso_data_packet_length == 0 {
            return Err("controller reports no HCI ISO data buffers".into());
        }
        const FIRST_PACKET_SDU_INFO_BYTES: u16 = 4;
        let needed = max_sdu.saturating_add(FIRST_PACKET_SDU_INFO_BYTES);
        if needed > self.iso_data_packet_length {
            return Err(format!(
                "R5 first-sound unfragmented SDU {}B needs {}B ISO_Data capacity (4B SDU header); controller reports {}B; fragmentation is not implemented",
                max_sdu, needed, self.iso_data_packet_length
            ));
        }
        Ok(())
    }
}

impl AttachedController {
    pub fn iso_controller_capabilities(&self) -> IsoControllerCapabilities {
        let names = supported_command_names(&self.info.supported_commands);
        let supports = |name| names.contains(&name);
        let features = self.info.local_le_features.as_deref().unwrap_or(&[]);
        let feature = |bit: usize| -> bool {
            features.get(bit / 8).is_some_and(|value| value & (1 << (bit % 8)) != 0)
        };
        IsoControllerCapabilities {
            cis_central: feature(28),
            le_2m_phy: feature(8),
            set_cig_parameters: supports("HCI_LE_SET_CIG_PARAMETERS_COMMAND"),
            create_cis: supports("HCI_LE_CREATE_CIS_COMMAND"),
            remove_cig: supports("HCI_LE_REMOVE_CIG_COMMAND"),
            setup_iso_data_path: supports("HCI_LE_SETUP_ISO_DATA_PATH_COMMAND"),
            iso_data_packet_length: self.info.iso_data_packet_length,
            iso_packet_count: self.info.total_num_iso_data_packets,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> IsoControllerCapabilities {
        IsoControllerCapabilities {
            cis_central: true,
            le_2m_phy: true,
            set_cig_parameters: true,
            create_cis: true,
            remove_cig: true,
            setup_iso_data_path: true,
            iso_data_packet_length: 248,
            iso_packet_count: 8,
        }
    }

    #[test]
    fn accepts_100_byte_first_sound_sdu() {
        baseline().validate_for_unfragmented_cis(100).unwrap();
    }

    #[test]
    fn rejects_missing_iso_command_or_buffer() {
        let mut capabilities = baseline();
        capabilities.create_cis = false;
        assert!(capabilities.validate_for_unfragmented_cis(100).is_err());
        capabilities = baseline();
        capabilities.iso_packet_count = 0;
        assert!(capabilities.validate_for_unfragmented_cis(100).is_err());
    }

    #[test]
    fn refuses_unimplemented_fragmentation() {
        assert!(baseline().validate_for_unfragmented_cis(249).is_err());
        let mut cap = baseline();
        cap.iso_data_packet_length = 102;
        assert!(cap.validate_for_unfragmented_cis(100).is_err());
    }
}
