//! R2 ownership adapter around the R1 hardware-proven BE200 substrate.
//!
//! This crate intentionally starts as policy/types only. The existing R1 files
//! remain canonical until the first Bumble attach experiment is validated.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerMode {
    Bootloader,
    Operational,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachAction {
    /// Attach to the already operational controller. No reset and no firmware load.
    AttachOperational,
    /// R1.5b firmware loader is required before host attach.
    RequireValidatedFirmwareBoot,
}

pub fn plan_attach(mode: ControllerMode) -> AttachAction {
    match mode {
        ControllerMode::Operational => AttachAction::AttachOperational,
        ControllerMode::Bootloader => AttachAction::RequireValidatedFirmwareBoot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_never_requests_firmware_boot() {
        assert_eq!(
            plan_attach(ControllerMode::Operational),
            AttachAction::AttachOperational
        );
    }
}
