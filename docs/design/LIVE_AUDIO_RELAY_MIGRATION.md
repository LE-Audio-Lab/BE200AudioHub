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

### M6 — current-path live HIL — HIL PASS ×2

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

Real-hardware `round8 live-relay` has passed twice consecutively with identical clean 10-second results.


## HIL findings

### HIL-1 — process-loopback activation rejected — FIXED

Observed on real BE200 + Galaxy Buds3 Pro:

```text
Current Control/LE Audio setup reached the earbuds successfully.
Earbud connection tone was audible.
Windows Process Loopback then failed immediately with:
ActivateAudioInterface(-2147483634)
```

`-2147483634 == 0x8000000E == E_ILLEGAL_METHOD_CALL`.

Root cause: the hand-written `IActivateAudioInterfaceCompletionHandler`
advertised `IUnknown` and the completion-handler IID but did not advertise
`IAgileObject`. `ActivateAudioInterfaceAsync` requires an agile completion
handler and rejects a non-agile handler before asynchronous activation begins.

Fix:

```text
18cf4e13  fix(wasapi): expose agile completion handler
da16206b  test(wasapi): keep agile COM regression inside adapter
```

Software regression after the fix:

```text
cargo test --workspace --locked   PASS — 400 passed / 0 failed
cargo build --workspace --locked  PASS
```


### HIL-2 — live Windows audio end-to-end — PASS ×2

Two consecutive 10-second runs on real BE200 + Galaxy Buds3 Pro reproduced the
same clean result while music was audibly reproduced through the earbuds:

```text
Process Loopback:
  packets=1000
  frames=480000
  silent_frames=0
  discontinuities=0

PCM boundary:
  captured=480000
  written=480000
  dropped=0
  silent_trim=0

Media clock:
  intervals=1000
  keepalive=2
  source_skipped=0
  underflow_frames=960
  startup_trim=0
  gradual_trim=0
  emergency_trim=0

Bridge:
  submitted=1000
  backpressure_drop=0
  fallback=0
  grid_skipped=0
  stale=0

ISO Host:
  submitted=2000
  completed=2000
  in_flight_after=0

StopControl reverse teardown -> Control Idle: PASS
Bond store unchanged: true
```

The two keepalive intervals and 960 underflow PCM frames correspond exactly to
the 20 ms source-startup window before Process Loopback supplied live PCM.

## Longevity mode

Long-term clock/drift acceptance uses:

```text
round8 live-relay --longevity-test
```

Policy:

```text
normal mode:
  default 10 s
  maximum 120 s

longevity mode:
  default 300 s
  maximum 86400 s / 24 h
  heartbeat every 30 s
```

`--seconds` remains available in longevity mode. The parser validates the
duration only after all flags are read, so flag order is irrelevant.


### HIL-3 — 300-second longevity data plane passed, normal teardown ordering failed

A five-minute longevity run reached 300/300 seconds with continuous audible
music, then failed only during normal Stop:

```text
TeardownFailed {
  cleanup: [
    Timeout(CleanupAse { side: Right }),
    Timeout(CleanupAse { side: Left })
  ]
}
```

This localizes the failure to graceful teardown after the longevity data window,
not to live PCM capture, LC3, ISO scheduling, or the established CIS stream.

Shared-resource source review corrected the teardown model:

```text
AOSP system/bta/le_audio/state_machine.cc

SuspendStream:
  target QoS Configured
  -> ASCS Disable

StopStream:
  target Idle
  -> ASCS Release

Streaming -> Releasing:
  -> Remove ISO data path
  -> Disconnect CIS after data-path removal
  -> remove CIG once CISes are gone
```

Bumble-rs `bumble-profiles/src/ascs.rs` independently confirms:

```text
Disable on Sink:
  Streaming/Enabling -> QoS Configured

Release:
  any non-Idle ASE -> Releasing
```

BE200AudioHub normal Stop had inherited failure rollback ordering:

```text
route -> path -> CIS -> Release ASE -> CIG
```

That ordering is now split from failure rollback. Normal Stop becomes:

```text
route -> Release ASE -> path -> CIS -> CIG -> ACL/privacy
```

Failure rollback remains intentionally fail-closed:

```text
route -> path -> CIS -> Release ASE -> CIG -> ACL/privacy
```

AOSP also accepts Release completion into either Idle or cached Codec
Configured. BE200AudioHub now preserves strict readback validation for all
other ASCS operations but treats Codec Configured as an additional valid
terminal readback for Release. The HIL-3 failure itself was still a timeout
before readback; this compatibility fix closes the adjacent caching gap before
the next longevity run.


### HIL-4 — graceful Release must pivot on RELEASING

A second 300-second longevity run kept the live data plane healthy through the
full window, but normal Stop again ended with dual `CleanupAse` timeouts.

The previous HIL-3 fix corrected command ordering but still modeled Release as
one synchronous operation that had to reach terminal Idle/Codec Configured
before ISO path/CIS teardown. AOSP's LE Audio state machine does not do that.

For a Streaming Sink ASE, AOSP uses:

```text
Release
  -> ASE notification: RELEASING
  -> Remove ISO data path
  -> Disconnect CIS
  -> terminal ASE state: Idle or cached Codec Configured
```

Therefore `RELEASING` is the graceful-stop transport teardown pivot, not the
terminal completion of Release.

BE200AudioHub now gives graceful Stop a notification-gated Release operation:
ATT/Control-Point success plus an ASE RELEASING notification is sufficient to
advance into ISO path/CIS cleanup. Other ASCS operations retain strict readback
verification, and failure rollback keeps its existing terminal Release
readback after transport retirement.


### HIL-5 — ASCS Control Point uses Write Without Response

Repeated 30-second HIL runs showed both graceful Release operations timing out
before any ATT Write Response, Control Point response, or ASE RELEASING
notification was observed. A 1-second run could still complete teardown, making
the previous Write Request behavior appear intermittently valid.

Reference implementations do not require an ATT Write Response for normal ASCS
Control Point procedures. AOSP uses its no-response GATT write path for short
Control Point payloads, and Bumble clients likewise issue the Control Point
write without response.

BE200AudioHub now maps ASCS Control Point operations to
`GattOperation::WriteCommand`. CCCD subscription continues to use
`WriteRequest`. The ASCS state machine still requires the semantic Control
Point SUCCESS notification and the expected ASE state notification; only the
non-normative ATT Write Response dependency is removed.


### HIL-6 — teardown ACL queue audit

A 30-second normal-mode run still failed graceful Release after ASCS Control
Point writes were aligned to Write Command. The Release gate showed local write
completion but no Control Point SUCCESS or ASE state notification on either
ear.

Bumble's `send_l2cap_on_handle()` returns success after enqueueing ACL
fragments even when controller credits prevent immediate HCI submission.
Therefore local Write Command completion is not proof that the packet reached
the controller.

The Round8 failure path now queries a diagnostic ACL snapshot after teardown
failure:
- `bumble_pending`: Bumble ACL packets queued or in flight.
- `audit_in_flight`: packets actually submitted through QueueHostTransport
  and not yet completed by HCI Number Of Completed Packets.
- `waiting_estimate = bumble_pending - audit_in_flight`.

Interpretation:
- waiting_estimate > 0: packet(s) are still blocked in Bumble's host queue.
- waiting_estimate == 0 and audit_in_flight > 0: packet(s) reached HCI but
  controller completion is still outstanding.
- both zero: host/controller transmission retired; missing CP/ASE response is
  above the HCI ACL completion boundary.

### Round11 decision — preserve graceful architecture, opt in to BE200 workaround

Round10 ultimately showed that the long-session Release failure was a symptom
of a broader BE200-path Host-facing ACL-RX cessation that also occurs without
CIG/CIS/ISO/media.

The final architecture does **not** redefine normal LE Audio Stop around that
quirk.

Default teardown remains:

```text
route
→ ASCS Release / RELEASING
→ ISO path
→ CIS
→ CIG
→ ACL/privacy
```

For current BE200 HIL/product-path runs, the controller-clean workaround is
selected explicitly:

```powershell
--ungraceful-teardown
```

That path skips peer ASCS Release and retires route/path/CIS/CIG/ACL/privacy
resources directly. A successful run with this flag is not evidence that peer
graceful Release completed.

See [BE200_TEARDOWN_POLICY.md](./BE200_TEARDOWN_POLICY.md).
