use std::collections::BTreeSet;

use crate::plane::l1_hci::iso::cig_cis::CisRef;

/// Conservative cleanup knowledge used after uncertain controller outcomes.
///
/// These are intentionally not confirmed happy-path facts. They mean only that
/// cleanup must assume a resource may still exist until real evidence clears it.
pub(crate) struct CleanupKnowledge {
    pub(crate) cig_may_exist: bool,
    pub(crate) iso_paths_may_exist: BTreeSet<CisRef>,
}

impl CleanupKnowledge {
    pub(crate) fn new() -> Self {
        Self {
            cig_may_exist: false,
            iso_paths_may_exist: BTreeSet::new(),
        }
    }
}
