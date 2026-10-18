use crate::plane::l1_control::ascs::operation::{
    AseOperationMachine,
    AsePrepareMachine,
};

/// Temporary ASCS transaction progress owned by L1-Control.
///
/// Preparation and semantic control are separate proven local FSMs. They are
/// grouped here because they are mutually exclusive stages of the active
/// Control ASE workflow; confirmed ASE facts live in ConfirmedStatus.
pub(crate) struct AseControlOperation {
    pub(crate) prepare: Option<AsePrepareMachine>,
    pub(crate) control: Option<AseOperationMachine>,
}

impl AseControlOperation {
    pub(crate) const fn new() -> Self {
        Self {
            prepare: None,
            control: None,
        }
    }
}
