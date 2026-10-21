use crate::{
    core::{
        condition::{
            blackboard::Blackboard,
            interface::{ConditionChange, ConditionScope},
        },
        lifecycle::LifecycleSnapshot,
    },
    plane::{
        l1_control::interface::condition::ControlConditionUpdate,
        l1_hci::interface::condition::HciConditionUpdate,
        l1_media::interface::condition::MediaConditionUpdate,
        l2_transport::interface::condition::TransportConditionUpdate,
    },
};

pub(super) struct ConditionReducer;

impl ConditionReducer {
    pub(super) fn lifecycle(bb: &mut Blackboard, update: LifecycleSnapshot) -> ConditionChange {
        if update.generation.0 < bb.generation.0 {
            return Self::stale(bb);
        }

        let old_session = bb.lifecycle.value().session_id;
        let new_generation = update.generation != bb.generation;
        let new_session = old_session != update.session_id;
        let mut changed = false;

        if new_generation {
            bb.generation = update.generation;
            bb.reset_planes_for_generation();
            changed = true;
        } else if new_session {
            bb.reset_session_projection();
            changed = true;
        }
        changed |= bb.lifecycle.replace(update);
        Self::finish(bb, changed)
    }

    pub(super) fn control(bb: &mut Blackboard, scope: ConditionScope, update: ControlConditionUpdate) -> ConditionChange {
        if !Self::accepts_scope(bb, scope) { return Self::stale(bb); }
        let changed = bb.control.replace(update);
        Self::finish(bb, changed)
    }
    pub(super) fn hci(bb: &mut Blackboard, scope: ConditionScope, update: HciConditionUpdate) -> ConditionChange {
        if !Self::accepts_scope(bb, scope) { return Self::stale(bb); }
        let changed = bb.hci.apply(update);
        Self::finish(bb, changed)
    }
    pub(super) fn media(bb: &mut Blackboard, scope: ConditionScope, update: MediaConditionUpdate) -> ConditionChange {
        if !Self::accepts_scope(bb, scope) { return Self::stale(bb); }
        let changed = bb.media.apply(update);
        Self::finish(bb, changed)
    }
    pub(super) fn transport(bb: &mut Blackboard, scope: ConditionScope, update: TransportConditionUpdate) -> ConditionChange {
        if !Self::accepts_scope(bb, scope) { return Self::stale(bb); }
        let changed = bb.transport.apply(update);
        Self::finish(bb, changed)
    }

    fn accepts_scope(bb: &Blackboard, scope: ConditionScope) -> bool {
        if scope.generation != bb.generation { return false; }
        match scope.session_id {
            Some(session_id) => bb.lifecycle.value().session_id == Some(session_id),
            None => true,
        }
    }
    fn stale(bb: &Blackboard) -> ConditionChange {
        ConditionChange { changed: false, stale: true, epoch: bb.epoch }
    }
    fn finish(bb: &mut Blackboard, changed: bool) -> ConditionChange {
        if changed { bb.epoch = bb.epoch.saturating_add(1); }
        ConditionChange { changed, stale: false, epoch: bb.epoch }
    }
}
