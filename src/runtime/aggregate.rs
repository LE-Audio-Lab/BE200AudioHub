//! Core-side runtime observation aggregation.
//!
//! This module is deliberately separate from the Bluetooth/LE Audio
//! ControlPlane. Plane-local mutable state stays inside each owner. Producers
//! publish typed/coalesced snapshots; this single-writer aggregator reduces
//! them into the shared-readable RuntimeState world model.

use crate::architecture::{
    control::ControlSnapshot,
    hci_host::HciHostSnapshot,
    lifecycle::LifecycleSnapshot,
    media::MediaSnapshot,
    runtime_state::RuntimeState,
    transport::TransportSnapshot,
};

/// Coarse plane-level observations accepted by the RuntimeAggregator.
///
/// These are value projections only. They must never contain handles to mutable
/// subsystem implementation objects.
#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeObservation {
    Lifecycle(LifecycleSnapshot),
    Control(ControlSnapshot),
    Media(MediaSnapshot),
    Hci(HciHostSnapshot),
    Transport(TransportSnapshot),
}

/// Contract for producer-local telemetry aggregation.
///
/// High-rate raw events are accumulated inside the owning plane. Core sees only
/// the resulting snapshot; it must not receive one observation per packet/frame.
///
/// This trait intentionally does not define timing/window/reset policy. Each
/// plane owns that policy because packet counters, queue depths, media timing,
/// and latency histograms have different aggregation semantics.
pub trait PlaneTelemetryAccumulator<Event> {
    type Snapshot: Clone + PartialEq;

    fn observe(&mut self, event: &Event);
    fn snapshot(&self) -> Self::Snapshot;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeStateChange {
    pub changed: bool,
    pub epoch: u64,
}

/// Single writer of RuntimeState.
///
/// RuntimeAggregator is intended to live in CoreContext beside Lifecycle and
/// ControlPlane. It is not part of the Control semantic FSM.
pub struct RuntimeAggregator {
    state: RuntimeState,
}

impl RuntimeAggregator {
    pub fn new(state: RuntimeState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    /// Read-only value snapshot for CLI/UI/logging consumers.
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

    /// Reduce an already-coalesced group of plane snapshots.
    ///
    /// Every changed plane projection advances the observation epoch once.
    /// Callers use the returned changed bit to decide whether a new normal
    /// RuntimeState log/snapshot publication is useful.
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
    use crate::{
        architecture::{
        control::{ControlSnapshot, ControlState},
        hci_host::{HciHostSnapshot, HciHostState},
        lifecycle::{LifecycleSnapshot, LifecycleState, SessionState},
        media::{MediaSnapshot, MediaState},
        runtime_state::RuntimeState,
        transport::{TransportSnapshot, TransportState},
        },
        runtime::generation::Generation,
    };

    use super::*;

    fn initial() -> RuntimeState {
        RuntimeState::initial(Generation(7))
    }

    #[test]
    fn identical_observation_is_coalesced_without_epoch_churn() {
        let mut aggregate = RuntimeAggregator::new(initial());
        let snapshot = aggregate.state().transport;

        assert_eq!(
            aggregate.apply(RuntimeObservation::Transport(snapshot)),
            RuntimeStateChange {
                changed: false,
                epoch: 0,
            }
        );
    }

    #[test]
    fn plane_observations_update_only_their_projection() {
        let mut aggregate = RuntimeAggregator::new(initial());

        let hci = HciHostSnapshot {
            state: HciHostState::Ready,
            stream_id: None,
            controller_iso_capacity: 8,
            controller_iso_in_flight: 0,
        };
        assert!(aggregate.apply(RuntimeObservation::Hci(hci)).changed);
        assert_eq!(aggregate.state().epoch, 1);
        assert_eq!(aggregate.state().hci, hci);
        assert_eq!(aggregate.state().media.state, MediaState::Stopped);

        let media = MediaSnapshot {
            state: MediaState::Prepared,
            produced_frame: None,
            source_ring_fill_frames: 0,
            encoded_ring_fill_frames: 0,
            asrc_ratio: 1.0,
        };
        assert!(aggregate.apply(RuntimeObservation::Media(media)).changed);
        assert_eq!(aggregate.state().epoch, 2);
        assert_eq!(aggregate.state().media, media);
        assert_eq!(aggregate.state().hci, hci);
    }

    #[test]
    fn lifecycle_generation_is_mirrored_into_world_generation() {
        let mut aggregate = RuntimeAggregator::new(initial());
        let lifecycle = LifecycleSnapshot {
            generation: Generation(8),
            session_id: None,
            lifecycle: LifecycleState::Starting,
            session: SessionState::Preparing,
        };

        aggregate.apply(RuntimeObservation::Lifecycle(lifecycle));

        assert_eq!(aggregate.state().generation, Generation(8));
        assert_eq!(aggregate.state().lifecycle, lifecycle);
        assert_eq!(aggregate.state().epoch, 1);
    }

    #[test]
    fn control_snapshot_is_a_projection_not_plane_state_storage() {
        let mut aggregate = RuntimeAggregator::new(initial());
        let control = ControlSnapshot {
            state: ControlState::Resolving,
            stream_id: None,
            format: None,
        };

        aggregate.apply(RuntimeObservation::Control(control));

        assert_eq!(aggregate.state().control, control);
        assert_eq!(
            aggregate.state().transport,
            TransportSnapshot {
                state: TransportState::Detached,
                rx_packets: 0,
                tx_packets: 0,
                faults: 0,
            }
        );
    }

    #[derive(Default)]
    struct CounterAccumulator {
        count: u64,
    }

    impl PlaneTelemetryAccumulator<u32> for CounterAccumulator {
        type Snapshot = u64;

        fn observe(&mut self, _event: &u32) {
            self.count += 1;
        }

        fn snapshot(&self) -> Self::Snapshot {
            self.count
        }
    }

    #[test]
    fn plane_accumulator_contract_keeps_raw_events_local() {
        let mut accumulator = CounterAccumulator::default();
        for value in 0..100 {
            accumulator.observe(&value);
        }
        assert_eq!(accumulator.snapshot(), 100);
    }
}
