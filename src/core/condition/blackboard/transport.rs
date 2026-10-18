use crate::plane::l2_transport::interface::condition::TransportConditionUpdate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::core::condition) struct TransportBlackboard { value: TransportConditionUpdate }
impl TransportBlackboard {
    pub(in crate::core::condition) const fn initial() -> Self { Self { value: TransportConditionUpdate::initial() } }
    pub(in crate::core::condition) const fn value(self) -> TransportConditionUpdate { self.value }
    pub(in crate::core::condition) fn replace(&mut self, value: TransportConditionUpdate) -> bool {
        if self.value == value { return false; }
        self.value = value; true
    }
    pub(in crate::core::condition) fn reset(&mut self) -> bool { self.replace(TransportConditionUpdate::initial()) }
}
