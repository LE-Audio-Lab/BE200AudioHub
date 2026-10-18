use crate::plane::l1_hci::interface::condition::HciConditionUpdate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HciBlackboard { value: HciConditionUpdate }
impl HciBlackboard {
    pub(super) const fn initial() -> Self { Self { value: HciConditionUpdate::initial() } }
    pub(super) const fn value(self) -> HciConditionUpdate { self.value }
    pub(super) fn replace(&mut self, value: HciConditionUpdate) -> bool {
        if self.value == value { return false; }
        self.value = value; true
    }
    pub(super) fn reset(&mut self) -> bool { self.replace(HciConditionUpdate::initial()) }
    pub(super) fn clear_stream(&mut self) -> bool {
        if self.value.stream_id.is_none() { return false; }
        self.value.stream_id = None;
        true
    }
}
