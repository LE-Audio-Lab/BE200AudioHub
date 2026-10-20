//! Live PCM -> LC3 -> logical ISO product runtime.
//!
//! Media time is authoritative. PCM buffering is only a bounded cushion; old
//! PCM is consumed/discarded when the absolute media grid advances.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc,
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

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
            telemetry::MediaBridgeTelemetrySnapshot,
        },
    },
};

const LIVE_ENCODED_CAPACITY: usize = 4;
pub const LIVE_BRIDGE_START_LEAD_US: u64 = 20_000;
const WORKER_MAX_SLEEP_US: u64 = 2_000;

#[derive(Clone)]
pub struct LiveGain {
    linear_bits: Arc<AtomicU32>,
}

impl LiveGain {
    pub fn new(linear: f32) -> Self {
        let value = linear.clamp(0.0, 1.0);
        Self {
            linear_bits: Arc::new(AtomicU32::new(value.to_bits())),
        }
    }

    pub fn unity() -> Self {
        Self::new(1.0)
    }

    pub fn set_linear(&self, linear: f32) {
        let value = linear.clamp(0.0, 1.0);
        self.linear_bits
            .store(value.to_bits(), Ordering::Release);
    }

    pub fn linear(&self) -> f32 {
        f32::from_bits(self.linear_bits.load(Ordering::Acquire))
    }
}

impl Default for LiveGain {
    fn default() -> Self {
        Self::unity()
    }
}

fn apply_live_gain(frames: &mut [[f32; 2]], linear: f32) {
    if linear == 1.0 {
        return;
    }
    for frame in frames {
        frame[0] *= linear;
        frame[1] *= linear;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LivePcmError {
    Clock(StreamClockError),
    Codec(MediaLc3Error),
    Bridge(BridgeTxError),
    FrameInsert(FrameInsertOutcome),
    FrameIdOverflow,
    ConsumerFrameSizeMismatch,
    ThreadSpawn,
    StartupChannelClosed,
    ThreadPanicked,
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
    gain: LiveGain,
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
        Self::new_with_gain(
            binding,
            boundary,
            epoch_us,
            tx,
            LiveGain::unity(),
        )
    }

    pub fn new_with_gain(
        binding: StreamBinding,
        boundary: PcmRelayBoundary,
        epoch_us: u64,
        tx: HciLogicalIsoTx,
        gain: LiveGain,
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
            gain,
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

    pub fn bridge_telemetry(&self) -> MediaBridgeTelemetrySnapshot {
        self.bridge.telemetry_snapshot()
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

        apply_live_gain(&mut self.scratch, self.gain.linear());

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


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LivePcmWorkerExit {
    pub pcm: LivePcmStats,
    pub bridge: MediaBridgeTelemetrySnapshot,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LivePcmWorkerSnapshot {
    pub pcm: LivePcmStats,
    pub bridge_submitted_intervals: u64,
    pub bridge_dropped_backpressure_intervals: u64,
    pub bridge_fallback_intervals: u64,
    pub bridge_skipped_intervals: u64,
    pub bridge_stale_frames_dropped: u64,
    pub pcm_fill_frames: usize,
}

#[derive(Default)]
struct LivePcmWorkerTelemetry {
    intervals_consumed: AtomicU64,
    source_intervals_skipped: AtomicU64,
    keepalive_intervals: AtomicU64,
    pcm_frames_read: AtomicU64,
    pcm_underflow_frames: AtomicU64,
    startup_trim_frames: AtomicU64,
    gradual_drift_trim_frames: AtomicU64,
    emergency_drift_trim_frames: AtomicU64,
    bridge_submitted_intervals: AtomicU64,
    bridge_dropped_backpressure_intervals: AtomicU64,
    bridge_fallback_intervals: AtomicU64,
    bridge_skipped_intervals: AtomicU64,
    bridge_stale_frames_dropped: AtomicU64,
    pcm_fill_frames: AtomicUsize,
}

impl LivePcmWorkerTelemetry {
    fn publish(&self, runtime: &LivePcmIsoRuntime) {
        let pcm = runtime.stats();
        let bridge = runtime.bridge_telemetry();

        self.intervals_consumed
            .store(pcm.intervals_consumed, Ordering::Relaxed);
        self.source_intervals_skipped
            .store(pcm.source_intervals_skipped, Ordering::Relaxed);
        self.keepalive_intervals
            .store(pcm.keepalive_intervals, Ordering::Relaxed);
        self.pcm_frames_read
            .store(pcm.pcm_frames_read, Ordering::Relaxed);
        self.pcm_underflow_frames
            .store(pcm.pcm_underflow_frames, Ordering::Relaxed);
        self.startup_trim_frames
            .store(pcm.startup_trim_frames, Ordering::Relaxed);
        self.gradual_drift_trim_frames
            .store(pcm.gradual_drift_trim_frames, Ordering::Relaxed);
        self.emergency_drift_trim_frames
            .store(pcm.emergency_drift_trim_frames, Ordering::Relaxed);
        self.bridge_submitted_intervals
            .store(bridge.submitted_intervals, Ordering::Relaxed);
        self.bridge_dropped_backpressure_intervals.store(
            bridge.dropped_backpressure_intervals,
            Ordering::Relaxed,
        );
        self.bridge_fallback_intervals
            .store(bridge.fallback_intervals, Ordering::Relaxed);
        self.bridge_skipped_intervals
            .store(bridge.skipped_intervals, Ordering::Relaxed);
        self.bridge_stale_frames_dropped
            .store(bridge.stale_frames_dropped, Ordering::Relaxed);
        self.pcm_fill_frames
            .store(runtime.pcm_fill_frames(), Ordering::Relaxed);
    }

    fn snapshot(&self) -> LivePcmWorkerSnapshot {
        LivePcmWorkerSnapshot {
            pcm: LivePcmStats {
                intervals_consumed: self
                    .intervals_consumed
                    .load(Ordering::Relaxed),
                source_intervals_skipped: self
                    .source_intervals_skipped
                    .load(Ordering::Relaxed),
                keepalive_intervals: self
                    .keepalive_intervals
                    .load(Ordering::Relaxed),
                pcm_frames_read: self
                    .pcm_frames_read
                    .load(Ordering::Relaxed),
                pcm_underflow_frames: self
                    .pcm_underflow_frames
                    .load(Ordering::Relaxed),
                startup_trim_frames: self
                    .startup_trim_frames
                    .load(Ordering::Relaxed),
                gradual_drift_trim_frames: self
                    .gradual_drift_trim_frames
                    .load(Ordering::Relaxed),
                emergency_drift_trim_frames: self
                    .emergency_drift_trim_frames
                    .load(Ordering::Relaxed),
            },
            bridge_submitted_intervals: self
                .bridge_submitted_intervals
                .load(Ordering::Relaxed),
            bridge_dropped_backpressure_intervals: self
                .bridge_dropped_backpressure_intervals
                .load(Ordering::Relaxed),
            bridge_fallback_intervals: self
                .bridge_fallback_intervals
                .load(Ordering::Relaxed),
            bridge_skipped_intervals: self
                .bridge_skipped_intervals
                .load(Ordering::Relaxed),
            bridge_stale_frames_dropped: self
                .bridge_stale_frames_dropped
                .load(Ordering::Relaxed),
            pcm_fill_frames: self
                .pcm_fill_frames
                .load(Ordering::Relaxed),
        }
    }
}

pub struct LivePcmIsoWorker {
    stop: Arc<AtomicBool>,
    telemetry: Arc<LivePcmWorkerTelemetry>,
    join: Option<thread::JoinHandle<Result<LivePcmWorkerExit, LivePcmError>>>,
}

impl LivePcmIsoWorker {
    pub fn start(
        binding: StreamBinding,
        boundary: PcmRelayBoundary,
        tx: HciLogicalIsoTx,
    ) -> Result<Self, LivePcmError> {
        Self::start_with_gain(
            binding,
            boundary,
            tx,
            LiveGain::unity(),
        )
    }

    pub fn start_with_gain(
        binding: StreamBinding,
        boundary: PcmRelayBoundary,
        tx: HciLogicalIsoTx,
        gain: LiveGain,
    ) -> Result<Self, LivePcmError> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let telemetry = Arc::new(LivePcmWorkerTelemetry::default());
        let thread_telemetry = Arc::clone(&telemetry);
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);

        let join = thread::Builder::new()
            .name("be200-media-live".into())
            .spawn(move || {
                let origin = Instant::now();
                let mut runtime = match LivePcmIsoRuntime::new_with_gain(
                    binding,
                    boundary,
                    LIVE_BRIDGE_START_LEAD_US,
                    tx,
                    gain,
                ) {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = startup_tx.send(Err(error.clone()));
                        return Err(error);
                    }
                };

                runtime.arm_source();
                thread_telemetry.publish(&runtime);
                let _ = startup_tx.send(Ok(()));

                while !thread_stop.load(Ordering::Acquire) {
                    let now_us = u64::try_from(
                        origin.elapsed().as_micros(),
                    )
                    .unwrap_or(u64::MAX);

                    match runtime.on_wake(now_us)? {
                        LivePcmStep::Wait { deadline_us, .. } => {
                            if deadline_us > now_us {
                                let sleep_us = (deadline_us - now_us)
                                    .min(WORKER_MAX_SLEEP_US);
                                thread::sleep(Duration::from_micros(
                                    sleep_us,
                                ));
                            } else {
                                thread::yield_now();
                            }
                        }
                        LivePcmStep::Interval { .. } => {
                            thread_telemetry.publish(&runtime);
                        }
                    }
                }

                runtime.disable_source();
                thread_telemetry.publish(&runtime);
                Ok(LivePcmWorkerExit {
                    pcm: runtime.stats(),
                    bridge: runtime.bridge_telemetry(),
                })
            })
            .map_err(|_| LivePcmError::ThreadSpawn)?;

        match startup_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                telemetry,
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(_) => {
                let _ = join.join();
                Err(LivePcmError::StartupChannelClosed)
            }
        }
    }

    pub fn snapshot(&self) -> LivePcmWorkerSnapshot {
        self.telemetry.snapshot()
    }

    pub fn shutdown(
        mut self,
    ) -> Result<LivePcmWorkerExit, LivePcmError> {
        self.stop.store(true, Ordering::Release);
        let Some(join) = self.join.take() else {
            return Err(LivePcmError::StartupChannelClosed);
        };
        join.join().map_err(|_| LivePcmError::ThreadPanicked)?
    }
}

impl Drop for LivePcmIsoWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
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
    fn live_gain_clamps_and_scales_stereo_pcm() {
        let gain = LiveGain::new(2.0);
        assert_eq!(gain.linear(), 1.0);

        gain.set_linear(-1.0);
        assert_eq!(gain.linear(), 0.0);

        gain.set_linear(0.5);
        let mut frames = [[0.8, -0.4], [0.2, 0.6]];
        apply_live_gain(&mut frames, gain.linear());
        assert_eq!(frames, [[0.4, -0.2], [0.1, 0.3]]);
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
    fn worker_telemetry_snapshot_mirrors_runtime_without_owning_time() {
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

        let frames = vec![[0.2, -0.2]; 960];
        assert_eq!(producer.push(&frames, false).written_frames, 960);
        runtime.on_wake(EPOCH_US).unwrap();

        let telemetry = LivePcmWorkerTelemetry::default();
        telemetry.publish(&runtime);
        let snapshot = telemetry.snapshot();

        assert_eq!(snapshot.pcm.intervals_consumed, 1);
        assert_eq!(snapshot.pcm.pcm_frames_read, 480);
        assert_eq!(snapshot.bridge_submitted_intervals, 1);
        assert_eq!(snapshot.bridge_dropped_backpressure_intervals, 0);
        assert_eq!(snapshot.pcm_fill_frames, 480);
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
