use crate::core::lifecycle::LifecycleSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::core::condition) struct LifecycleBlackboard {
    value: LifecycleSnapshot,
}

impl LifecycleBlackboard {
    pub(in crate::core::condition) const fn new(value: LifecycleSnapshot) -> Self { Self { value } }
    pub(in crate::core::condition) const fn value(self) -> LifecycleSnapshot { self.value }
    pub(in crate::core::condition) fn replace(&mut self, value: LifecycleSnapshot) -> bool {
        if self.value == value { return false; }
        self.value = value;
        true
    }
}
