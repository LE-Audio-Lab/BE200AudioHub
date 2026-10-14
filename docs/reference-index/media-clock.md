# Media Worker / StreamClock / ASRC Reference Index

## BE200AudioHub contract

Media owns the source/media clock domain:

```text
WASAPI source
→ PCM ring
→ clock estimator / ASRC
→ channel mapping
→ LC3
→ EncodedStereoFrame(FrameId)
```

Media does not know CIS handles, HCI credits, USB endpoints or HCI sequence.

A session-scoped `StreamClock` defines one authoritative target grid. Initial target is local monotonic/QPC; future mode may be controller-anchored.

ASRC is an adapter to the target clock, **not** the clock itself.

## Primary media-worker oracle — AOSP

Project: AOSP Bluetooth

File: `system/bta/le_audio/audio_hal_client/audio_source_hal_client.cc`

### `SourceImpl::SendAudioData()`

Read for:

- bytes/frames per media interval;
- source read;
- underflow handling;
- ASRC invocation before handing audio downstream.

Adopt:

- source read and ASRC belong in the media path;
- one media interval is configured explicitly.

### `SourceImpl::InitAudioSinkThread()`

Reference for a dedicated real-time media execution context.

### `SourceImpl::StartAudioTicks()`

Reference for:

- periodic scheduling at configured data interval;
- worker-context timer;
- ASRC state prepared before ticks.

### `SourceImpl::StopAudioTicks()`

Reference for cancelling ticks before destroying clock/adaptation state.

### `SourceImpl::Start()`

Reference for preparing HAL/session/config without immediately producing periodic media.

### `SourceImpl::ConfirmStreamingRequest()`

Reference for starting ticks only after the Bluetooth stream is semantically ready.

### `SourceImpl::Stop()`

Reference for stopping periodic production through the owning worker before session release.

## Controller-anchor / correction oracle — nRF Audio

Project: nRF Connect SDK Audio

File: `applications/nrf_audio/src/bluetooth/bt_stream/bt_le_audio_tx/bt_le_audio_tx.c`

### `controller_ts_tx_get()`

Reads controller transmit synchronization timestamp.

Use for future controller-anchored `StreamClock` mode.

### `tx_call_interval_check()`

Detects source-call gaps and invalidates stale controller timestamp estimation.

Reference for:

- estimator lock should be explicitly lost after severe source discontinuity.

### `tx_streams_send()`

Sends all streams for the logical interval in one call.

Cross-reference with IsoTxBridge group semantics.

### `tx_controller_sync_and_correct()`

Primary reference for classifying timing error against controller grid and deciding correction.

Use conceptually:

- small error → continuous correction/estimator;
- very large error → discrete correction/re-anchor.

### `bt_le_audio_tx_send()`

Read for source timing pathology handling:

- source too slow → interval skip / empty SDU behavior;
- source too fast → flush/drop current data;
- streams sent as a group;
- controller alignment corrected after send.

Do not copy the exact nRF audio pipeline; copy the separation between normal drift compensation and severe discrete correction.

## Audio datapath cross-check — nRF Audio

File: `applications/nrf_audio/src/audio/audio_datapath.c`

Anchors:

- `err_us_calculate()`
- `audio_datapath_drift_compensation()`
- `audio_datapath_presentation_compensation()`
- `audio_datapath_just_in_time_check_and_adjust()`
- `audio_datapath_sdu_ref_update()`
- `audio_datapath_stream_out()`

Read for:

- estimating accumulated timing error;
- continuous drift correction;
- presentation/just-in-time correction;
- updating SDU reference timing separately from codec work.

## Negative/limited precedent — LE-Audio-Relay drift guard

Project: LE-Audio-Relay

File: `Timing/ProvisionalPositiveDriftGuard.cs`

### `GetSilentFramesToSuppress()`

Useful as a discrete inaudible-correction precedent during declared silence.

### `ApplyAfterRender()`

Uses one-frame gradual trims and hard recenter around a ring cushion.

Important source comment:

> This is NOT clock synchronization.

Use only as evidence for:

- discrete correction strategy;
- bounded ring target.

Do not reuse as final ASRC/clock estimator.

## StreamClock deterministic model

Recommended test model:

```text
FrameId N
target_time(N) = epoch + N * interval
```

All stage-specific timing is represented as lead/deadline offsets from this same grid.

Tests:

1. target times remain monotonic;
2. FrameId never depends on HCI completion count;
3. local monotonic mode tolerates timer callback jitter without changing the logical grid;
4. controller-anchor mode estimates phase offset without redefining FrameId;
5. lock loss/re-anchor is explicit;
6. source discontinuity cannot silently accumulate arbitrary ring latency.

## ASRC estimator tests

Use synthetic source clocks:

- exact 48 kHz;
- +10/+50/+100 ppm;
- -10/-50/-100 ppm;
- initial phase offset;
- slowly changing drift;
- sudden 20–100 ms source discontinuity;
- long gap/restart.

Properties:

- small drift converges without unbounded PCM fill;
- correction rate is bounded;
- severe discontinuity requests discrete policy rather than extreme resampling;
- target StreamClock remains unchanged by source drift.

## Media lifecycle tests

State shape may evolve, but preserve:

`Stopped → Prepared → Primed → Running → Draining/Stopped`

Test:

- no FrameId/tick production before Running;
- Prepare allocates/configures codec/ASRC/rings;
- stop invalidates future tick callbacks;
- old-generation ticks cannot produce frames after restart;
- encoder state never shared across sessions.
