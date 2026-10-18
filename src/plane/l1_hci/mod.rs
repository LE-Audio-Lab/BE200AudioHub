//! L1 HCI Host Plane.
//!
//! Round2 activates ACL/GATT ownership first; host and ISO boundaries are migrated in bounded steps.

pub mod acl;
pub mod gatt;
pub mod host;
pub mod iso;
