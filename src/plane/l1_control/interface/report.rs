//! Reliable lower semantic reports accepted by L1-Control.

use crate::plane::l1_hci::interface::report::HciReport;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlLowerReport {
    Hci(HciReport),
}
