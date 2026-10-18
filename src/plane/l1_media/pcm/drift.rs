//! Temporary one-sided positive-drift guard ported from LE-Audio-Relay.
//!
//! This is not clock synchronization. It only keeps the residual producer
//! queue near the retained cushion while a later full timing controller is
//! still absent.

use super::ring::SpscStereoPcmRing;

const GRADUAL_TRIM_EVERY_CONSUMER_TICKS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DriftTrimResult {
    pub gradual_frames: usize,
    pub emergency_frames: usize,
}

#[derive(Debug)]
pub struct ProvisionalPositiveDriftGuard {
    target_frames: usize,
    low_water_frames: usize,
    high_water_frames: usize,
    hard_recenter_frames: usize,
    gradual_trim_active: bool,
    consumer_tick_counter: usize,
}

impl ProvisionalPositiveDriftGuard {
    pub fn new(
        sample_rate_hz: usize,
        capacity_frames: usize,
        target_frames: usize,
    ) -> Result<Self, &'static str> {
        if sample_rate_hz == 0 {
            return Err("sample rate must be nonzero");
        }
        if capacity_frames <= 2 {
            return Err("PCM capacity must exceed two frames");
        }

        let target_frames = target_frames.clamp(1, capacity_frames - 2);
        let one_ms = (sample_rate_hz / 1000).max(1);

        let low_water_frames =
            (target_frames + one_ms).clamp(target_frames + 1, capacity_frames - 2);
        let high_water_frames =
            (target_frames + one_ms * 2).clamp(low_water_frames + 1, capacity_frames - 1);
        let hard_recenter_frames =
            (target_frames + one_ms * 10).clamp(high_water_frames + 1, capacity_frames);

        Ok(Self {
            target_frames,
            low_water_frames,
            high_water_frames,
            hard_recenter_frames,
            gradual_trim_active: false,
            consumer_tick_counter: 0,
        })
    }

    pub fn silent_frames_to_suppress(
        &self,
        incoming_frames: usize,
        current_fill_frames: usize,
    ) -> usize {
        if incoming_frames == 0 || current_fill_frames <= self.target_frames {
            return 0;
        }

        incoming_frames.min(current_fill_frames - self.target_frames)
    }

    pub fn apply_after_consume(
        &mut self,
        ring: &SpscStereoPcmRing,
    ) -> DriftTrimResult {
        let residual_fill = ring.fill_frames();

        if residual_fill >= self.hard_recenter_frames {
            let trimmed =
                ring.discard(residual_fill.saturating_sub(self.target_frames));
            self.gradual_trim_active = false;
            self.consumer_tick_counter = 0;
            return DriftTrimResult {
                gradual_frames: 0,
                emergency_frames: trimmed,
            };
        }

        if !self.gradual_trim_active
            && residual_fill >= self.high_water_frames
        {
            self.gradual_trim_active = true;
            self.consumer_tick_counter = 0;
        }

        if !self.gradual_trim_active {
            return DriftTrimResult::default();
        }

        if residual_fill <= self.low_water_frames {
            self.gradual_trim_active = false;
            self.consumer_tick_counter = 0;
            return DriftTrimResult::default();
        }

        self.consumer_tick_counter += 1;
        if self.consumer_tick_counter < GRADUAL_TRIM_EVERY_CONSUMER_TICKS {
            return DriftTrimResult::default();
        }

        self.consumer_tick_counter = 0;
        DriftTrimResult {
            gradual_frames: ring.discard(1),
            emergency_frames: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_suppression_only_removes_accumulated_excess() {
        let guard =
            ProvisionalPositiveDriftGuard::new(48_000, 3_840, 480).unwrap();

        assert_eq!(guard.silent_frames_to_suppress(480, 480), 0);
        assert_eq!(guard.silent_frames_to_suppress(480, 600), 120);
        assert_eq!(guard.silent_frames_to_suppress(100, 900), 100);
    }

    #[test]
    fn emergency_recenter_returns_to_ten_ms_target() {
        let ring = SpscStereoPcmRing::new(3_840).unwrap();
        assert_eq!(ring.write_silence(1_100, 3_840), 1_100);

        let mut guard =
            ProvisionalPositiveDriftGuard::new(48_000, 3_840, 480).unwrap();
        let result = guard.apply_after_consume(&ring);

        assert_eq!(result.gradual_frames, 0);
        assert_eq!(result.emergency_frames, 620);
        assert_eq!(ring.fill_frames(), 480);
    }
}
