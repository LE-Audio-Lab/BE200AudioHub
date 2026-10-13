//! Transport-agnostic LE pairing orchestration.
//!
//! This crate deliberately contains no HCI, SMP, GATT, Bumble, Intel, or
//! Samsung implementation types. It owns only the control-plane state machine
//! distilled from AOSP's CSIS/LE bonding flow and Zephyr's CSIP coordinator:
//! discover a candidate, serialize one LE bond, verify coordinated-set
//! membership, then commit or discard the provisional bond.

mod machine;
mod model;

pub use machine::{InvalidTransition, PairingAction, PairingEvent, PairingMachine, PairingStage, PairingState, Transition};
pub use model::{AddressKind, BondRequest, BondRole, ExpectedSetMember, MemberIdentity, PeerAddress, SetDescriptor, SetHandle};
