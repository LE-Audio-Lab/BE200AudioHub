//! Product BAP Unicast Client coordinator.
//!
//! Protocol primitives come from Bumble-RS. This crate owns orchestration and
//! normalized product snapshots only.

mod codec_plan;
mod codec_config;
mod discovery;
mod snapshot;

pub use codec_config::{
    configure_and_release, CodecConfigScope, CodecConfiguredMember, CodecConfiguredSetSnapshot,
};
pub use codec_plan::{
    plan_first_sound, plan_stereo_lc3, CodecConfigurationPlan, PlannedSink, StereoSide,
};
pub use discovery::discover_audio_member;
pub use snapshot::{
    AseSnapshot, AscsSnapshot, AudioMemberSnapshot, AudioSetSnapshot, CapSnapshot,
    ConnectedAudioMember, GattServiceRange, PacsSnapshot,
};

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
