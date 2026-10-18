//! Transitional logical identities not yet assigned their final V2 owner.
//!
//! AudioSide moved to L1-Control/interface/stream in Round 5.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StreamId(pub u64);
