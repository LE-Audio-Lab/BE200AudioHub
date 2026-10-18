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

### M4 — Windows Process Loopback source — NEXT

Port the source semantics only:

- Windows 10 build 20348+;
- process loopback;
- exclude current process tree;
- shared mode;
- event driven;
- 48 kHz Float32 stereo;
- approximately 10 ms capture buffer;
- no destination endpoint ownership.

### M5 — live PCM → LC3 → bridge — PENDING

Add the 10 ms Media consumer that:

1. consumes exactly 480 stereo Float32 frames from `PcmRelayBoundary`;
2. converts to Media PCM representation;
3. assigns absolute FrameId from StreamClock;
4. encodes left/right LC3;
5. pushes encoded frame to IsoTxBridge.

### M6 — current-path live HIL — PENDING

Use the existing Round8 current Control/HCI setup and teardown path, replacing
only the synthetic source with live Process Loopback input.
