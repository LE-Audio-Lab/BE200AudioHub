use crate::plane::l1_control::{
    discovery::{
        audio::AudioDiscoveryMachine,
        snapshot::AudioMemberSnapshot,
    },
};

/// Temporary progress for discovering LE Audio services across the product set.
///
/// The completed dual-member AudioSetSnapshot is durable Control truth and is
/// promoted into ConfirmedStatus by the engine. This object owns only the
/// in-progress per-member discovery machine and partial member accumulation.
pub(crate) struct AudioDiscoveryOperation {
    pub(crate) machine: Option<AudioDiscoveryMachine>,
    pub(crate) members: Vec<AudioMemberSnapshot>,
}

impl AudioDiscoveryOperation {
    pub(crate) const fn new() -> Self {
        Self {
            machine: None,
            members: Vec::new(),
        }
    }
}
