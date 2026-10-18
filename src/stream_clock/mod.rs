//! Session-scoped authoritative media timeline.
//!
//! The first concrete implementation is deliberately a pure local-monotonic
//! grid. It owns no OS timer and has no transport/completion input. Callers
//! supply monotonic timestamps for queries; those queries never mutate the
//! epoch or period.

use crate::architecture::{
    control::StreamBinding,
    stream_clock::{ClockLock, StreamClockSnapshot},
    types::{FrameId, SessionId, StreamId},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamClockError {
    ZeroPeriod,
    BeforeEpoch {
        now_us: u64,
        epoch_us: u64,
    },
    TargetTimeOverflow {
        frame_id: FrameId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameTiming {
    /// Target is in the future. Before the epoch, FrameId(0) is zero frames
    /// ahead of the first grid position.
    Early {
        frames_ahead: u64,
        lead_us: u64,
    },
    /// Frame belongs to the current absolute grid interval.
    Current {
        elapsed_us: u64,
        until_next_us: u64,
    },
    /// At least one later grid interval has already begun.
    Stale {
        frames_behind: u64,
        late_us: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalStreamClock {
    session_id: SessionId,
    stream_id: StreamId,
    epoch_us: u64,
    period_us: u32,
}

impl LocalStreamClock {
    pub fn from_binding(
        binding: StreamBinding,
        epoch_us: u64,
    ) -> Result<Self, StreamClockError> {
        let period_us = binding.format.frame_duration_us;
        if period_us == 0 {
            return Err(StreamClockError::ZeroPeriod);
        }

        Ok(Self {
            session_id: binding.session_id,
            stream_id: binding.stream_id,
            epoch_us,
            period_us,
        })
    }

    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub const fn stream_id(&self) -> StreamId {
        self.stream_id
    }

    pub const fn epoch_us(&self) -> u64 {
        self.epoch_us
    }

    pub const fn period_us(&self) -> u32 {
        self.period_us
    }

    pub fn target_time_us(
        &self,
        frame_id: FrameId,
    ) -> Result<u64, StreamClockError> {
        let offset = frame_id
            .0
            .checked_mul(u64::from(self.period_us))
            .ok_or(StreamClockError::TargetTimeOverflow {
                frame_id,
            })?;
        self.epoch_us
            .checked_add(offset)
            .ok_or(StreamClockError::TargetTimeOverflow {
                frame_id,
            })
    }

    pub fn current_frame_at(
        &self,
        now_us: u64,
    ) -> Option<FrameId> {
        let elapsed = now_us.checked_sub(self.epoch_us)?;
        Some(FrameId(
            elapsed / u64::from(self.period_us),
        ))
    }

    pub fn classify(
        &self,
        frame_id: FrameId,
        now_us: u64,
    ) -> Result<FrameTiming, StreamClockError> {
        let target_us = self.target_time_us(frame_id)?;

        let Some(current) = self.current_frame_at(now_us) else {
            return Ok(FrameTiming::Early {
                frames_ahead: frame_id.0,
                lead_us: target_us - now_us,
            });
        };

        if frame_id.0 > current.0 {
            return Ok(FrameTiming::Early {
                frames_ahead: frame_id.0 - current.0,
                lead_us: target_us - now_us,
            });
        }

        if frame_id == current {
            let elapsed_us = now_us - target_us;
            return Ok(FrameTiming::Current {
                elapsed_us,
                until_next_us:
                    u64::from(self.period_us) - elapsed_us,
            });
        }

        Ok(FrameTiming::Stale {
            frames_behind: current.0 - frame_id.0,
            late_us: now_us - target_us,
        })
    }

    pub fn snapshot(
        &self,
        now_us: u64,
    ) -> Result<StreamClockSnapshot, StreamClockError> {
        let current_frame = self.current_frame_at(now_us).ok_or(
            StreamClockError::BeforeEpoch {
                now_us,
                epoch_us: self.epoch_us,
            },
        )?;

        Ok(StreamClockSnapshot {
            period_us: self.period_us,
            current_frame,
            lock: ClockLock::LocalMonotonic,
            rate_error_ppm: 0,
        })
    }
}
