//! Session-scoped authoritative media timeline.
//!
//! The first concrete implementation is deliberately a pure local-monotonic
//! grid. It owns no OS timer and has no transport/completion input. Callers
//! supply monotonic timestamps for queries; those queries never mutate the
//! epoch or period.

use crate::{
    architecture::{
        stream_clock::{ClockLock, StreamClockSnapshot},
        types::{SessionId, StreamId},
    },
    plane::l1_control::interface::stream::StreamBinding,
    plane::l1_media::clock::frame::FrameId,
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


#[cfg(test)]
mod tests {
    use crate::architecture::{
        control::{
            LeAudioFormat, StreamBinding, StreamTopology,
        },
        types::{SessionId, StreamId},
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

    #[test]
    fn binding_establishes_one_absolute_ten_ms_grid() {
        let clock =
            LocalStreamClock::from_binding(binding(), EPOCH_US)
                .unwrap();

        assert_eq!(clock.session_id(), SessionId(7));
        assert_eq!(clock.stream_id(), StreamId(11));
        assert_eq!(clock.epoch_us(), EPOCH_US);
        assert_eq!(clock.period_us(), 10_000);

        assert_eq!(
            clock.target_time_us(FrameId(0)).unwrap(),
            EPOCH_US
        );
        assert_eq!(
            clock.target_time_us(FrameId(1)).unwrap(),
            EPOCH_US + 10_000
        );
        assert_eq!(
            clock.target_time_us(FrameId(100)).unwrap(),
            EPOCH_US + 1_000_000
        );
    }

    #[test]
    fn timer_callback_jitter_does_not_move_logical_grid() {
        let clock =
            LocalStreamClock::from_binding(binding(), EPOCH_US)
                .unwrap();

        assert_eq!(
            clock
                .classify(FrameId(3), EPOCH_US + 30_007)
                .unwrap(),
            FrameTiming::Current {
                elapsed_us: 7,
                until_next_us: 9_993,
            }
        );
        assert_eq!(
            clock
                .classify(FrameId(4), EPOCH_US + 30_007)
                .unwrap(),
            FrameTiming::Early {
                frames_ahead: 1,
                lead_us: 9_993,
            }
        );

        // The next target remains epoch + N*period. Callback jitter is an
        // observation only; there is no "last callback + period" state.
        assert_eq!(
            clock.target_time_us(FrameId(4)).unwrap(),
            EPOCH_US + 40_000
        );
    }

    #[test]
    fn large_time_jump_marks_old_frames_stale_without_stretching_period() {
        let clock =
            LocalStreamClock::from_binding(binding(), EPOCH_US)
                .unwrap();

        assert_eq!(
            clock.current_frame_at(EPOCH_US + 55_000),
            Some(FrameId(5))
        );
        assert_eq!(
            clock
                .classify(FrameId(2), EPOCH_US + 55_000)
                .unwrap(),
            FrameTiming::Stale {
                frames_behind: 3,
                late_us: 35_000,
            }
        );
        assert_eq!(
            clock.target_time_us(FrameId(6)).unwrap(),
            EPOCH_US + 60_000
        );
    }

    #[test]
    fn before_epoch_is_early_and_snapshot_is_not_running_yet() {
        let clock =
            LocalStreamClock::from_binding(binding(), EPOCH_US)
                .unwrap();

        assert_eq!(
            clock.classify(FrameId(0), EPOCH_US - 250).unwrap(),
            FrameTiming::Early {
                frames_ahead: 0,
                lead_us: 250,
            }
        );
        assert_eq!(
            clock.snapshot(EPOCH_US - 1),
            Err(StreamClockError::BeforeEpoch {
                now_us: EPOCH_US - 1,
                epoch_us: EPOCH_US,
            })
        );
    }

    #[test]
    fn snapshot_projects_local_monotonic_lock_without_rate_error() {
        let clock =
            LocalStreamClock::from_binding(binding(), EPOCH_US)
                .unwrap();

        assert_eq!(
            clock.snapshot(EPOCH_US + 29_999).unwrap(),
            StreamClockSnapshot {
                period_us: 10_000,
                current_frame: FrameId(2),
                lock: ClockLock::LocalMonotonic,
                rate_error_ppm: 0,
            }
        );
    }

    #[test]
    fn invalid_period_and_target_overflow_fail_explicitly() {
        let mut invalid = binding();
        invalid.format.frame_duration_us = 0;
        assert_eq!(
            LocalStreamClock::from_binding(invalid, EPOCH_US),
            Err(StreamClockError::ZeroPeriod)
        );

        let clock = LocalStreamClock::from_binding(
            binding(),
            u64::MAX - 5_000,
        )
        .unwrap();
        assert_eq!(
            clock.target_time_us(FrameId(1)),
            Err(StreamClockError::TargetTimeOverflow {
                frame_id: FrameId(1),
            })
        );
    }
}
