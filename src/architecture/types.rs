//! Shared architecture-level identifiers.
//! These identifiers describe logical ownership, not transport handles.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Generation(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StreamId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioSide {
    Left,
    Right,
}
