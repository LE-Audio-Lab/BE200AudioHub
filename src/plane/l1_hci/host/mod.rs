//! Single-owner HCI Host execution components.
//!
//! Round2 stages the host files against the new ACL/GATT owners. The host module
//! is activated together with ISO/interface migration in Round3 to avoid a
//! duplicate host/ISO type universe.

pub mod engine;
pub mod link;
pub mod stack;
pub mod worker;
