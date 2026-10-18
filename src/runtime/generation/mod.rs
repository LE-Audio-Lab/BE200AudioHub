use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use crate::architecture::types::Generation;

#[derive(Clone, Debug)]
pub struct GenerationGate {
    current: Arc<AtomicU64>,
}

impl GenerationGate {
    pub fn new(initial: Generation) -> Self {
        Self {
            current: Arc::new(AtomicU64::new(initial.0)),
        }
    }

    pub fn current(&self) -> Generation {
        Generation(self.current.load(Ordering::Acquire))
    }

    pub fn accepts(&self, generation: Generation) -> bool {
        self.current().0 == generation.0
    }

    pub fn advance(&self) -> Generation {
        Generation(self.current.fetch_add(1, Ordering::AcqRel) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advancing_invalidates_old_generation() {
        let gate = GenerationGate::new(Generation(7));
        assert!(gate.accepts(Generation(7)));
        assert_eq!(gate.advance(), Generation(8));
        assert!(!gate.accepts(Generation(7)));
        assert!(gate.accepts(Generation(8)));
    }

    #[test]
    fn clones_share_the_same_generation() {
        let gate = GenerationGate::new(Generation(11));
        let clone = gate.clone();
        gate.advance();
        assert_eq!(clone.current(), Generation(12));
    }
}
