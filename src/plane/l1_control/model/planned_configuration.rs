use crate::plane::l1_control::planning::{
    codec::StereoCodecPlan,
    enable::StereoEnablePlan,
    iso::StereoIsoPlan,
    qos::StereoQosPlan,
};

/// Configuration selected by L1-Control planners but not itself proof that
/// the corresponding controller/ASE state has been confirmed.
///
/// Confirmation evidence belongs in ConfirmedStatus.
pub(crate) struct PlannedConfiguration {
    pub(crate) codec_plan: Option<StereoCodecPlan>,
    pub(crate) iso_plan: Option<StereoIsoPlan>,
    pub(crate) qos_plan: Option<StereoQosPlan>,
    pub(crate) enable_plan: Option<StereoEnablePlan>,
}

impl PlannedConfiguration {
    pub(crate) const fn new() -> Self {
        Self {
            codec_plan: None,
            iso_plan: None,
            qos_plan: None,
            enable_plan: None,
        }
    }
}
