//! Runtime Plane -> Condition publication.
//!
//! State-changing facts share one bounded reliable inbox. Aggregate telemetry
//! uses one independent coalesced-latest slot per Plane so HCI, Media and
//! Transport can never overwrite each other's observations.

use crate::{
    core::{
        condition::{
            interface::{ConditionScope, ConditionChange},
            Condition,
        },
        lifecycle::{SessionId},
    },
    plane::{
        l1_hci::interface::condition::HciConditionUpdate,
        l1_media::interface::condition::MediaConditionUpdate,
        l2_transport::interface::condition::TransportConditionUpdate,
    },
    runtime::{
        generation::Generation,
        mailbox::{
            self,
            latest::{self, LatestReceiver, LatestSendError, LatestSender},
            BoundedReceiver, BoundedSender, RecvError, SendError,
        },
    },
};

pub const CONDITION_RELIABLE_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConditionPublishError {
    ReliableFull,
    Disconnected,
    WrongDeliveryClass,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConditionReconcileStats {
    pub reliable_applied: u64,
    pub telemetry_applied: u64,
    pub stale_rejected: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scoped<T> {
    scope: ConditionScope,
    update: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReliableConditionUpdate {
    Hci(Scoped<HciConditionUpdate>),
    Media(Scoped<MediaConditionUpdate>),
    Transport(Scoped<TransportConditionUpdate>),
}

pub struct HciConditionPublisher {
    scope: ConditionScope,
    reliable: BoundedSender<ReliableConditionUpdate>,
    telemetry: LatestSender<Scoped<HciConditionUpdate>>,
}

pub struct MediaConditionPublisher {
    scope: ConditionScope,
    reliable: BoundedSender<ReliableConditionUpdate>,
    telemetry: LatestSender<Scoped<MediaConditionUpdate>>,
}

pub struct TransportConditionPublisher {
    scope: ConditionScope,
    reliable: BoundedSender<ReliableConditionUpdate>,
    telemetry: LatestSender<Scoped<TransportConditionUpdate>>,
}

pub struct ConditionPublishers {
    pub hci: HciConditionPublisher,
    pub media: MediaConditionPublisher,
    pub transport: TransportConditionPublisher,
}

pub struct ConditionPublicationInbox {
    reliable: BoundedReceiver<ReliableConditionUpdate>,
    hci_telemetry: LatestReceiver<Scoped<HciConditionUpdate>>,
    media_telemetry: LatestReceiver<Scoped<MediaConditionUpdate>>,
    transport_telemetry:
        LatestReceiver<Scoped<TransportConditionUpdate>>,
}

pub fn runtime_channels(
    generation: Generation,
    session_id: SessionId,
) -> (ConditionPublishers, ConditionPublicationInbox) {
    let (reliable_tx, reliable_rx) =
        mailbox::bounded(CONDITION_RELIABLE_CAPACITY);
    let (hci_telemetry_tx, hci_telemetry_rx) = latest::latest();
    let (media_telemetry_tx, media_telemetry_rx) = latest::latest();
    let (transport_telemetry_tx, transport_telemetry_rx) =
        latest::latest();

    let session_scope = ConditionScope::session(generation, session_id);
    let runtime_scope = ConditionScope::runtime(generation);

    (
        ConditionPublishers {
            hci: HciConditionPublisher {
                scope: session_scope,
                reliable: reliable_tx.clone(),
                telemetry: hci_telemetry_tx,
            },
            media: MediaConditionPublisher {
                scope: session_scope,
                reliable: reliable_tx.clone(),
                telemetry: media_telemetry_tx,
            },
            transport: TransportConditionPublisher {
                scope: runtime_scope,
                reliable: reliable_tx,
                telemetry: transport_telemetry_tx,
            },
        },
        ConditionPublicationInbox {
            reliable: reliable_rx,
            hci_telemetry: hci_telemetry_rx,
            media_telemetry: media_telemetry_rx,
            transport_telemetry: transport_telemetry_rx,
        },
    )
}

fn map_reliable_error(error: SendError) -> ConditionPublishError {
    match error {
        SendError::Full => ConditionPublishError::ReliableFull,
        SendError::Disconnected => ConditionPublishError::Disconnected,
    }
}

fn map_latest_error(_: LatestSendError) -> ConditionPublishError {
    ConditionPublishError::Disconnected
}

impl HciConditionPublisher {
    pub fn publish_reliable(
        &self,
        update: HciConditionUpdate,
    ) -> Result<(), ConditionPublishError> {
        if matches!(update, HciConditionUpdate::Telemetry(_)) {
            return Err(ConditionPublishError::WrongDeliveryClass);
        }
        self.reliable
            .try_send(ReliableConditionUpdate::Hci(Scoped {
                scope: self.scope,
                update,
            }))
            .map_err(map_reliable_error)
    }

    pub fn publish_telemetry(
        &self,
        update: HciConditionUpdate,
    ) -> Result<(), ConditionPublishError> {
        if !matches!(update, HciConditionUpdate::Telemetry(_)) {
            return Err(ConditionPublishError::WrongDeliveryClass);
        }
        self.telemetry
            .try_publish(Scoped {
                scope: self.scope,
                update,
            })
            .map_err(map_latest_error)
    }
}

impl MediaConditionPublisher {
    pub fn publish_reliable(
        &self,
        update: MediaConditionUpdate,
    ) -> Result<(), ConditionPublishError> {
        if matches!(update, MediaConditionUpdate::Telemetry(_)) {
            return Err(ConditionPublishError::WrongDeliveryClass);
        }
        self.reliable
            .try_send(ReliableConditionUpdate::Media(Scoped {
                scope: self.scope,
                update,
            }))
            .map_err(map_reliable_error)
    }

    pub fn publish_telemetry(
        &self,
        update: MediaConditionUpdate,
    ) -> Result<(), ConditionPublishError> {
        if !matches!(update, MediaConditionUpdate::Telemetry(_)) {
            return Err(ConditionPublishError::WrongDeliveryClass);
        }
        self.telemetry
            .try_publish(Scoped {
                scope: self.scope,
                update,
            })
            .map_err(map_latest_error)
    }
}

impl TransportConditionPublisher {
    pub fn publish_reliable(
        &self,
        update: TransportConditionUpdate,
    ) -> Result<(), ConditionPublishError> {
        if matches!(update, TransportConditionUpdate::Telemetry(_)) {
            return Err(ConditionPublishError::WrongDeliveryClass);
        }
        self.reliable
            .try_send(ReliableConditionUpdate::Transport(Scoped {
                scope: self.scope,
                update,
            }))
            .map_err(map_reliable_error)
    }

    pub fn publish_telemetry(
        &self,
        update: TransportConditionUpdate,
    ) -> Result<(), ConditionPublishError> {
        if !matches!(update, TransportConditionUpdate::Telemetry(_)) {
            return Err(ConditionPublishError::WrongDeliveryClass);
        }
        self.telemetry
            .try_publish(Scoped {
                scope: self.scope,
                update,
            })
            .map_err(map_latest_error)
    }
}

impl ConditionPublicationInbox {
    pub fn reconcile(
        &self,
        condition: &mut Condition,
    ) -> ConditionReconcileStats {
        let mut stats = ConditionReconcileStats::default();

        // Capacity is fixed, therefore draining the reliable lane completely is
        // bounded work.
        for _ in 0..CONDITION_RELIABLE_CAPACITY {
            let update = match self.reliable.try_recv() {
                Ok(update) => update,
                Err(RecvError::Empty | RecvError::Disconnected) => break,
            };
            let change = match update {
                ReliableConditionUpdate::Hci(scoped) => condition
                    .apply_hci(scoped.scope, scoped.update),
                ReliableConditionUpdate::Media(scoped) => condition
                    .apply_media(scoped.scope, scoped.update),
                ReliableConditionUpdate::Transport(scoped) => condition
                    .apply_transport(scoped.scope, scoped.update),
            };
            observe_change(change, &mut stats, true);
        }

        if let Ok(scoped) = self.hci_telemetry.try_recv() {
            let change = condition.apply_hci(scoped.scope, scoped.update);
            observe_change(change, &mut stats, false);
        }
        if let Ok(scoped) = self.media_telemetry.try_recv() {
            let change = condition.apply_media(scoped.scope, scoped.update);
            observe_change(change, &mut stats, false);
        }
        if let Ok(scoped) = self.transport_telemetry.try_recv() {
            let change =
                condition.apply_transport(scoped.scope, scoped.update);
            observe_change(change, &mut stats, false);
        }

        stats
    }
}

fn observe_change(
    change: ConditionChange,
    stats: &mut ConditionReconcileStats,
    reliable: bool,
) {
    if change.stale {
        stats.stale_rejected = stats.stale_rejected.saturating_add(1);
    } else if reliable {
        stats.reliable_applied = stats.reliable_applied.saturating_add(1);
    } else {
        stats.telemetry_applied = stats.telemetry_applied.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::lifecycle::{
            LifecycleSnapshot, LifecycleState, SessionState, StreamId,
        },
        plane::{
            l1_hci::interface::condition::{
                HciConditionState, HciHostState,
            },
            l1_media::{
                interface::condition::{
                    ClockLock, MediaConditionState, MediaState,
                },
                telemetry::{
                    MediaBridgeTelemetrySnapshot, MediaTelemetrySnapshot,
                    MediaTimingTelemetrySnapshot,
                },
            },
        },
        runtime::telemetry::TelemetryRingStats,
    };

    fn condition() -> Condition {
        Condition::new(LifecycleSnapshot {
            generation: Generation(7),
            session_id: Some(SessionId(11)),
            lifecycle: LifecycleState::Running,
            session: SessionState::Streaming,
        })
    }

    fn ring() -> TelemetryRingStats {
        TelemetryRingStats {
            capacity: 256,
            len: 0,
            stored: 0,
            dropped_newest: 0,
            overwritten_oldest: 0,
        }
    }

    #[test]
    fn reliable_state_and_per_plane_latest_telemetry_reconcile_together() {
        let (publishers, inbox) =
            runtime_channels(Generation(7), SessionId(11));
        let mut condition = condition();

        let state = HciConditionState {
            state: HciHostState::StreamBound,
            stream_id: Some(StreamId(9)),
        };
        publishers
            .hci
            .publish_reliable(HciConditionUpdate::State(state))
            .unwrap();

        let media = |intervals| MediaTelemetrySnapshot {
            timing: MediaTimingTelemetrySnapshot {
                produced_frame: None,
                clock: None,
                pcm_fill_frames: 480,
                intervals_consumed: intervals,
                source_intervals_skipped: 0,
                keepalive_intervals: 0,
                pcm_frames_read: intervals * 480,
                pcm_underflow_frames: 0,
                startup_trim_frames: 0,
                gradual_drift_trim_frames: 0,
                emergency_drift_trim_frames: 0,
                trace: ring(),
            },
            bridge: MediaBridgeTelemetrySnapshot {
                submitted_intervals: intervals,
                dropped_backpressure_intervals: 0,
                fallback_intervals: 0,
                skipped_intervals: 0,
                stale_frames_dropped: 0,
                trace: ring(),
            },
        };

        publishers
            .media
            .publish_telemetry(MediaConditionUpdate::Telemetry(media(1)))
            .unwrap();
        publishers
            .media
            .publish_telemetry(MediaConditionUpdate::Telemetry(media(2)))
            .unwrap();

        let stats = inbox.reconcile(&mut condition);
        assert_eq!(stats.reliable_applied, 1);
        assert_eq!(stats.telemetry_applied, 1);
        assert_eq!(stats.stale_rejected, 0);

        let snapshot = condition.snapshot();
        assert_eq!(snapshot.hci.state, state);
        assert_eq!(
            snapshot.media.telemetry.unwrap().timing.intervals_consumed,
            2
        );
    }

    #[test]
    fn telemetry_cannot_enter_reliable_lane() {
        let (publishers, _inbox) =
            runtime_channels(Generation(7), SessionId(11));
        let update = MediaConditionUpdate::Telemetry(
            MediaTelemetrySnapshot {
                timing: MediaTimingTelemetrySnapshot {
                    produced_frame: None,
                    clock: None,
                    pcm_fill_frames: 0,
                    intervals_consumed: 0,
                    source_intervals_skipped: 0,
                    keepalive_intervals: 0,
                    pcm_frames_read: 0,
                    pcm_underflow_frames: 0,
                    startup_trim_frames: 0,
                    gradual_drift_trim_frames: 0,
                    emergency_drift_trim_frames: 0,
                    trace: ring(),
                },
                bridge: MediaBridgeTelemetrySnapshot {
                    submitted_intervals: 0,
                    dropped_backpressure_intervals: 0,
                    fallback_intervals: 0,
                    skipped_intervals: 0,
                    stale_frames_dropped: 0,
                    trace: ring(),
                },
            },
        );
        assert_eq!(
            publishers.media.publish_reliable(update),
            Err(ConditionPublishError::WrongDeliveryClass)
        );
    }

    #[test]
    fn state_cannot_enter_lossy_latest_lane() {
        let (publishers, _inbox) =
            runtime_channels(Generation(7), SessionId(11));
        assert_eq!(
            publishers.media.publish_telemetry(
                MediaConditionUpdate::State(MediaConditionState {
                    state: MediaState::Running,
                    clock_lock: ClockLock::LocalMonotonic,
                })
            ),
            Err(ConditionPublishError::WrongDeliveryClass)
        );
    }
}
