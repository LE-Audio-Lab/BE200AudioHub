//! Product BAP Unicast Client coordinator.
//!
//! Protocol primitives come from Bumble-RS. This crate owns orchestration only.

use device_policy::StreamPlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Discovering,
    CodecConfigured,
    QosConfigured,
    CisEstablished,
    DataPathReady,
    Enabling,
    Streaming,
    Releasing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub state: SessionState,
    pub plan: Option<StreamPlan>,
}

/// Hardware actions are deliberately not wired in this bootstrap commit.
/// See docs/R2_PSEUDOCODE.md and docs/UPSTREAM_REUSE_MAP.md.
pub struct BapCoordinator {
    snapshot: SessionSnapshot,
}

impl Default for BapCoordinator {
    fn default() -> Self {
        Self {
            snapshot: SessionSnapshot {
                state: SessionState::Idle,
                plan: None,
            },
        }
    }
}

impl BapCoordinator {
    pub fn snapshot(&self) -> &SessionSnapshot {
        &self.snapshot
    }
}
