use crate::plane::l1_control::interface::condition::ControlConditionUpdate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ControlBlackboard { value: ControlConditionUpdate }
impl ControlBlackboard {
    pub(super) const fn initial() -> Self { Self { value: ControlConditionUpdate::initial() } }
    pub(super) const fn value(self) -> ControlConditionUpdate { self.value }
    pub(super) fn replace(&mut self, value: ControlConditionUpdate) -> bool {
        if self.value == value { return false; }
        self.value = value; true
    }
    pub(super) fn reset(&mut self) -> bool { self.replace(ControlConditionUpdate::initial()) }
}
