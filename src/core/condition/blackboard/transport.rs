use crate::plane::l2_transport::interface::condition::TransportConditionUpdate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TransportBlackboard { value: TransportConditionUpdate }
impl TransportBlackboard {
    pub(super) const fn initial() -> Self { Self { value: TransportConditionUpdate::initial() } }
    pub(super) const fn value(self) -> TransportConditionUpdate { self.value }
    pub(super) fn replace(&mut self, value: TransportConditionUpdate) -> bool {
        if self.value == value { return false; }
        self.value = value; true
    }
    pub(super) fn reset(&mut self) -> bool { self.replace(TransportConditionUpdate::initial()) }
}
