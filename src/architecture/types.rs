//! Transitional import path during Round 6 architecture retirement.
//!
//! The identity types are owned by Core/Lifecycle. This file defines no new
//! identity and is removed once all consumers use the Core path.

pub use crate::core::lifecycle::{SessionId, StreamId};
