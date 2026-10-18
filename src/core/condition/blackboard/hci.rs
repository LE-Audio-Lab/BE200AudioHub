use crate::plane::l1_hci::interface::condition::HciConditionUpdate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::core::condition) struct HciBlackboard { value: HciConditionUpdate }
impl HciBlackboard {
    pub(in crate::core::condition) const fn initial() -> Self { Self { value: HciConditionUpdate::initial() } }
    pub(in crate::core::condition) const fn value(self) -> HciConditionUpdate { self.value }
    pub(in crate::core::condition) fn replace(&mut self, value: HciConditionUpdate) -> bool {
        if self.value == value { return false; }
        self.value = value; true
    }
    pub(in crate::core::condition) fn reset(&mut self) -> bool { self.replace(HciConditionUpdate::initial()) }
    pub(in crate::core::condition) fn clear_stream(&mut self) -> bool {
        if self.value.stream_id.is_none() { return false; }
        self.value.stream_id = None;
        true
    }
}
