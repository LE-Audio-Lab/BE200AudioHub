# Round 4 — L1 Media and Runtime Ownership Audit

Status: **Round 4 implementation handoff**

This note records the actual ownership boundaries after the V2 Round 4
migration. It does not introduce WASAPI capture, ASRC, controller clock
discipline, or Core Condition publication.

## 1. Active L1-Media layout

```text
plane/l1_media/
├── source/
│   └── synthetic.rs
├── codec/
│   └── lc3.rs
├── clock/
│   ├── frame.rs
│   └── local.rs
├── bridge/
│   ├── scheduler.rs
│   └── runtime.rs
├── interface/
│   └── frame.rs
├── telemetry/
│   ├── accumulator.rs
│   ├── observation.rs
│   └── trace.rs
├── model/
└── capture/
```

`model/` and `capture/` intentionally remain empty until real durable Media
model state and WASAPI capture implementation exist.

The old top-level trees are removed:

```text
src/media/
src/stream_clock/
src/iso_tx_bridge/
```

## 2. Media-owned identities and contracts

Round 4 establishes one physical owner for:

```text
FrameId
    -> plane/l1_media/clock/frame.rs

EncodedStereoFrame<P>
    -> plane/l1_media/interface/frame.rs

LocalStreamClock
    -> plane/l1_media/clock/local.rs

IntervalGroupScheduler
    -> plane/l1_media/bridge/scheduler.rs

LogicalIsoTxBridge
    -> plane/l1_media/bridge/runtime.rs
```

`SessionId`, `StreamId`, `AudioSide`, and `StreamBinding` were not
re-owned in Round 4. Their final ownership is handled by the later
Control/Core migration.

## 3. Authoritative media time

The authoritative media grid remains:

```text
StreamBinding frame duration
        +
LocalStreamClock epoch
        +
FrameId
        ->
absolute target time
```

`IntervalGroupScheduler::on_wake(now_us)` derives the current logical frame
from this absolute grid.

Callback jitter does not shift future deadlines.

If the callback wakes late, the scheduler:

1. computes the current absolute-grid FrameId;
2. drops expired buffered frames;
3. records skipped logical intervals;
4. submits the current interval;
5. advances to the next grid FrameId.

It does not stretch the grid or wait for HCI/controller progress.

## 4. HCI boundary

Production L1-Media has no dependency on:

- raw CIS connection handles;
- `CisRef`;
- controller ISO credits;
- Number Of Completed Packets;
- `HciCommand` / `HciReport`;
- `IsoTxAudit`.

The Bridge runtime owns only:

```text
HciLogicalIsoTx
```

The logical request type is surfaced through:

```text
plane/l1_hci/interface/iso_tx.rs
```

so the scheduler does not import HCI's internal ISO implementation module.

The resulting Media -> HCI boundary is:

```text
FrameId + encoded stereo payload
        ->
Media scheduler
        ->
LogicalIsoSduGroupRequest
        ->
HciLogicalIsoTx
```

Raw handle resolution remains entirely inside L1-HCI.

## 5. Backpressure invariant

When the bounded HCI logical ISO lane is full:

```text
try_submit_group
    -> SendError::Full
    -> BridgeTxStep::DroppedBackpressure
```

The interval is dropped atomically.

It is not retried on the next media interval.

The scheduler's next FrameId/deadline continues on the absolute media grid.

Therefore:

> controller/HCI capacity can cause a bounded data drop, but it cannot slow,
> pause, or advance Media time.

## 6. HCI completion invariant

Round 4 re-audited L1-Media imports.

No production Media path consumes Number Of Completed Packets, HCI credit
counters, or HCI completion telemetry.

Therefore the frozen invariant remains:

> HCI completion releases controller capacity only and never advances FrameId
> or LocalStreamClock.

## 7. Producer-local Media telemetry

Round 4 adds Bridge telemetry only for events that already exist in the
scheduler/runtime.

Aggregate schema:

```text
MediaBridgeTelemetrySnapshot {
    submitted_intervals
    dropped_backpressure_intervals
    fallback_intervals
    skipped_intervals
    stale_frames_dropped
    trace
}
```

Semantics:

- `submitted_intervals`: logical stereo intervals accepted by the HCI bounded
  data lane;
- `dropped_backpressure_intervals`: complete logical stereo intervals dropped
  because the HCI data lane was full;
- `fallback_intervals`: intervals that used the complete stereo fallback;
- `skipped_intervals`: absolute-grid intervals skipped because the scheduler
  woke late;
- `stale_frames_dropped`: encoded buffered frames whose absolute grid target
  had expired.

Telemetry does not alter scheduler state, retry behavior, FrameId, StreamClock,
or HCI admission.

## 8. Media raw trace

Current raw trace records are deliberately narrow:

```text
MediaBridgeTraceRecord::Submitted
MediaBridgeTraceRecord::DroppedBackpressure
```

Each record contains only Media-domain data:

```text
FrameId
IntervalGroupSource
skipped_intervals
stale_frames_dropped
```

It contains no CIS handle, controller credit, HCI sequence number, PB flag, or
USB information.

Default capacity is 256 records using the generic runtime
`TelemetryRing<T>` recent-history/overwrite-oldest policy.

Timer `Wait` events are intentionally not traced to avoid high-rate wake
noise.

## 9. Runtime substrate after Round 4

```text
runtime/
├── aggregate.rs
├── cancellation/
│   └── mod.rs
├── context/
│   ├── mod.rs
│   └── policy.rs
├── generation/
│   └── mod.rs
├── mailbox/
│   ├── mod.rs
│   └── contract.rs
├── request/
│   └── mod.rs
├── supervisor/
│   └── mod.rs
└── telemetry/
    ├── mod.rs
    └── ring.rs
```

`aggregate.rs` is intentionally transitional and remains for the later
Condition migration.

## 10. Runtime ownership decisions

### Generation

`Generation` is owned by:

```text
runtime/generation/
```

because it is a generation-fencing primitive.

Architecture read-model files may reference it, but do not define a second
Generation identity.

### Execution-domain policy

```text
ExecutionDomain
BlockingRule
ExecutionPolicy
```

are owned by:

```text
runtime/context/policy.rs
```

They constrain execution contexts and are not product-domain state.

### RequestId

`RequestId` is owned by:

```text
runtime/request/
```

because it identifies runtime request/reply correlation.

### Generic mailbox policy

```text
DeliveryClass
OverflowAction
MailboxContract
RELIABLE_MAILBOX
TELEMETRY_MAILBOX
```

live in:

```text
runtime/mailbox/contract.rs
```

These types describe generic runtime delivery semantics.

## 11. MessageScope audit

`MessageScope` and `Envelope<T>` were deliberately **not** moved into
runtime.

They currently carry:

```text
Generation
Option<SessionId>
Option<StreamId>
```

and therefore mix runtime fencing with product lifecycle/stream identity.

They also have no active construction path in the current product runtime.

They remain temporarily in `architecture/mailbox.rs` so Round 6 can either:

- delete them if the real runtime composition does not require this envelope;
- or assign them to the actual Core/Lifecycle interface that owns scoped
  product messages.

Creating `runtime/shared`, `shared_info`, or a generic `contract` bucket
for them is explicitly rejected.

## 12. Media-specific mailbox policy

`SOURCE_PCM_RING` and `TARGET_FRAME_STORE` also remain transitional rather
than being falsely declared generic runtime policy.

They describe Media latency/drop semantics and should move only when the real
capture/source buffer implementation exists.

## 13. Deferred work

Round 4 does not implement:

- WASAPI Process Loopback capture;
- source SPSC/ring implementation;
- clock estimator;
- ASRC;
- controller-anchored StreamClock;
- Media ConditionUpdate publication;
- Core Blackboard integration.

Producer-local Media telemetry is ready for a later bounded/coalesced
Condition publication path, but no such cross-thread path is added here.

## 14. Round 4 invariant summary

```text
source / LC3
    ->
Media-owned FrameId
    ->
LocalStreamClock absolute grid
    ->
IntervalGroupScheduler
    ->
LogicalIsoTxBridge
    ->
HciLogicalIsoTx
```

The architectural boundary is:

> Media owns time and logical interval identity. HCI owns controller capacity
> and transport-handle resolution. Backpressure may drop an interval, but HCI
> state never disciplines Media time.
