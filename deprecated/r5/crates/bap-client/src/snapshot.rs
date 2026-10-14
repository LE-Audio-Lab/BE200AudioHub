use bumble_profiles::ascs::AseStatus;
use bumble_profiles::bap::AudioLocation;
use bumble_profiles::pacs::{AudioContexts, PacRecord};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GattServiceRange {
    pub start_handle: u16,
    pub end_handle: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AseSnapshot {
    pub declaration_handle: u16,
    pub value_handle: u16,
    pub end_group_handle: u16,
    pub properties: u8,
    pub status: AseStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacsSnapshot {
    pub service: GattServiceRange,
    pub sink_pac_handle: Option<u16>,
    pub source_pac_handle: Option<u16>,
    pub sink_audio_location_handle: Option<u16>,
    pub source_audio_location_handle: Option<u16>,
    pub available_contexts_handle: u16,
    pub supported_contexts_handle: u16,
    pub sink_pac_records: Vec<PacRecord>,
    pub source_pac_records: Vec<PacRecord>,
    pub sink_audio_locations: Option<AudioLocation>,
    pub source_audio_locations: Option<AudioLocation>,
    pub available_contexts: AudioContexts,
    pub supported_contexts: AudioContexts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AscsSnapshot {
    pub service: GattServiceRange,
    pub sink_ases: Vec<AseSnapshot>,
    pub source_ases: Vec<AseSnapshot>,
    pub control_point_handle: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapSnapshot {
    pub service: GattServiceRange,
    pub included_csis: Option<GattServiceRange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedAudioMember {
    pub identity_address: String,
    pub rank: u8,
    pub current_address: String,
    pub current_address_type: u8,
    pub connection_handle: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioMemberSnapshot {
    pub identity_address: String,
    pub rank: u8,
    pub current_address: String,
    pub current_address_type: u8,
    pub connection_handle: u16,
    pub pacs: PacsSnapshot,
    pub ascs: AscsSnapshot,
    pub cap: Option<CapSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSetSnapshot {
    pub target_name: String,
    pub set_size: u8,
    pub members: Vec<AudioMemberSnapshot>,
}

impl AudioSetSnapshot {
    pub fn validate_sink_discovery(&self) -> Result<(), String> {
        if self.members.len() != usize::from(self.set_size) {
            return Err(format!(
                "audio snapshot has {} member(s), expected Set Size {}",
                self.members.len(), self.set_size
            ));
        }
        for member in &self.members {
            if member.pacs.sink_pac_records.is_empty() {
                return Err(format!(
                    "member {} exposes no Sink PAC record",
                    member.identity_address
                ));
            }
            if member.ascs.sink_ases.is_empty() {
                return Err(format!(
                    "member {} exposes no Sink ASE",
                    member.identity_address
                ));
            }
        }
        Ok(())
    }
}
