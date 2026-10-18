//! Coarse whole-runtime and session lifecycle ownership.

pub mod identity;
pub mod machine;
pub mod model;
pub mod recovery;

pub use identity::{SessionId, StreamId};
pub use model::{LifecycleModel, LifecycleSnapshot, LifecycleState, SessionState};
pub use recovery::RecoveryClass;
