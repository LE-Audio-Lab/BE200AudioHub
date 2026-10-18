//! Nonblocking runtime edge from the interval scheduler to HCI Host.
//!
//! Media time never waits for HCI/control progress. Each wake may submit at
//! most one complete stereo interval group. A full bounded HCI data lane drops
//! that interval atomically; it is never retried on a later media interval.

use crate::{
    architecture::{
        control::StreamBinding,
        media::EncodedStereoFrame,
        types::FrameId,
    },
    plane::l1_hci::interface::iso_tx::HciLogicalIsoTx,
    runtime::mailbox::SendError,
};

use super::{
    FrameInsertOutcome, IntervalGroupScheduler,
    IntervalGroupSource, LogicalGroupError, SchedulerError,
    SchedulerStep, StereoFallback,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeTxStep {
    Wait {
        frame_id: FrameId,
        deadline_us: u64,
    },
    Submitted {
        frame_id: FrameId,
        source: IntervalGroupSource,
        skipped_intervals: u64,
        stale_frames_dropped: usize,
    },
    DroppedBackpressure {
        frame_id: FrameId,
        source: IntervalGroupSource,
        skipped_intervals: u64,
        stale_frames_dropped: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeTxError {
    Scheduler(SchedulerError),
    LogicalGroup(LogicalGroupError),
    HciDisconnected,
}

pub struct LogicalIsoTxBridge {
    scheduler: IntervalGroupScheduler<Vec<u8>>,
    tx: HciLogicalIsoTx,
}

impl LogicalIsoTxBridge {
    pub fn new(
        binding: StreamBinding,
        epoch_us: u64,
        capacity: usize,
        fallback: StereoFallback<Vec<u8>>,
        tx: HciLogicalIsoTx,
    ) -> Result<Self, BridgeTxError> {
        let scheduler = IntervalGroupScheduler::new(
            binding,
            epoch_us,
            capacity,
            fallback,
        )
        .map_err(BridgeTxError::Scheduler)?;

        Ok(Self { scheduler, tx })
    }

    pub fn push_encoded(
        &mut self,
        frame: EncodedStereoFrame<Vec<u8>>,
    ) -> FrameInsertOutcome {
        self.scheduler.push_encoded(frame)
    }

    pub fn stored_len(&self) -> usize {
        self.scheduler.stored_len()
    }

    pub fn next_deadline_us(
        &self,
    ) -> Result<u64, BridgeTxError> {
        self.scheduler
            .next_deadline_us()
            .map_err(BridgeTxError::Scheduler)
    }

    pub fn on_wake(
        &mut self,
        now_us: u64,
    ) -> Result<BridgeTxStep, BridgeTxError> {
        match self
            .scheduler
            .on_wake(now_us)
            .map_err(BridgeTxError::Scheduler)?
        {
            SchedulerStep::Wait {
                frame_id,
                deadline_us,
            } => Ok(BridgeTxStep::Wait {
                frame_id,
                deadline_us,
            }),
            SchedulerStep::Submit(group) => {
                let frame_id = group.frame_id;
                let source = group.source;
                let skipped_intervals = group.skipped_intervals;
                let stale_frames_dropped =
                    group.stale_frames_dropped;

                let request = self
                    .scheduler
                    .into_logical_iso_request(group)
                    .map_err(BridgeTxError::LogicalGroup)?;

                match self.tx.try_submit_group(request) {
                    Ok(()) => Ok(BridgeTxStep::Submitted {
                        frame_id,
                        source,
                        skipped_intervals,
                        stale_frames_dropped,
                    }),
                    Err(SendError::Full) => {
                        Ok(BridgeTxStep::DroppedBackpressure {
                            frame_id,
                            source,
                            skipped_intervals,
                            stale_frames_dropped,
                        })
                    }
                    Err(SendError::Disconnected) => {
                        Err(BridgeTxError::HciDisconnected)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        architecture::{
            control::{
                LeAudioFormat, StreamBinding, StreamTopology,
            },
            media::EncodedStereoFrame,
            types::{FrameId, SessionId, StreamId},
        },
        plane::l1_hci::interface::mailbox::hci_mailbox_with_iso,
        plane::l1_hci::iso::tx::IsoDataGroupRequest,
    };

    use super::*;

    const EPOCH_US: u64 = 1_000_000;

    fn binding() -> StreamBinding {
        StreamBinding {
            session_id: SessionId(7),
            stream_id: StreamId(11),
            format: LeAudioFormat {
                sample_rate_hz: 48_000,
                frame_duration_us: 10_000,
                octets_per_sdu: 100,
            },
            presentation_delay_us: 40_000,
            topology: StreamTopology::StereoSplit,
        }
    }

    fn fallback() -> StereoFallback<Vec<u8>> {
        StereoFallback {
            left: vec![0xFA; 100],
            right: vec![0xFB; 100],
        }
    }

    fn frame(id: u64) -> EncodedStereoFrame<Vec<u8>> {
        EncodedStereoFrame {
            frame_id: FrameId(id),
            left: vec![0x10 + id as u8; 100],
            right: vec![0x20 + id as u8; 100],
        }
    }

    #[test]
    fn bridge_submits_handle_free_logical_group_to_shared_hci_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 1, 1, 2);
        let mut bridge = LogicalIsoTxBridge::new(
            binding(),
            EPOCH_US,
            2,
            fallback(),
            control.logical_iso_tx(),
        )
        .unwrap();

        assert_eq!(
            bridge.push_encoded(frame(0)),
            FrameInsertOutcome::Accepted
        );
        assert_eq!(
            bridge.on_wake(EPOCH_US).unwrap(),
            BridgeTxStep::Submitted {
                frame_id: FrameId(0),
                source: IntervalGroupSource::Encoded,
                skipped_intervals: 0,
                stale_frames_dropped: 0,
            }
        );

        let IsoDataGroupRequest::Logical(group) =
            ports.iso_rx.try_recv().unwrap()
        else {
            panic!("bridge must use logical HCI data plane");
        };
        assert_eq!(group.session_id, SessionId(7));
        assert_eq!(group.stream_id, StreamId(11));
        assert_eq!(group.left, vec![0x10; 100]);
        assert_eq!(group.right, vec![0x20; 100]);
    }

    #[test]
    fn full_hci_lane_drops_interval_without_retrying_media_time() {
        let (control, _ingress, _reports, _ports) =
            hci_mailbox_with_iso(1, 1, 1, 1);
        let mut bridge = LogicalIsoTxBridge::new(
            binding(),
            EPOCH_US,
            2,
            fallback(),
            control.logical_iso_tx(),
        )
        .unwrap();

        bridge.push_encoded(frame(0));
        bridge.push_encoded(frame(1));

        assert!(matches!(
            bridge.on_wake(EPOCH_US).unwrap(),
            BridgeTxStep::Submitted {
                frame_id: FrameId(0),
                ..
            }
        ));
        assert!(matches!(
            bridge.on_wake(EPOCH_US + 10_000).unwrap(),
            BridgeTxStep::DroppedBackpressure {
                frame_id: FrameId(1),
                ..
            }
        ));

        assert_eq!(
            bridge.next_deadline_us().unwrap(),
            EPOCH_US + 20_000
        );
        assert_eq!(bridge.stored_len(), 0);
    }

    #[test]
    fn wake_before_epoch_stays_waiting_and_does_not_touch_hci_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 1, 1, 1);
        let mut bridge = LogicalIsoTxBridge::new(
            binding(),
            EPOCH_US,
            1,
            fallback(),
            control.logical_iso_tx(),
        )
        .unwrap();

        assert_eq!(
            bridge.on_wake(EPOCH_US - 250).unwrap(),
            BridgeTxStep::Wait {
                frame_id: FrameId(0),
                deadline_us: EPOCH_US,
            }
        );
        assert!(ports.iso_rx.try_recv().is_err());
    }

    #[test]
    fn disconnected_hci_lane_is_terminal_not_backpressure() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 1, 1, 1);
        let mut bridge = LogicalIsoTxBridge::new(
            binding(),
            EPOCH_US,
            1,
            fallback(),
            control.logical_iso_tx(),
        )
        .unwrap();
        drop(ports.iso_rx);

        assert_eq!(
            bridge.on_wake(EPOCH_US),
            Err(BridgeTxError::HciDisconnected)
        );
    }
}
