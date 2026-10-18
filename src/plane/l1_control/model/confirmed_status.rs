use std::collections::BTreeSet;

use bumble_profiles::ascs::AseStatus;

use crate::plane::{
    l1_control::{
        ascs::operation::PreparedAseControl,
        discovery::{
            peer::ResolvedMemberPresence,
            snapshot::AudioSetSnapshot,
        },
        interface::stream::StreamBinding,
    },
    l1_hci::{
        acl::connection::ConnectionToken,
        iso::cig_cis::CisRef,
    },
};

/// Durable facts that L1-Control has already confirmed.
///
/// This is operational Control truth, not a Core/Condition read model.
/// Temporary callback/transaction progress belongs to Control operations,
/// while selected-but-not-yet-confirmed configuration remains outside this
/// structure until its ownership is made explicit.
pub(crate) struct ConfirmedStatus {
    pub(crate) resolved_members: Vec<ResolvedMemberPresence>,
    pub(crate) connections: [Option<ConnectionToken>; 2],
    pub(crate) encryption_key_sizes: [Option<u8>; 2],
    pub(crate) privacy_active: bool,
    pub(crate) audio_set: Option<AudioSetSnapshot>,
    pub(crate) prepared_ases: [Option<PreparedAseControl>; 2],
    pub(crate) codec_statuses: [Option<AseStatus>; 2],
    pub(crate) qos_statuses: [Option<AseStatus>; 2],
    pub(crate) enable_statuses: [Option<AseStatus>; 2],
    pub(crate) live_cis: BTreeSet<CisRef>,
    pub(crate) iso_paths_ready: BTreeSet<CisRef>,
    pub(crate) streaming_statuses: [Option<AseStatus>; 2],
    pub(crate) active_stream_binding: Option<StreamBinding>,
}

impl ConfirmedStatus {
    pub(crate) fn new() -> Self {
        Self {
            resolved_members: Vec::new(),
            connections: [None, None],
            encryption_key_sizes: [None, None],
            privacy_active: false,
            audio_set: None,
            prepared_ases: [None, None],
            codec_statuses: [None, None],
            qos_statuses: [None, None],
            enable_statuses: [None, None],
            live_cis: BTreeSet::new(),
            iso_paths_ready: BTreeSet::new(),
            streaming_statuses: [None, None],
            active_stream_binding: None,
        }
    }
}
