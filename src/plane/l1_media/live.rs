//! Live PCM -> LC3 -> logical ISO product runtime.
//!
//! Media time is authoritative. PCM buffering is only a bounded cushion; old
//! PCM is consumed/discarded when the absolute media grid advances.

use crate::{
    plane::{
        l1_control::interface::stream::StreamBinding,
        l1_hci::interface::iso_tx::HciLogicalIsoTx,
        l1_media::{
            bridge::{
                runtime::{
                    BridgeTxError, BridgeTxStep, LogicalIsoTxBridge,
                },
                scheduler::{FrameInsertOutcome, StereoFallback},
            },
            clock::{
                frame::FrameId,
                local::{LocalStreamClock, StreamClockError},
            },
            codec::lc3::{
                encode_lc3_silence, Lc3StereoEncoder, MediaLc3Error,
            },
            pcm::relay_boundary::{
                PcmConsumeResult, PcmRelayBoundary,
                LIVE_PCM_CONSUMER_FRAMES,
            },
            source::synthetic::SAMPLES_PER_FRAME,
        },
    },
};

const LIVE_ENCODED_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LivePcmError {
    Clock(StreamClockError),
    Codec(MediaLc3Error),
    Bridge(BridgeTxError),
    FrameInsert(FrameInsertOutcome),
    FrameIdOverflow,
    ConsumerFrameSizeMismatch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LivePcmStats {
    pub intervals_consumed: u64,
    pub source_intervals_skipped: u64,
    pub keepalive_intervals: u64,
    pub pcm_frames_read: u64,
    pub pcm_underflow_frames: u64,
    pub startup_trim_frames: u64,
    pub gradual_drift_trim_frames: u64,
    pub emergency_drift_trim_frames: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LivePcmStep {
    Wait {
        frame_id: FrameId,
        deadline_us: u64,
    },
    Interval {
        frame_id: FrameId,
        source_intervals_skipped: u64,
        pcm: PcmConsumeResult,
        bridge: BridgeTxStep,
    },
}

pub struct LivePcmIsoRuntime {
    clock: LocalStreamClock,
    next_source_frame: FrameId,
    boundary: PcmRelayBoundary,
    encoder: Lc3StereoEncoder,
    bridge: LogicalIsoTxBridge,
    scratch: [[f32; 2]; SAMPLES_PER_FRAME],
    stats: LivePcmStats,
}

impl LivePcmIsoRuntime {
    pub fn new(
        binding: StreamBinding,
        boundary: PcmRelayBoundary,
        epoch_us: u64,
        tx: HciLogicalIsoTx,
    ) -> Result<Self, LivePcmError> {
        if LIVE_PCM_CONSUMER_FRAMES != SAMPLES_PER_FRAME {
            return Err(LivePcmError::ConsumerFrameSizeMismatch);
        }

        let clock = LocalStreamClock::from_binding(binding, epoch_us)
            .map_err(LivePcmError::Clock)?;
        let encoder =
            Lc3StereoEncoder::new(binding.format).map_err(LivePcmError::Codec)?;
        let (left, right) =
            encode_lc3_silence(binding.format).map_err(LivePcmError::Codec)?;
        let bridge = LogicalIsoTxBridge::new(
            binding,
            epoch_us,
            LIVE_ENCODED_CAPACITY,
            StereoFallback { left, right },
            tx,
        )
        .map_err(LivePcmError::Bridge)?;

        Ok(Self {
            clock,
            next_source_frame: FrameId(0),
            boundary,
            encoder,
            bridge,
            scratch: [[0.0; 2]; SAMPLES_PER_FRAME],
            stats: LivePcmStats::default(),
        })
    }

    pub fn arm_source(&self) {
        self.boundary.arm();
    }

    pub fn disable_source(&self) {
        self.boundary.disable();
    }

    pub fn stats(&self) -> LivePcmStats {
        self.stats
    }

    pub fn pcm_fill_frames(&self) -> usize {
        self.boundary.fill_frames()
    }

    pub fn next_deadline_us(&self) -> Result<u64, LivePcmError> {
        self.clock
            .target_time_us(self.next_source_frame)
            .map_err(LivePcmError::Clock)
    }

    fn observe_consume(&mut self, result: PcmConsumeResult) {
        self.stats.intervals_consumed =
            self.stats.intervals_consumed.saturating_add(1);
        self.stats.pcm_frames_read = self
            .stats
            .pcm_frames_read
            .saturating_add(result.read_frames as u64);
        self.stats.pcm_underflow_frames = self
            .stats
            .pcm_underflow_frames
            .saturating_add(
                result
                    .requested_frames
                    .saturating_sub(result.read_frames) as u64,
            );
        self.stats.startup_trim_frames = self
            .stats
            .startup_trim_frames
            .saturating_add(result.startup_trim_frames as u64);
        self.stats.gradual_drift_trim_frames = self
            .stats
            .gradual_drift_trim_frames
            .saturating_add(result.drift_trim.gradual_frames as u64);
        self.stats.emergency_drift_trim_frames = self
            .stats
            .emergency_drift_trim_frames
            .saturating_add(result.drift_trim.emergency_frames as u64);
        if result.keepalive {
            self.stats.keepalive_intervals =
                self.stats.keepalive_intervals.saturating_add(1);
        }
    }

    fn consume_discarded_interval(&mut self) {
        let result = self.boundary.consume(&mut self.scratch);
        self.observe_consume(result);
    }

    pub fn on_wake(
        &mut self,
        now_us: u64,
    ) -> Result<LivePcmStep, LivePcmError> {
        let Some(current) = self.clock.current_frame_at(now_us) else {
            return Ok(LivePcmStep::Wait {
                frame_id: self.next_source_frame,
                deadline_us: self.next_deadline_us()?,
            });
        };

        if current.0 < self.next_source_frame.0 {
            return Ok(LivePcmStep::Wait {
                frame_id: self.next_source_frame,
                deadline_us: self.next_deadline_us()?,
            });
        }

        let skipped =
            current.0.saturating_sub(self.next_source_frame.0);
        for _ in 0..skipped {
            self.consume_discarded_interval();
        }
        self.stats.source_intervals_skipped = self
            .stats
            .source_intervals_skipped
            .saturating_add(skipped);

        let pcm = self.boundary.consume(&mut self.scratch);
        self.observe_consume(pcm);

        let encoded = self
            .encoder
            .encode_float32_stereo(current, &self.scratch)
            .map_err(LivePcmError::Codec)?;
        let inserted = self.bridge.push_encoded(encoded);
        if inserted != FrameInsertOutcome::Accepted {
            return Err(LivePcmError::FrameInsert(inserted));
        }

        self.next_source_frame = FrameId(
            current.0.checked_add(1).ok_or(
                LivePcmError::FrameIdOverflow,
            )?,
        );

        let bridge = self
            .bridge
            .on_wake(now_us)
            .map_err(LivePcmError::Bridge)?;

        Ok(LivePcmStep::Interval {
            frame_id: current,
            source_intervals_skipped: skipped,
            pcm,
            bridge,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        core::lifecycle::{SessionId, StreamId},
        plane::{
            l1_control::interface::stream::{
                LeAudioFormat, StreamBinding, StreamTopology,
            },
            l1_hci::{
                interface::mailbox::hci_mailbox_with_iso,
                iso::tx::IsoDataGroupRequest,
            },
            l1_media::pcm::relay_boundary::PcmRelayBoundary,
        },
    };

    use super::*;

    const EPOCH_US: u64 = 20_000;

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

    #[test]
    fn before_epoch_waits_without_consuming_pcm() {
        let (boundary, producer) = PcmRelayBoundary::relay_defaults();
        let (control, _ingress, _reports, _ports) =
            hci_mailbox_with_iso(1, 1, 1, 4);
        let mut runtime = LivePcmIsoRuntime::new(
            binding(),
            boundary,
            EPOCH_US,
            control.logical_iso_tx(),
        )
        .unwrap();
        runtime.arm_source();
        let frames = vec![[0.25, -0.25]; 960];
        assert_eq!(producer.push(&frames, false).written_frames, 960);

        assert_eq!(
            runtime.on_wake(EPOCH_US - 1).unwrap(),
            LivePcmStep::Wait {
                frame_id: FrameId(0),
                deadline_us: EPOCH_US,
            }
        );
        assert_eq!(runtime.pcm_fill_frames(), 960);
    }

    #[test]
    fn due_interval_consumes_pcm_encodes_lc3_and_submits_logical_group() {
        let (boundary, producer) = PcmRelayBoundary::relay_defaults();
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 1, 1, 4);
        let mut runtime = LivePcmIsoRuntime::new(
            binding(),
            boundary,
            EPOCH_US,
            control.logical_iso_tx(),
        )
        .unwrap();
        runtime.arm_source();

        let frames = vec![[0.25, -0.25]; 960];
        assert_eq!(producer.push(&frames, false).written_frames, 960);

        let step = runtime.on_wake(EPOCH_US).unwrap();
        let LivePcmStep::Interval {
            frame_id,
            pcm,
            bridge,
            ..
        } = step
        else {
            panic!("expected live interval");
        };
        assert_eq!(frame_id, FrameId(0));
        assert_eq!(pcm.read_frames, 480);
        assert!(!pcm.keepalive);
        assert!(matches!(
            bridge,
            BridgeTxStep::Submitted {
                frame_id: FrameId(0),
                ..
            }
        ));

        let request = ports.iso_rx.try_recv().unwrap();
        let IsoDataGroupRequest::Logical(group) = request else {
            panic!("expected logical ISO group");
        };
        assert_eq!(group.session_id, SessionId(7));
        assert_eq!(group.stream_id, StreamId(11));
        assert_eq!(group.left.len(), 100);
        assert_eq!(group.right.len(), 100);
        assert_eq!(runtime.pcm_fill_frames(), 480);
    }

    #[test]
    fn late_media_wake_discards_old_pcm_intervals_before_encoding_current() {
        let (boundary, producer) = PcmRelayBoundary::relay_defaults();
        let (control, _ingress, _reports, _ports) =
            hci_mailbox_with_iso(1, 1, 1, 4);
        let mut runtime = LivePcmIsoRuntime::new(
            binding(),
            boundary,
            EPOCH_US,
            control.logical_iso_tx(),
        )
        .unwrap();
        runtime.arm_source();

        let frames = vec![[0.1, -0.1]; 1_920];
        assert_eq!(producer.push(&frames, false).written_frames, 1_920);

        let step = runtime.on_wake(EPOCH_US + 20_000).unwrap();
        let LivePcmStep::Interval {
            frame_id,
            source_intervals_skipped,
            ..
        } = step
        else {
            panic!("expected live interval");
        };
        assert_eq!(frame_id, FrameId(2));
        assert_eq!(source_intervals_skipped, 2);
        assert_eq!(runtime.stats().source_intervals_skipped, 2);
    }
}
