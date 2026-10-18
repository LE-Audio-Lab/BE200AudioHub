//! Initial interval-group scheduler for the Media -> HCI boundary.
//!
//! This core is deterministic and I/O-free. MediaRt owns it alongside the
//! encoded frame store. It follows LocalStreamClock's absolute grid and never
//! waits for HCI/controller progress.

use crate::{
    architecture::{
        control::StreamBinding,
        media::EncodedStereoFrame,
        types::FrameId,
    },
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
