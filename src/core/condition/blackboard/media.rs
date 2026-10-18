use crate::plane::l1_media::interface::condition::MediaConditionUpdate;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::core::condition) struct MediaBlackboard { value: MediaConditionUpdate }
impl MediaBlackboard {
    pub(in crate::core::condition) const fn initial() -> Self { Self { value: MediaConditionUpdate::initial() } }
    pub(in crate::core::condition) const fn value(self) -> MediaConditionUpdate { self.value }
    pub(in crate::core::condition) fn replace(&mut self, value: MediaConditionUpdate) -> bool {
        if self.value == value { return false; }
        self.value = value; true
    }
    pub(in crate::core::condition) fn reset(&mut self) -> bool { self.replace(MediaConditionUpdate::initial()) }
}
