# Runtime Observation and Telemetry Pipeline

This document freezes the observation pipeline below Core/UI without wiring any
specific HCI, Media, or Transport producer yet.

## 1. Three distinct lanes

The runtime must not treat all lower-plane output as one class of "logs".

### A. Reliable semantic lane

State-changing protocol and lifecycle events are reliable typed traffic.

Examples:

- connection/security transitions;
- ATT/GATT semantic completion;
- ASE/CIG/CIS/ISO-path transitions;
- route bind/unbind acknowledgement;
- lifecycle state change;
- fault.

Rules:

- bounded;
- producer non-blocking;
- mailbox full is a fault;
- never silently drop;
- never overwrite with "latest";
- may drive Control/Lifecycle state machines.

### B. Aggregated telemetry lane

High-rate operational observations remain local to the owning plane until
aggregated.

Examples:

- packet counters;
- submitted/completed/in-flight;
- queue depth;
- ring fill;
- encode/deadline statistics;
- latency summaries.

Each producer owns a `PlaneTelemetryAccumulator<Event>` implementation:

```text
raw/high-rate event
    ↓
plane-local accumulator
    ↓
typed plane snapshot
    ↓
RuntimeObservation
```

The generic trait intentionally does not impose a common windowing/reset policy.
ISO counters, transport latency, and media timing have different semantics.

### C. Raw diagnostic trace lane

Raw diagnostic records use `TelemetryRing<T>`.

They are debugging evidence only and must never become protocol truth.

The default constructor is:

```rust
telemetry_ring::<T>(capacity)
```

Default overflow behavior:

```text
fixed capacity
→ producer never blocks
→ newest record is accepted
→ oldest record is evicted
→ overwritten_oldest counter increments
→ no persistent-storage write occurs
```

Explicit `DropNewest` remains available for cases where preserving the first
part of a trace is preferable.

## 2. Runtime aggregation

Core-side `RuntimeAggregator` is the single writer of `RuntimeState`.

```text
LifecycleSnapshot ─┐
ControlSnapshot   ─┤
MediaSnapshot     ─┤
HciHostSnapshot   ─┤
TransportSnapshot ─┘
        │
        ▼
RuntimeObservation
        │
        ▼
RuntimeAggregator
        │
        ▼
RuntimeState
```

Repeated identical observations are coalesced and do not advance the
RuntimeState observation epoch.

A changed observation updates only its plane projection.

## 3. Snapshot API

The normal upper-level read path is a value snapshot:

```text
RuntimeAggregator::snapshot()
        ↓
current RuntimeState clone
        ↓
CLI / egui / normal status log
```

Readers do not receive mutable access to subsystem objects or to the
aggregator-owned RuntimeState.

The intended normal operating mode is therefore:

```text
many raw events
→ local aggregation
→ occasional changed RuntimeState
→ normal log/status snapshot
```

not:

```text
every HCI/ISO/USB/media event
→ persistent log
```

## 4. Logging policy

Normal logs should be driven primarily by meaningful RuntimeState changes and
low-rate status snapshots.

Raw trace storage is bounded in memory. If a diagnostic ring fills, records are
lost according to its explicit policy and the loss counter records that fact.

Persistent raw logging, if added later, must be an explicit diagnostic mode
with its own bounded drain/backpressure policy. It is not the default runtime
behavior.

## 5. Ownership

```text
HciContext
├── private mutable HCI state
├── semantic reports -> ControlPlane
├── telemetry accumulator -> HciHostSnapshot
└── optional TelemetryRing<RawHciTrace>

MediaRtContext
├── private mutable media state
├── telemetry accumulator -> MediaSnapshot
└── optional TelemetryRing<RawMediaTrace>

TransportContext
├── private mutable transport state
├── telemetry accumulator -> TransportSnapshot
└── optional TelemetryRing<RawTransportTrace>

CoreContext
├── Lifecycle
├── ControlPlane
├── RuntimeAggregator
└── snapshot API
```

No lower plane mutates RuntimeState directly.

## 6. Current implementation boundary

Implemented now:

- generic `TelemetryRing<T>`;
- `telemetry_ring(capacity)` default recent-history helper;
- explicit DropNewest / OverwriteOldest policies;
- ring loss counters;
- `PlaneTelemetryAccumulator<Event>` contract;
- `RuntimeObservation`;
- Core-side `RuntimeAggregator`;
- read-only value `snapshot()`.

Not wired yet:

- concrete HCI telemetry accumulator;
- concrete Media telemetry accumulator;
- concrete Transport telemetry accumulator;
- raw trace producers;
- UI subscription/change notification;
- persistent diagnostic log drain.

Those are intentionally deferred until the owner-specific data structures are
designed.
