//! L0 product lifecycle identities.
//!
//! StreamId is owned by Core/L0 but this module does not define an allocation
//! policy. Existing callers continue to supply/mint it from orchestration.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StreamId(pub u64);
