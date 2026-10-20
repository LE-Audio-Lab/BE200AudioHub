//! Windows Process Loopback source boundary for L1-Media.
//!
//! Raw COM/WASAPI lives in L3 Platform. This owner only connects copied
//! Float32 stereo packets to the PCM producer and records source-local facts.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use crate::{
    platform::windows::process_loopback::{
        ProcessLoopbackCapture, ProcessLoopbackCaptureStats,
        ProcessLoopbackError,
    },
    plane::l1_media::pcm::relay_boundary::{
        PcmRelayProducer, PcmPushResult,
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessLoopbackSourceStats {
    pub captured_frames: u64,
    pub written_frames: u64,
    pub dropped_frames: u64,
    pub intentional_silent_trim_frames: u64,
    pub discontinuities: u64,
    pub last_device_position: u64,
    pub last_qpc_position: u64,
}

#[derive(Default)]
struct SourceCounters {
    captured_frames: AtomicU64,
    written_frames: AtomicU64,
    dropped_frames: AtomicU64,
    intentional_silent_trim_frames: AtomicU64,
    discontinuities: AtomicU64,
    last_device_position: AtomicU64,
    last_qpc_position: AtomicU64,
}

impl SourceCounters {
    fn record(
        &self,
        push: PcmPushResult,
        discontinuity: bool,
        device_position: u64,
        qpc_position: u64,
    ) {
        self.captured_frames.fetch_add(
            push.input_frames as u64,
            Ordering::Relaxed,
        );
        self.written_frames.fetch_add(
            push.written_frames as u64,
            Ordering::Relaxed,
        );
        self.dropped_frames.fetch_add(
            push.dropped_frames as u64,
            Ordering::Relaxed,
        );
        self.intentional_silent_trim_frames.fetch_add(
            push.intentional_silent_trim_frames as u64,
            Ordering::Relaxed,
        );
        if discontinuity {
            self.discontinuities.fetch_add(1, Ordering::Relaxed);
        }
        self.last_device_position
            .store(device_position, Ordering::Relaxed);
        self.last_qpc_position.store(qpc_position, Ordering::Relaxed);
    }

    fn snapshot(&self) -> ProcessLoopbackSourceStats {
        ProcessLoopbackSourceStats {
            captured_frames: self.captured_frames.load(Ordering::Relaxed),
            written_frames: self.written_frames.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            intentional_silent_trim_frames: self
                .intentional_silent_trim_frames
                .load(Ordering::Relaxed),
            discontinuities: self.discontinuities.load(Ordering::Relaxed),
            last_device_position: self
                .last_device_position
                .load(Ordering::Relaxed),
            last_qpc_position: self
                .last_qpc_position
                .load(Ordering::Relaxed),
        }
    }
}

pub struct ProcessLoopbackSource {
    capture: Option<ProcessLoopbackCapture>,
    counters: Arc<SourceCounters>,
}

impl ProcessLoopbackSource {
    pub fn start_excluding_current_process_tree(
        producer: PcmRelayProducer,
    ) -> Result<Self, ProcessLoopbackError> {
        let counters = Arc::new(SourceCounters::default());
        let callback_counters = Arc::clone(&counters);

        let capture = ProcessLoopbackCapture::start_excluding_process_tree(
            std::process::id(),
            move |packet| {
                let result =
                    producer.push(packet.frames, packet.silent);

                if result.dropped_frames > 0 || packet.discontinuity {
                    println!(
                        "[live][capture] loss/offset dropped_frames={} discontinuity={} device_position={} qpc_position={}",
                        result.dropped_frames,
                        packet.discontinuity,
                        packet.device_position,
                        packet.qpc_position,
                    );
                }

                callback_counters.record(
                    result,
                    packet.discontinuity,
                    packet.device_position,
                    packet.qpc_position,
                );
            },
        )?;

        Ok(Self {
            capture: Some(capture),
            counters,
        })
    }

    pub fn stats(&self) -> ProcessLoopbackSourceStats {
        self.counters.snapshot()
    }

    pub fn shutdown(
        mut self,
    ) -> Result<
        (ProcessLoopbackSourceStats, ProcessLoopbackCaptureStats),
        ProcessLoopbackError,
    > {
        let capture = self.capture.take().ok_or(
            ProcessLoopbackError::StartupChannelClosed,
        )?;
        let platform = capture.shutdown()?;
        Ok((self.counters.snapshot(), platform))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_stats_default_to_zero() {
        assert_eq!(
            SourceCounters::default().snapshot(),
            ProcessLoopbackSourceStats::default()
        );
    }

    #[test]
    fn source_counters_keep_capture_and_ring_outcomes_separate() {
        let counters = SourceCounters::default();
        counters.record(
            PcmPushResult {
                input_frames: 480,
                written_frames: 470,
                intentional_silent_trim_frames: 5,
                dropped_frames: 5,
            },
            true,
            100,
            200,
        );

        assert_eq!(
            counters.snapshot(),
            ProcessLoopbackSourceStats {
                captured_frames: 480,
                written_frames: 470,
                dropped_frames: 5,
                intentional_silent_trim_frames: 5,
                discontinuities: 1,
                last_device_position: 100,
                last_qpc_position: 200,
            }
        );
    }
}
