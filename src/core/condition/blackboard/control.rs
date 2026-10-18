use crate::plane::l1_control::interface::condition::ControlConditionUpdate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::core::condition) struct ControlBlackboard { value: ControlConditionUpdate }
impl ControlBlackboard {
    pub(in crate::core::condition) const fn initial() -> Self { Self { value: ControlConditionUpdate::initial() } }
    pub(in crate::core::condition) const fn value(self) -> ControlConditionUpdate { self.value }
    pub(in crate::core::condition) fn replace(&mut self, value: ControlConditionUpdate) -> bool {
        if self.value == value { return false; }
        self.value = value; true
    }
    pub(in crate::core::condition) fn reset(&mut self) -> bool { self.replace(ControlConditionUpdate::initial()) }
}
