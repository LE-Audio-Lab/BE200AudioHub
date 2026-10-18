//! Single-writer RuntimeState aggregation.
//!
//! Plane-local mutable state remains inside its owner. Control/Core receives
//! typed observations and reduces them into a shared-readable world model.
//! Repeated identical snapshots are coalesced: they do not advance the public
//! RuntimeState epoch.

use crate::architecture::runtime_state::RuntimeState;

use super::boundary::RuntimeObservation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeStateChange {
    pub changed: bool,
    pub epoch: u64,
}

pub struct RuntimeStateReducer {
    state: RuntimeState,
}

impl RuntimeStateReducer {
    pub fn new(state: RuntimeState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    /// Clone the observation model for readers such as CLI/UI/telemetry.
    ///
    /// The clone is a value snapshot. Readers never receive mutable access to
    /// the reducer-owned RuntimeState.
    pub fn snapshot(&self) -> RuntimeState {
        self.state.clone()
    }

    pub fn apply(
        &mut self,
        observation: RuntimeObservation,
    ) -> RuntimeStateChange {
        let changed = match observation {
            RuntimeObservation::Lifecycle(snapshot) => {
                let changed = self.state.lifecycle != snapshot
                    || self.state.generation != snapshot.generation;
                if changed {
                    self.state.generation = snapshot.generation;
                    self.state.lifecycle = snapshot;
                }
                changed
            }
            RuntimeObservation::Control(snapshot) => {
                let changed = self.state.control != snapshot;
                if changed {
                    self.state.control = snapshot;
                }
                changed
            }
            RuntimeObservation::Media(snapshot) => {
                let changed = self.state.media != snapshot;
                if changed {
                    self.state.media = snapshot;
                }
                changed
            }
            RuntimeObservation::Hci(snapshot) => {
                let changed = self.state.hci != snapshot;
                if changed {
                    self.state.hci = snapshot;
                }
                changed
            }
            RuntimeObservation::Transport(snapshot) => {
                let changed = self.state.transport != snapshot;
                if changed {
                    self.state.transport = snapshot;
                }
                changed
            }
        };

        if changed {
            self.state.epoch = self.state.epoch.saturating_add(1);
        }

        RuntimeStateChange {
            changed,
            epoch: self.state.epoch,
        }
    }

    /// Reduce a coalesced batch and publish at most one externally meaningful
    /// epoch movement per changed observation. Callers can decide whether to
    /// emit a new read-only snapshot based on the returned changed flag.
    pub fn apply_all(
        &mut self,
        observations: impl IntoIterator<Item = RuntimeObservation>,
    ) -> RuntimeStateChange {
        let start_epoch = self.state.epoch;
        for observation in observations {
            self.apply(observation);
        }
        RuntimeStateChange {
            changed: self.state.epoch != start_epoch,
            epoch: self.state.epoch,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::architecture::{
        control::{ControlSnapshot, ControlState},
        hci_host::{HciHostSnapshot, HciHostState},
        lifecycle::{
            LifecycleSnapshot, LifecycleState, SessionState,
        },
        media::{MediaSnapshot, MediaState},
        runtime_state::RuntimeState,
        transport::{TransportSnapshot, TransportState},
        types::Generation,
    };

    use super::*;

    fn initial() -> RuntimeState {
        RuntimeState::initial(Generation(7))
    }

    #[test]
    fn identical_observation_is_coalesced_without_epoch_churn() {
        let mut reducer = RuntimeStateReducer::new(initial());
        let snapshot = reducer.state().transport;

        assert_eq!(
            reducer.apply(RuntimeObservation::Transport(snapshot)),
            RuntimeStateChange {
                changed: false,
                epoch: 0,
            }
        );
    }

    #[test]
    fn plane_observations_update_only_their_projection() {
        let mut reducer = RuntimeStateReducer::new(initial());

        let hci = HciHostSnapshot {
            state: HciHostState::Ready,
            stream_id: None,
            controller_iso_capacity: 8,
            controller_iso_in_flight: 0,
        };
        assert!(reducer.apply(RuntimeObservation::Hci(hci)).changed);
        assert_eq!(reducer.state().epoch, 1);
        assert_eq!(reducer.state().hci, hci);
        assert_eq!(reducer.state().media.state, MediaState::Stopped);

        let media = MediaSnapshot {
            state: MediaState::Prepared,
            produced_frame: None,
            source_ring_fill_frames: 0,
            encoded_ring_fill_frames: 0,
            asrc_ratio: 1.0,
        };
        assert!(reducer.apply(RuntimeObservation::Media(media)).changed);
        assert_eq!(reducer.state().epoch, 2);
        assert_eq!(reducer.state().media, media);
        assert_eq!(reducer.state().hci, hci);
    }

    #[test]
    fn lifecycle_generation_is_mirrored_into_world_generation() {
        let mut reducer = RuntimeStateReducer::new(initial());
        let lifecycle = LifecycleSnapshot {
            generation: Generation(8),
            session_id: None,
            lifecycle: LifecycleState::Starting,
            session: SessionState::Preparing,
        };

        reducer.apply(RuntimeObservation::Lifecycle(lifecycle));

        assert_eq!(reducer.state().generation, Generation(8));
        assert_eq!(reducer.state().lifecycle, lifecycle);
        assert_eq!(reducer.state().epoch, 1);
    }

    #[test]
    fn control_snapshot_is_a_projection_not_plane_state_storage() {
        let mut reducer = RuntimeStateReducer::new(initial());
        let control = ControlSnapshot {
            state: ControlState::Resolving,
            stream_id: None,
            format: None,
        };

        reducer.apply(RuntimeObservation::Control(control));

        assert_eq!(reducer.state().control, control);
        assert_eq!(
            reducer.state().transport,
            TransportSnapshot {
                state: TransportState::Detached,
                rx_packets: 0,
                tx_packets: 0,
                faults: 0,
            }
        );
    }
}
