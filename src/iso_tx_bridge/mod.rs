pub mod runtime;

//! Initial interval-group scheduler for the Media -> HCI boundary.
//!
//! This core is deterministic and I/O-free. MediaRt owns it alongside the
//! encoded frame store. It follows LocalStreamClock's absolute grid and never
//! waits for HCI/controller progress.

use crate::{
    architecture::{
        control::StreamBinding,
        media::EncodedStereoFrame,
        types::{AudioSide, FrameId},
    },
    hci_host::iso_tx::LogicalIsoSduGroupRequest,
    stream_clock::{LocalStreamClock, StreamClockError},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StereoFallback<P> {
    pub left: P,
    pub right: P,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameInsertOutcome {
    Accepted,
    DroppedStale,
    Duplicate,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntervalGroupSource {
    Encoded,
    Fallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledIntervalGroup<P> {
    pub frame_id: FrameId,
    pub target_time_us: u64,
    pub left: P,
    pub right: P,
    pub source: IntervalGroupSource,
    /// Logical intervals skipped because the scheduler callback arrived after
    /// one or more absolute grid boundaries.
    pub skipped_intervals: u64,
    /// Buffered frames discarded because their absolute target had expired.
    pub stale_frames_dropped: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerStep<P> {
    Wait {
        frame_id: FrameId,
        deadline_us: u64,
    },
    Submit(ScheduledIntervalGroup<P>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulerError {
    ZeroCapacity,
    FrameIdOverflow,
    Clock(StreamClockError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalGroupError {
    PayloadLength {
        side: AudioSide,
        expected: usize,
        actual: usize,
    },
}

pub struct IntervalGroupScheduler<P> {
    binding: StreamBinding,
    clock: LocalStreamClock,
    next_frame: FrameId,
    slots: Vec<Option<EncodedStereoFrame<P>>>,
    fallback: StereoFallback<P>,
}

impl<P> IntervalGroupScheduler<P> {
    pub fn new(
        binding: StreamBinding,
        epoch_us: u64,
        capacity: usize,
        fallback: StereoFallback<P>,
    ) -> Result<Self, SchedulerError> {
        if capacity == 0 {
            return Err(SchedulerError::ZeroCapacity);
        }
        let clock = LocalStreamClock::from_binding(
            binding,
            epoch_us,
        )
        .map_err(SchedulerError::Clock)?;

        let mut slots = Vec::with_capacity(capacity);
        slots.resize_with(capacity, || None);

        Ok(Self {
            binding,
            clock,
            next_frame: FrameId(0),
            slots,
            fallback,
        })
    }

    pub const fn binding(&self) -> StreamBinding {
        self.binding
    }

    pub const fn clock(&self) -> LocalStreamClock {
        self.clock
    }

    pub const fn next_frame(&self) -> FrameId {
        self.next_frame
    }

    pub fn stored_len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    pub fn push_encoded(
        &mut self,
        frame: EncodedStereoFrame<P>,
    ) -> FrameInsertOutcome {
        if frame.frame_id.0 < self.next_frame.0 {
            return FrameInsertOutcome::DroppedStale;
        }
        if self.slots.iter().any(|slot| {
            slot.as_ref()
                .is_some_and(|stored| stored.frame_id == frame.frame_id)
        }) {
            return FrameInsertOutcome::Duplicate;
        }

        let Some(slot) =
            self.slots.iter_mut().find(|slot| slot.is_none())
        else {
            return FrameInsertOutcome::Full;
        };
        *slot = Some(frame);
        FrameInsertOutcome::Accepted
    }

    pub fn next_deadline_us(
        &self,
    ) -> Result<u64, SchedulerError> {
        self.clock
            .target_time_us(self.next_frame)
            .map_err(SchedulerError::Clock)
    }

    fn drop_before(&mut self, frame_id: FrameId) -> usize {
        let mut dropped = 0;
        for slot in &mut self.slots {
            if slot
                .as_ref()
                .is_some_and(|frame| frame.frame_id.0 < frame_id.0)
            {
                *slot = None;
                dropped += 1;
            }
        }
        dropped
    }

    fn take_frame(
        &mut self,
        frame_id: FrameId,
    ) -> Option<EncodedStereoFrame<P>> {
        for slot in &mut self.slots {
            if slot
                .as_ref()
                .is_some_and(|frame| frame.frame_id == frame_id)
            {
                return slot.take();
            }
        }
        None
    }
}

impl IntervalGroupScheduler<Vec<u8>> {
    /// Convert one scheduled logical interval into the product HCI data-plane
    /// request. FrameId and target_time_us remain Bridge/clock-domain metadata;
    /// HCI Host owns the independent HCI ISO sequence space.
    pub fn into_logical_iso_request(
        &self,
        group: ScheduledIntervalGroup<Vec<u8>>,
    ) -> Result<LogicalIsoSduGroupRequest, LogicalGroupError> {
        let expected = usize::from(
            self.binding.format.octets_per_sdu,
        );

        if group.left.len() != expected {
            return Err(LogicalGroupError::PayloadLength {
                side: AudioSide::Left,
                expected,
                actual: group.left.len(),
            });
        }
        if group.right.len() != expected {
            return Err(LogicalGroupError::PayloadLength {
                side: AudioSide::Right,
                expected,
                actual: group.right.len(),
            });
        }

        Ok(LogicalIsoSduGroupRequest {
            session_id: self.binding.session_id,
            stream_id: self.binding.stream_id,
            left: group.left,
            right: group.right,
        })
    }
}

impl<P: Clone> IntervalGroupScheduler<P> {
    pub fn on_wake(
        &mut self,
        now_us: u64,
    ) -> Result<SchedulerStep<P>, SchedulerError> {
        let Some(current) = self.clock.current_frame_at(now_us) else {
            return Ok(SchedulerStep::Wait {
                frame_id: self.next_frame,
                deadline_us: self.next_deadline_us()?,
            });
        };

        if current.0 < self.next_frame.0 {
            return Ok(SchedulerStep::Wait {
                frame_id: self.next_frame,
                deadline_us: self.next_deadline_us()?,
            });
        }

        let target = current;
        let skipped_intervals =
            target.0.saturating_sub(self.next_frame.0);
        let stale_frames_dropped = self.drop_before(target);
        let target_time_us = self
            .clock
            .target_time_us(target)
            .map_err(SchedulerError::Clock)?;

        let (left, right, source) =
            if let Some(frame) = self.take_frame(target) {
                (
                    frame.left,
                    frame.right,
                    IntervalGroupSource::Encoded,
                )
            } else {
                (
                    self.fallback.left.clone(),
                    self.fallback.right.clone(),
                    IntervalGroupSource::Fallback,
                )
            };

        self.next_frame = FrameId(
            target
                .0
                .checked_add(1)
                .ok_or(SchedulerError::FrameIdOverflow)?,
        );

        Ok(SchedulerStep::Submit(ScheduledIntervalGroup {
            frame_id: target,
            target_time_us,
            left,
            right,
            source,
            skipped_intervals,
            stale_frames_dropped,
        }))
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
        media::{
            lc3::{
                encode_lc3_silence,
                prepare_synthetic_lc3,
            },
            synthetic::SyntheticToneConfig,
        },
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

    fn frame(id: u64, left: u8, right: u8) -> EncodedStereoFrame<Vec<u8>> {
        EncodedStereoFrame {
            frame_id: FrameId(id),
            left: vec![left; 100],
            right: vec![right; 100],
        }
    }

    fn scheduler(capacity: usize) -> IntervalGroupScheduler<Vec<u8>> {
        IntervalGroupScheduler::new(
            binding(),
            EPOCH_US,
            capacity,
            StereoFallback {
                left: vec![0xFA; 100],
                right: vec![0xFB; 100],
            },
        )
        .unwrap()
    }

    #[test]
    fn synthetic_lc3_to_bridge_to_logical_iso_request_is_end_to_end() {
        let binding = binding();
        let config = SyntheticToneConfig {
            frame_count: 3,
            fade_samples: 0,
            ..SyntheticToneConfig::default()
        };
        let mut frames =
            prepare_synthetic_lc3(binding, config).unwrap();
        assert_eq!(frames.len(), 3);

        let (silence_left, silence_right) =
            encode_lc3_silence(binding.format).unwrap();
        let fallback = StereoFallback {
            left: silence_left.clone(),
            right: silence_right.clone(),
        };
        let mut scheduler = IntervalGroupScheduler::new(
            binding,
            EPOCH_US,
            3,
            fallback,
        )
        .unwrap();

        // Feed frame 0 and frame 2; frame 1 is intentionally absent.
        assert_eq!(
            scheduler.push_encoded(frames.remove(0)),
            FrameInsertOutcome::Accepted
        );
        let frame_two = frames.remove(1);
        assert_eq!(frame_two.frame_id, FrameId(2));
        assert_eq!(
            scheduler.push_encoded(frame_two),
            FrameInsertOutcome::Accepted
        );

        let SchedulerStep::Submit(first) =
            scheduler.on_wake(EPOCH_US).unwrap()
        else {
            panic!("frame 0 must submit");
        };
        assert_eq!(first.frame_id, FrameId(0));
        assert_eq!(first.source, IntervalGroupSource::Encoded);
        let first_request = scheduler
            .into_logical_iso_request(first)
            .unwrap();
        assert_eq!(first_request.session_id, SessionId(7));
        assert_eq!(first_request.stream_id, StreamId(11));
        assert_eq!(first_request.left.len(), 100);
        assert_eq!(first_request.right.len(), 100);
        assert_ne!(first_request.left, first_request.right);

        let SchedulerStep::Submit(second) =
            scheduler.on_wake(EPOCH_US + 10_000).unwrap()
        else {
            panic!("missing frame 1 must use fallback");
        };
        assert_eq!(second.frame_id, FrameId(1));
        assert_eq!(second.source, IntervalGroupSource::Fallback);
        let second_request = scheduler
            .into_logical_iso_request(second)
            .unwrap();
        assert_eq!(second_request.left, silence_left);
        assert_eq!(second_request.right, silence_right);

        let SchedulerStep::Submit(third) =
            scheduler.on_wake(EPOCH_US + 20_000).unwrap()
        else {
            panic!("frame 2 must remain scheduled on absolute grid");
        };
        assert_eq!(third.frame_id, FrameId(2));
        assert_eq!(third.source, IntervalGroupSource::Encoded);
        let third_request = scheduler
            .into_logical_iso_request(third)
            .unwrap();
        assert_eq!(third_request.left.len(), 100);
        assert_eq!(third_request.right.len(), 100);
    }

    #[test]
    fn scheduled_group_maps_to_handle_free_logical_hci_request() {
        let scheduler = scheduler(2);
        let group = ScheduledIntervalGroup {
            frame_id: FrameId(42),
            target_time_us: EPOCH_US + 420_000,
            left: vec![0x11; 100],
            right: vec![0x22; 100],
            source: IntervalGroupSource::Encoded,
            skipped_intervals: 0,
            stale_frames_dropped: 0,
        };

        let request =
            scheduler.into_logical_iso_request(group).unwrap();

        assert_eq!(request.session_id, SessionId(7));
        assert_eq!(request.stream_id, StreamId(11));
        assert_eq!(request.left, vec![0x11; 100]);
        assert_eq!(request.right, vec![0x22; 100]);
    }

    #[test]
    fn logical_request_rejects_payload_outside_binding_format() {
        let scheduler = scheduler(2);
        let group = ScheduledIntervalGroup {
            frame_id: FrameId(0),
            target_time_us: EPOCH_US,
            left: vec![0; 99],
            right: vec![0; 100],
            source: IntervalGroupSource::Encoded,
            skipped_intervals: 0,
            stale_frames_dropped: 0,
        };

        assert_eq!(
            scheduler.into_logical_iso_request(group),
            Err(LogicalGroupError::PayloadLength {
                side: AudioSide::Left,
                expected: 100,
                actual: 99,
            })
        );
    }

    #[test]
    fn scheduler_uses_absolute_grid_and_exact_frame_ids() {
        let mut scheduler = scheduler(4);
        assert_eq!(
            scheduler.push_encoded(frame(0, 0x10, 0x20)),
            FrameInsertOutcome::Accepted
        );
        assert_eq!(
            scheduler.on_wake(EPOCH_US - 321).unwrap(),
            SchedulerStep::Wait {
                frame_id: FrameId(0),
                deadline_us: EPOCH_US,
            }
        );

        let SchedulerStep::Submit(group) =
            scheduler.on_wake(EPOCH_US + 777).unwrap()
        else {
            panic!("frame 0 must submit in its absolute interval");
        };
        assert_eq!(group.frame_id, FrameId(0));
        assert_eq!(group.target_time_us, EPOCH_US);
        assert_eq!(group.left, vec![0x10; 100]);
        assert_eq!(group.right, vec![0x20; 100]);
        assert_eq!(group.source, IntervalGroupSource::Encoded);
        assert_eq!(group.skipped_intervals, 0);
        assert_eq!(scheduler.next_frame(), FrameId(1));

        // Callback jitter does not move frame 1's absolute deadline.
        assert_eq!(
            scheduler.next_deadline_us().unwrap(),
            EPOCH_US + 10_000
        );
    }

    #[test]
    fn missing_frame_uses_complete_stereo_fallback_group() {
        let mut scheduler = scheduler(2);

        let SchedulerStep::Submit(group) =
            scheduler.on_wake(EPOCH_US).unwrap()
        else {
            panic!("missing frame must still submit fallback");
        };
        assert_eq!(group.frame_id, FrameId(0));
        assert_eq!(group.left, vec![0xFA; 100]);
        assert_eq!(group.right, vec![0xFB; 100]);
        assert_eq!(group.source, IntervalGroupSource::Fallback);
    }

    #[test]
    fn late_wake_drops_stale_buffer_and_jumps_to_current_grid_frame() {
        let mut scheduler = scheduler(5);
        for id in 0..=3 {
            assert_eq!(
                scheduler.push_encoded(frame(
                    id,
                    0x10 + id as u8,
                    0x20 + id as u8,
                )),
                FrameInsertOutcome::Accepted
            );
        }

        let SchedulerStep::Submit(group) =
            scheduler.on_wake(EPOCH_US + 30_123).unwrap()
        else {
            panic!("late wake must submit current frame");
        };

        assert_eq!(group.frame_id, FrameId(3));
        assert_eq!(group.target_time_us, EPOCH_US + 30_000);
        assert_eq!(group.skipped_intervals, 3);
        assert_eq!(group.stale_frames_dropped, 3);
        assert_eq!(group.source, IntervalGroupSource::Encoded);
        assert_eq!(group.left, vec![0x13; 100]);
        assert_eq!(scheduler.stored_len(), 0);
        assert_eq!(scheduler.next_frame(), FrameId(4));
    }

    #[test]
    fn encoded_store_is_bounded_duplicate_safe_and_rejects_old_frames() {
        let mut scheduler = scheduler(2);

        assert_eq!(
            scheduler.push_encoded(frame(0, 1, 2)),
            FrameInsertOutcome::Accepted
        );
        assert_eq!(
            scheduler.push_encoded(frame(0, 3, 4)),
            FrameInsertOutcome::Duplicate
        );
        assert_eq!(
            scheduler.push_encoded(frame(1, 5, 6)),
            FrameInsertOutcome::Accepted
        );
        assert_eq!(
            scheduler.push_encoded(frame(2, 7, 8)),
            FrameInsertOutcome::Full
        );

        let _ = scheduler.on_wake(EPOCH_US).unwrap();
        assert_eq!(
            scheduler.push_encoded(frame(0, 9, 10)),
            FrameInsertOutcome::DroppedStale
        );
    }
}
