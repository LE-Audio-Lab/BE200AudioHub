//! Transitional import path during Round 6 architecture retirement.
//!
//! Lifecycle ownership is in Core/Lifecycle. This file defines no duplicate
//! state and is removed after Condition/Core consumers switch paths.

pub use crate::core::lifecycle::{
    LifecycleSnapshot, LifecycleState, RecoveryClass, SessionState,
};
