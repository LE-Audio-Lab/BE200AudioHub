# LC3 Codec Boundary Reference Index

## BE200AudioHub contract

LC3 belongs to Media Plane.

Media input:

- canonical PCM;
- codec configuration selected by Control.

Media output:

`EncodedStereoFrame { FrameId, left, right }`

Bridge receives opaque encoded payloads. HCI Host receives opaque ISO SDUs.

Neither Bridge nor HCI Host calls LC3.

## Primary oracle — liblc3 public API

Project: liblc3

File: `include/lc3.h`

Implementation counterpart: `src/lc3.c`

### `lc3_frame_samples()`

Reference for deriving PCM samples per LC3 frame from frame duration/sample rate.

Use in configuration validation and buffer sizing.

### `lc3_frame_bytes()`

Reference for valid frame payload byte count / bitrate relation where applicable.

### `lc3_resolve_bitrate()`

Reference for translating payload/frame configuration into bitrate.

Useful for diagnostics/planner validation, not HCI scheduling.

### `lc3_delay_samples()`

Reference for codec algorithmic delay accounting.

Keep codec delay distinct from capture/queue/controller/acoustic latency.

### `lc3_encoder_size()`

Reference for prepare-time encoder state allocation.

### `lc3_setup_encoder()`

Reference for configuring reusable encoder state.

Expected lifecycle:

- allocate/setup during Media Prepare;
- no encoder construction in per-10-ms hot path.

### `lc3_encode()`

Primary encode call.

Test exact:

- PCM sample count;
- input sample format/stride;
- output frame byte count;
- independent left/right encoder state.

### Decoder counterparts

Index corresponding decoder size/setup/decode APIs for offline golden/self-tests.

A decoder is not required in runtime downlink path, but is valuable as a test oracle for encoded frames.

## AOSP placement cross-check

Project: AOSP Bluetooth

File: `audio_source_hal_client.cc`

### `SourceImpl::SendAudioData()`

Media sequence establishes the architectural placement:

`source read → ASRC → downstream audio-ready callback`

LE Audio client later performs channel mapping/encoding before ISO submission.

The important rule for BE200AudioHub:

- clock adaptation happens before LC3;
- encoding follows the authoritative media interval;
- controller credit does not trigger encoding.

## Experiment/R5 HIL evidence

BE200AudioHub Experiment proved a working B3P configuration:

- 48 kHz;
- 10 ms frame duration;
- 100-byte mono LC3 SDU per ear;
- left 440 Hz / right 880 Hz synthetic First Sound decoded acoustically by Buds3 Pro.

Use those exact parameters as a regression fixture until Control planner intentionally selects something else.

## Codec wrapper responsibilities

A product wrapper should own:

- validated codec config;
- encoder state lifetime;
- scratch/output storage;
- PCM channel input contract;
- encoded fallback/silence frame preparation if used;
- error conversion into typed Media fault.

It must not own:

- FrameId clock;
- CIS handle;
- HCI sequence;
- controller credits.

## Deterministic tests

### Configuration

- supported 48 kHz / 10 ms configuration;
- invalid sample rate;
- invalid frame duration;
- output byte count mismatch;
- independent left/right configuration.

### Buffer sizing

For every config:

- `frame_samples` matches PCM interval;
- encoder input span exact;
- encoded output length exact;
- no per-frame state allocation required.

### Golden/sanity audio

Generate deterministic PCM:

- silence;
- impulse;
- constant DC-small amplitude;
- 440 Hz;
- 880 Hz;
- stereo with distinct L/R content.

Checks:

- encode succeeds;
- output fixed expected size;
- decode with liblc3 test decoder produces expected sample count;
- decoded signal correlation/frequency sanity remains within chosen numerical tolerance;
- left/right encoders cannot cross-contaminate state.

### Lifecycle

- Prepare creates encoder state;
- Running reuses state;
- Stop drops/reset state;
- new generation starts from clean encoder history as policy requires.

## Fallback frame policy

If Bridge needs a missed-frame fallback:

- Media prepares a valid encoded LC3 fallback/silence SDU;
- immutable fallback is attached to stream configuration;
- Bridge only selects/copies opaque payload.

Test fallback bytes by offline decode. Do not teach Bridge how to encode LC3.

## HIL boundary

HIL is only needed to validate that the selected codec configuration is accepted and acoustically decoded by the earbuds/controller path. Codec wrapper correctness is otherwise deterministic.
