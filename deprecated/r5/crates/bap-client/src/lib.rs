//! Product BAP Unicast Client coordinator.
//!
//! Protocol primitives come from Bumble-RS. This crate owns orchestration and
//! normalized product snapshots only.

mod codec_plan;
mod codec_config;
mod cis_session;
mod qos_plan;
mod discovery;
mod ase_read;
mod snapshot;

pub use cis_session::{establish_dual_cis_and_release, run_dual_iso_probe_and_release, DualCisProbeSnapshot, EnabledMember, IsoProbeSnapshot};
pub use codec_config::{
    configure_and_release, configure_qos_and_release,
    CodecConfigScope, CodecConfiguredMember, CodecConfiguredSetSnapshot,
    QosConfiguredMember, QosConfiguredSetSnapshot,
};
pub use codec_plan::{
    plan_first_sound, plan_stereo_lc3, CodecConfigurationPlan, PlannedSink, StereoSide,
};
pub use qos_plan::{
    plan_first_sound_qos, plan_live_codec_configured_qos, PlannedQosMember, StereoQosPlan,
    FIRST_SOUND_CIG_ID, LEFT_CIS_ID, RIGHT_CIS_ID,
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
