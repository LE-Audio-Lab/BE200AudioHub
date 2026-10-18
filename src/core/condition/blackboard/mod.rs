mod control;
mod hci;
mod lifecycle;
mod media;
mod transport;

use crate::{
    core::lifecycle::LifecycleSnapshot,
    runtime::generation::Generation,
};
use control::ControlBlackboard;
use hci::HciBlackboard;
use lifecycle::LifecycleBlackboard;
use media::MediaBlackboard;
use transport::TransportBlackboard;

pub(super) struct Blackboard {
    pub(super) epoch: u64,
    pub(super) generation: Generation,
    pub(super) lifecycle: LifecycleBlackboard,
    pub(super) control: ControlBlackboard,
    pub(super) hci: HciBlackboard,
    pub(super) media: MediaBlackboard,
    pub(super) transport: TransportBlackboard,
}

impl Blackboard {
    pub(super) const fn new(lifecycle: LifecycleSnapshot) -> Self {
        Self {
            epoch: 0,
            generation: lifecycle.generation,
            lifecycle: LifecycleBlackboard::new(lifecycle),
            control: ControlBlackboard::initial(),
            hci: HciBlackboard::initial(),
            media: MediaBlackboard::initial(),
            transport: TransportBlackboard::initial(),
        }
    }

    pub(super) fn reset_planes_for_generation(&mut self) {
        self.control.reset();
        self.hci.reset();
        self.media.reset();
        self.transport.reset();
    }

    pub(super) fn reset_session_projection(&mut self) {
        self.control.reset();
        self.media.reset();
        self.hci.clear_stream();
    }
}
