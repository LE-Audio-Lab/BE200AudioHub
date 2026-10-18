# Live Audio Relay Migration Progress

Status: **active**

Source reference: `STanJK/LE-Audio-Relay` main branch.

## Goal

Replace the synthetic-only Media source with a Windows live-audio path while
keeping the current BE200-owned LE Audio transport:

```text
Windows applications
    ↓
sacrificial physical default render sink
    ↓
Process Loopback capture
    ↓
PCM relay boundary
    ↓
SPSC stereo PCM ring
    ↓
10 ms Media consumer
    ↓
LC3 encoder
    ↓
IsoTxBridge / StreamClock
    ↓
logical HCI ISO
    ↓
BE200
    ↓
Buds3 Pro
```

The old Relay destination-side WASAPI render sink is intentionally **not**
migrated. In BE200AudioHub, L1-Media + HCI ISO replaces it.

## Frozen Relay-derived PCM parameters

```text
sample rate             48 kHz
format                  Float32 stereo
capture block target    10 ms / 480 frames
ring physical capacity  80 ms / 3840 frames
retained cushion        10 ms / 480 frames
startup hold ceiling    40 ms / 1920 frames
relay activation        consumer request + cushion
                        = 480 + 480 = 960 frames / 20 ms
```

Positive-drift guard, inherited semantically from Relay:

```text
target residual         10 ms
gradual trim stops      11 ms
gradual trim starts     12 ms
hard recenter           20 ms
physical capacity       80 ms
```

## Migration checkpoints

### M1 — ownership mapping — DONE

Mapped old Relay:

```text
ProcessLoopbackSource
→ PcmRelayBoundary
→ SpscPcmRing
→ WASAPI Buds render
```

to new BE200 path:

```text
ProcessLoopbackSource
→ PcmRelayBoundary
→ SpscStereoPcmRing
→ LC3
→ IsoTxBridge
→ HCI ISO
```

### M2 — SPSC PCM ring — DONE

Landed `l1_media/pcm/ring.rs`.

Properties:

- bounded;
- Float32 stereo;
- one producer / one consumer;
- lock-free;
- no per-frame allocation;
- no unsafe;
- explicit startup max-fill behavior;
- silence write and discard support.

### M3 — PCM relay boundary + provisional drift guard — LANDED

Landed:

- `l1_media/pcm/relay_boundary.rs`
- `l1_media/pcm/drift.rs`

Preserves Relay KEEPALIVE → ARMED → RELAY activation semantics. Consumer-side
keepalive is now PCM zeros fed toward Media rather than zeros rendered through
a Windows Buds endpoint.

### M3.1 — producer/consumer split — LANDED

`PcmRelayBoundary` is now consumer-owned and returns a separate
`PcmRelayProducer` for the capture thread. The two sides share only the
lock-free ring plus atomic mode. Consumer-only drift state is never shared with
the WASAPI producer.

### M4 — Windows Process Loopback source — LANDED

Raw Win32/WASAPI details are isolated in
`platform/windows/process_loopback.rs`; L1-Media sees only copied Float32
stereo packets through `source/process_loopback.rs`.

Ported source semantics:


- Windows 10 build 20348+;
- process loopback;
- exclude current process tree;
- shared mode;
- event driven;
- 48 kHz Float32 stereo;
- approximately 10 ms capture buffer;
- no destination endpoint ownership.

### M5 — live PCM → LC3 → bridge — LANDED

Landed `l1_media/live.rs` with `LivePcmIsoRuntime`.

The 10 ms Media consumer:


1. consumes exactly 480 stereo Float32 frames from `PcmRelayBoundary`;
2. converts to Media PCM representation;
3. assigns absolute FrameId from StreamClock;
4. encodes left/right LC3;
5. pushes encoded frame to IsoTxBridge.

### M5.1 — live Media worker — LANDED

`LivePcmIsoWorker` owns the live absolute-grid loop on a dedicated Media
thread. It starts before Process Loopback activation and emits encoded zero PCM
while capture is not ready, so COM/WASAPI startup cannot starve an already-live
CIS.

### M6 — current-path live HIL — SOFTWARE PASS / HIL READY

Use the existing Round8 current Control/HCI setup and teardown path, replacing
only the synthetic source with live Process Loopback input.


## Current HIL entry point

```text
be200hub round8 live-relay
```

The harness reuses the same current-path Control driver as First Sound. Runtime
shutdown order is source -> Media worker -> ISO drain -> StopControl reverse
teardown.


## Software gate

Current live-relay checkpoint:

```text
cargo test --workspace --locked   PASS — 399 passed / 0 failed
cargo check --workspace --locked  PASS
cargo build --workspace --locked  PASS
```

The remaining acceptance step is real-hardware `round8 live-relay` HIL.
