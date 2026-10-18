use crate::plane::l1_media::interface::condition::MediaConditionUpdate;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MediaBlackboard { value: MediaConditionUpdate }
impl MediaBlackboard {
    pub(super) const fn initial() -> Self { Self { value: MediaConditionUpdate::initial() } }
    pub(super) const fn value(self) -> MediaConditionUpdate { self.value }
    pub(super) fn replace(&mut self, value: MediaConditionUpdate) -> bool {
        if self.value == value { return false; }
        self.value = value; true
    }
    pub(super) fn reset(&mut self) -> bool { self.replace(MediaConditionUpdate::initial()) }
}
