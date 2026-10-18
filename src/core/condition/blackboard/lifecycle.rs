use crate::core::lifecycle::LifecycleSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LifecycleBlackboard {
    value: LifecycleSnapshot,
}

impl LifecycleBlackboard {
    pub(super) const fn new(value: LifecycleSnapshot) -> Self { Self { value } }
    pub(super) const fn value(self) -> LifecycleSnapshot { self.value }
    pub(super) fn replace(&mut self, value: LifecycleSnapshot) -> bool {
        if self.value == value { return false; }
        self.value = value;
        true
    }
}
