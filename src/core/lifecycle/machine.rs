//! Coarse L0 lifecycle owner.
//!
//! Round 6 establishes ownership only. Product startup/recovery sequencing is
//! composed later and must not be guessed into this structural refactor.

use crate::runtime::generation::Generation;

use super::model::{LifecycleModel, LifecycleSnapshot};

pub struct LifecycleMachine {
    model: LifecycleModel,
}

impl LifecycleMachine {
    pub const fn new(generation: Generation) -> Self {
        Self { model: LifecycleModel::stopped(generation) }
    }

    pub const fn model(&self) -> LifecycleModel { self.model }
    pub const fn snapshot(&self) -> LifecycleSnapshot { self.model.snapshot() }

    pub fn replace_model(&mut self, model: LifecycleModel) {
        self.model = model;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lifecycle::{LifecycleState, SessionState};

    #[test]
    fn new_lifecycle_is_stopped_without_session() {
        let machine = LifecycleMachine::new(Generation(7));
        let snapshot = machine.snapshot();
        assert_eq!(snapshot.generation, Generation(7));
        assert_eq!(snapshot.session_id, None);
        assert_eq!(snapshot.lifecycle, LifecycleState::Stopped);
        assert_eq!(snapshot.session, SessionState::Idle);
    }
}
