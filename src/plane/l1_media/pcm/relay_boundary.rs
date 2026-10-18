//! Live PCM startup/cushion boundary ported from LE-Audio-Relay.
//!
//! Producer: Windows Process Loopback capture.
//! Consumer: L1-Media 10 ms frame clock.
//!
//! Unlike the old Relay, the consumer is not a WASAPI render sink. Before
//! activation this boundary emits real zeros to Media; after activation it
//! feeds PCM that will be LC3 encoded and submitted through IsoTxBridge.

use std::sync::atomic::{AtomicU8, Ordering};

use super::{
    drift::{DriftTrimResult, ProvisionalPositiveDriftGuard},
    ring::SpscStereoPcmRing,
};

const MODE_KEEPALIVE: u8 = 0;
const MODE_ARMED: u8 = 1;
const MODE_RELAY: u8 = 2;

pub const LIVE_PCM_SAMPLE_RATE_HZ: usize = 48_000;
pub const LIVE_PCM_RING_CAPACITY_FRAMES: usize = 3_840; // 80 ms
pub const LIVE_PCM_TARGET_CUSHION_FRAMES: usize = 480; // 10 ms
pub const LIVE_PCM_STARTUP_HOLD_FRAMES: usize = 1_920; // 40 ms
pub const LIVE_PCM_CONSUMER_FRAMES: usize = 480; // 10 ms

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmBoundaryMode {
    KeepAlive,
    Armed,
    Relay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PcmPushResult {
    pub input_frames: usize,
    pub written_frames: usize,
    pub intentional_silent_trim_frames: usize,
    pub dropped_frames: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PcmConsumeResult {
    pub requested_frames: usize,
    pub read_frames: usize,
    pub keepalive: bool,
    pub startup_trim_frames: usize,
    pub drift_trim: DriftTrimResult,
}

#[derive(Debug)]
pub struct PcmRelayBoundary {
    ring: SpscStereoPcmRing,
    drift_guard: ProvisionalPositiveDriftGuard,
    mode: AtomicU8,
    target_cushion_frames: usize,
    startup_hold_frames: usize,
}

impl PcmRelayBoundary {
    pub fn relay_defaults() -> Self {
        Self::new(
            LIVE_PCM_SAMPLE_RATE_HZ,
            LIVE_PCM_RING_CAPACITY_FRAMES,
            LIVE_PCM_TARGET_CUSHION_FRAMES,
            LIVE_PCM_STARTUP_HOLD_FRAMES,
        )
        .expect("frozen live PCM boundary parameters are valid")
    }

    pub fn new(
        sample_rate_hz: usize,
        capacity_frames: usize,
        target_cushion_frames: usize,
        startup_hold_frames: usize,
    ) -> Result<Self, &'static str> {
        if target_cushion_frames == 0
            || target_cushion_frames >= capacity_frames
        {
            return Err("target cushion must fit inside PCM ring");
        }
        if startup_hold_frames < target_cushion_frames
            || startup_hold_frames > capacity_frames
        {
            return Err("startup hold must fit between cushion and capacity");
        }

        Ok(Self {
            ring: SpscStereoPcmRing::new(capacity_frames)?,
            drift_guard: ProvisionalPositiveDriftGuard::new(
                sample_rate_hz,
                capacity_frames,
                target_cushion_frames,
            )?,
            mode: AtomicU8::new(MODE_KEEPALIVE),
            target_cushion_frames,
            startup_hold_frames,
        })
    }

    pub fn mode(&self) -> PcmBoundaryMode {
        match self.mode.load(Ordering::Acquire) {
            MODE_ARMED => PcmBoundaryMode::Armed,
            MODE_RELAY => PcmBoundaryMode::Relay,
            _ => PcmBoundaryMode::KeepAlive,
        }
    }

    pub fn fill_frames(&self) -> usize {
        self.ring.fill_frames()
    }

    pub fn arm(&self) {
        self.mode.store(MODE_ARMED, Ordering::Release);
    }

    pub fn disable(&self) {
        self.mode.store(MODE_KEEPALIVE, Ordering::Release);
    }

    pub fn push(
        &mut self,
        frames: &[[f32; 2]],
        silent: bool,
    ) -> PcmPushResult {
        let input_frames = frames.len();
        if input_frames == 0 {
            return PcmPushResult::default();
        }

        let runtime = self.mode() == PcmBoundaryMode::Relay;
        let intentional_silent_trim_frames = if runtime && silent {
            self.drift_guard.silent_frames_to_suppress(
                input_frames,
                self.ring.fill_frames(),
            )
        } else {
            0
        };
        let frames_to_write =
            input_frames.saturating_sub(intentional_silent_trim_frames);
        let max_fill = if runtime {
            self.ring.capacity_frames()
        } else {
            self.startup_hold_frames
        };

        let written_frames = if frames_to_write == 0 {
            0
        } else if silent {
            self.ring.write_silence(frames_to_write, max_fill)
        } else {
            self.ring.write(&frames[..frames_to_write], max_fill)
        };

        PcmPushResult {
            input_frames,
            written_frames,
            intentional_silent_trim_frames,
            dropped_frames: frames_to_write.saturating_sub(written_frames),
        }
    }

    pub fn consume(
        &mut self,
        destination: &mut [[f32; 2]],
    ) -> PcmConsumeResult {
        destination.fill([0.0, 0.0]);

        let requested_frames = destination.len();
        if requested_frames == 0 {
            return PcmConsumeResult::default();
        }

        let mut mode = self.mode();
        let mut startup_trim_frames = 0;

        if mode == PcmBoundaryMode::Armed {
            let required_fill =
                requested_frames.saturating_add(self.target_cushion_frames);
            let observed_fill = self.ring.fill_frames();

            if observed_fill >= required_fill
                && self
                    .mode
                    .compare_exchange(
                        MODE_ARMED,
                        MODE_RELAY,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
            {
                startup_trim_frames =
                    self.ring.discard(observed_fill - required_fill);
            }

            mode = self.mode();
        }

        if mode != PcmBoundaryMode::Relay {
            return PcmConsumeResult {
                requested_frames,
                read_frames: 0,
                keepalive: true,
                startup_trim_frames,
                drift_trim: DriftTrimResult::default(),
            };
        }

        let read_frames = self.ring.read(destination);
        let drift_trim = self.drift_guard.apply_after_consume(&self.ring);

        PcmConsumeResult {
            requested_frames,
            read_frames,
            keepalive: false,
            startup_trim_frames,
            drift_trim,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(count: usize, value: f32) -> Vec<[f32; 2]> {
        vec![[value, -value]; count]
    }

    #[test]
    fn defaults_match_validated_relay_buffer_contract() {
        let boundary = PcmRelayBoundary::relay_defaults();
        assert_eq!(boundary.mode(), PcmBoundaryMode::KeepAlive);
        assert_eq!(LIVE_PCM_RING_CAPACITY_FRAMES, 3_840);
        assert_eq!(LIVE_PCM_TARGET_CUSHION_FRAMES, 480);
        assert_eq!(LIVE_PCM_STARTUP_HOLD_FRAMES, 1_920);
        assert_eq!(LIVE_PCM_CONSUMER_FRAMES, 480);
    }

    #[test]
    fn keepalive_and_armed_emit_real_zero_until_request_plus_cushion_exists() {
        let mut boundary = PcmRelayBoundary::relay_defaults();
        let mut out = [[1.0f32, 1.0f32]; LIVE_PCM_CONSUMER_FRAMES];

        let first = boundary.consume(&mut out);
        assert!(first.keepalive);
        assert!(out.iter().all(|frame| *frame == [0.0, 0.0]));

        boundary.arm();
        assert_eq!(boundary.push(&frames(959, 0.25), false).written_frames, 959);

        let still_armed = boundary.consume(&mut out);
        assert!(still_armed.keepalive);
        assert_eq!(boundary.mode(), PcmBoundaryMode::Armed);
    }

    #[test]
    fn relay_activates_at_twenty_ms_and_retains_ten_ms_cushion() {
        let mut boundary = PcmRelayBoundary::relay_defaults();
        boundary.arm();
        assert_eq!(boundary.push(&frames(960, 0.25), false).written_frames, 960);

        let mut out = [[0.0f32, 0.0f32]; LIVE_PCM_CONSUMER_FRAMES];
        let result = boundary.consume(&mut out);

        assert!(!result.keepalive);
        assert_eq!(result.read_frames, 480);
        assert_eq!(boundary.mode(), PcmBoundaryMode::Relay);
        assert_eq!(boundary.fill_frames(), 480);
        assert!(out.iter().all(|frame| *frame == [0.25, -0.25]));
    }

    #[test]
    fn preactivation_capture_is_bounded_by_forty_ms_startup_hold() {
        let mut boundary = PcmRelayBoundary::relay_defaults();
        boundary.arm();

        let result = boundary.push(&frames(2_400, 0.5), false);
        assert_eq!(result.written_frames, 1_920);
        assert_eq!(result.dropped_frames, 480);
        assert_eq!(boundary.fill_frames(), 1_920);

        let mut out = [[0.0f32, 0.0f32]; LIVE_PCM_CONSUMER_FRAMES];
        let activation = boundary.consume(&mut out);

        assert_eq!(activation.startup_trim_frames, 960);
        assert_eq!(activation.read_frames, 480);
        assert_eq!(boundary.fill_frames(), 480);
    }

    #[test]
    fn relay_underflow_zero_fills_missing_tail() {
        let mut boundary = PcmRelayBoundary::relay_defaults();
        boundary.arm();
        assert_eq!(boundary.push(&frames(960, 0.25), false).written_frames, 960);

        let mut out = [[0.0f32, 0.0f32]; LIVE_PCM_CONSUMER_FRAMES];
        let _ = boundary.consume(&mut out);
        assert_eq!(boundary.fill_frames(), 480);

        assert_eq!(boundary.ring.discard(400), 400);
        let underflow = boundary.consume(&mut out);
        assert_eq!(underflow.read_frames, 80);
        assert!(out[..80].iter().all(|frame| *frame == [0.25, -0.25]));
        assert!(out[80..].iter().all(|frame| *frame == [0.0, 0.0]));
    }
}
