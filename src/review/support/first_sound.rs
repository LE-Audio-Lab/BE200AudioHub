//! Review-only current product First Sound data-plane helper.
//!
//! This helper owns no Bluetooth setup/teardown. It consumes a transport-neutral
//! StreamBinding already confirmed by L1-Control and exercises the current
//! L1-Media LC3/clock/bridge -> logical HCI ISO path.

use std::{
    thread,
    time::{Duration, Instant},
};

use crate::{
    plane::{
        l1_control::interface::stream::StreamBinding,
        l1_hci::host::stack::HciStack,
        l1_media::{
            bridge::{
                runtime::{BridgeTxStep, LogicalIsoTxBridge},
                scheduler::{FrameInsertOutcome, StereoFallback},
            },
            codec::lc3::{encode_lc3_silence, prepare_synthetic_lc3},
            source::synthetic::SyntheticToneConfig,
        },
    },
};

pub const PRODUCT_BRIDGE_START_LEAD_US: u64 = 20_000;

pub fn submit_synthetic_lc3(
    stack: &HciStack,
    binding: StreamBinding,
    frame_count: usize,
) -> Result<(), String> {
    let frame_count_u64 = u64::try_from(frame_count)
        .map_err(|_| "frame count does not fit u64".to_string())?;

    let frames = prepare_synthetic_lc3(
        binding,
        SyntheticToneConfig {
            frame_count: frame_count_u64,
            ..SyntheticToneConfig::default()
        },
    )
    .map_err(|error| format!("LC3 synthetic preparation: {error:?}"))?;

    let (silence_left, silence_right) =
        encode_lc3_silence(binding.format)
            .map_err(|error| {
                format!("LC3 silence preparation: {error:?}")
            })?;

    let origin = Instant::now();
    let mut bridge = LogicalIsoTxBridge::new(
        binding,
        PRODUCT_BRIDGE_START_LEAD_US,
        frame_count,
        StereoFallback {
            left: silence_left,
            right: silence_right,
        },
        stack.logical_iso_tx(),
    )
    .map_err(|error| format!("logical ISO bridge init: {error:?}"))?;

    for frame in frames {
        match bridge.push_encoded(frame) {
            FrameInsertOutcome::Accepted => {}
            other => {
                return Err(format!(
                    "logical ISO bridge preload rejected frame: {other:?}"
                ))
            }
        }
    }

    let mut submitted = 0usize;
    while submitted < frame_count {
        let now_us = u64::try_from(origin.elapsed().as_micros())
            .unwrap_or(u64::MAX);

        match bridge
            .on_wake(now_us)
            .map_err(|error| {
                format!("logical ISO bridge wake: {error:?}")
            })?
        {
            BridgeTxStep::Wait { deadline_us, .. } => {
                if deadline_us > now_us {
                    thread::sleep(Duration::from_micros(
                        deadline_us - now_us,
                    ));
                }
            }
            BridgeTxStep::Submitted {
                frame_id,
                skipped_intervals,
                stale_frames_dropped,
                ..
            } => {
                if skipped_intervals != 0
                    || stale_frames_dropped != 0
                {
                    return Err(format!(
                        "logical ISO scheduler missed absolute grid at frame {}: skipped={} stale_dropped={}",
                        frame_id.0,
                        skipped_intervals,
                        stale_frames_dropped,
                    ));
                }
                submitted += 1;
            }
            BridgeTxStep::DroppedBackpressure {
                frame_id,
                ..
            } => {
                return Err(format!(
                    "logical HCI ISO lane backpressured frame {}; product bridge dropped the interval",
                    frame_id.0,
                ))
            }
        }
    }

    Ok(())
}
