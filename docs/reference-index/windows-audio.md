# Windows Audio Capture Reference Index

## BE200AudioHub contract

Windows audio is a Media input backend, not the destination/output architecture.

Target shape:

```text
CaptureContext
  Windows Process Loopback
  source clock/QPC observations
  packet/discontinuity handling
        ↓ bounded SPSC PCM
MediaRtContext
  StreamClock / ASRC / LC3
```

CaptureContext follows the Windows source event cadence. It must not become the target StreamClock.

## Primary platform precedent — LE-Audio-Relay

Project: `STanJK/LE-Audio-Relay`

File: `Routing/ProcessLoopbackSource.cs`

### `ProcessLoopbackSource.CreateExcludeCurrentProcessTreeAsync()`

Exact precedent for capture creation:

- Process Loopback;
- exclude current process tree;
- requested canonical format;
- shared mode;
- event sync;
- explicit buffer length;
- MMCSS `"Pro Audio"`.

Adopt:

- event-driven Process Loopback;
- exclusion of relay/hub process tree;
- COM/WASAPI object lifetime owned by CaptureContext.

Do not copy:

- downstream render/keepalive path.

### `ProcessLoopbackSource.Start()`

Idempotent capture start.

### `ProcessLoopbackSource.Stop()`

Idempotent capture stop.

### `ProcessLoopbackSource.Dispose()`

Reference for:

- unsubscribe callbacks before object disposal;
- stop before disposing recorder.

### `ProcessLoopbackSource.OnDataAvailable()`

Thin foreign/audio callback:

- forwards packet bytes, flags, device position and QPC position;
- no routing policy.

BE200AudioHub should preserve this thin-callback property.

### `ProcessLoopbackSource.OnRecordingStopped()`

Resets local started state and reports terminal exception.

Reference for explicit capture-health event.

## SPSC boundary precedent

Project: LE-Audio-Relay

File: `Timing/SpscPcmRing.cs`

### `SpscPcmRing.Write()`

Reference for bounded producer write and explicit max-fill limit.

### `WriteSilence()`

Reference for source-declared silence without constructing temporary PCM buffers.

### `Read()`

Reference for consumer-side bounded read.

### `Discard()`

Reference for explicit discrete correction/drop.

Implementation note:

- this byte-ring is precedent, not a required exact representation;
- BE200AudioHub canonical PCM is planned as 48 kHz Float32 stereo.

## Capture/render boundary precedent

File: `Timing/PcmRelayBoundary.cs`

### `Arm()`

Marks boundary ready to transition from startup hold to relay.

### `Disable()`

Returns to keepalive/no-relay mode.

### `Push()`

Read for:

- producer input frames;
- silent packet handling;
- bounded ring write;
- telemetry instead of callback blocking.

### `FillRenderBuffer()`

Read only for ring/cushion behavior:

- startup arm waits for requested frames + cushion;
- trims startup excess;
- read is bounded;
- missing data results in zero-filled destination.

Do not copy the render/keepalive semantics into BE200AudioHub. The useful precedent is bounded producer/consumer decoupling.

## Route lifecycle precedent

File: `Routing/RouteSession.cs`

### `RouteSession.StartAsync()`

Read for ordering:

- validate endpoint/routing;
- create canonical format;
- construct ring/boundary/provider;
- create render and Process Loopback capture;
- transfer ownership into session;
- start components in explicit order.

For BE200AudioHub, keep only the source/capture lessons; destination render is replaced by Media/LE Audio pipeline.

### `RouteSession.DisposeAsync()`

Reference for ordered shutdown and callback unsubscription.

### `OnCapturePacket()`

Reference for:

- source-declared silent flag;
- callback pushes into bounded boundary;
- callback exception recorded and escalated.

### `OnCaptureStopped()`

Reference for unexpected source termination becoming lifecycle fault unless shutdown is already intentional.

### `SignalFailure()`

Reference for one-shot fault publication.

## Temporary drift precedent

File: `Timing/ProvisionalPositiveDriftGuard.cs`

### `GetSilentFramesToSuppress()`

Use only as a discrete source-domain correction example.

### `ApplyAfterRender()`

Use only for:

- bounded cushion target;
- gradual one-frame slips;
- emergency recenter.

The file explicitly says it is **not clock synchronization**. Final BE200AudioHub uses StreamClock + estimator/ASRC.

## CaptureContext deterministic test obligations

Use a fake Process Loopback source adapter.

Tests:

1. start/stop idempotence;
2. packet callback preserves PCM frames;
3. Silent flag produces correct zero/silence representation;
4. source discontinuity flag creates explicit discontinuity event;
5. QPC/device position observations are forwarded to estimator input;
6. ring full never blocks callback;
7. configured overflow policy is explicit and counted;
8. capture stopped during normal shutdown does not become a fault;
9. unexpected capture stopped does become a fault;
10. stale-generation callbacks cannot write into a new session;
11. CaptureContext never waits for MediaRt consumer progress.

## HIL boundary

Hardware/Windows tests validate:

- Process Loopback activation on target Windows build;
- process-tree exclusion;
- callback packet sizes/cadence;
- QPC behavior;
- device invalidation;
- actual scheduling jitter.

Those measurements must not redefine the target Media clock.
