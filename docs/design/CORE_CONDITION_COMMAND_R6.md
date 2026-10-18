
# Round 6 — Core / Condition / Command / Lifecycle Audit

Status: **V2 structural refactor complete**

This document records the actual architecture after Round 6. It supersedes the transitional RuntimeState / RuntimeAggregator design. It does not claim that the final composed product runtime, WASAPI capture, ASRC, or ControlMachine semantic decomposition are complete.

---

## 1. Final source root

The source root is now:

```text
src/
├── main.rs
├── core/
├── plane/
├── runtime/
├── platform/
└── review/
```

The following no longer exist:

```text
src/architecture/
src/runtime/aggregate.rs
src/control/
src/hci_host/
src/media/
src/stream_clock/
src/iso_tx_bridge/
src/transport/
```

Repository search after the migration reports no remaining source references to:

```text
architecture::
RuntimeAggregator
RuntimeObservation
RuntimeState
core::condition::blackboard    from any Plane
```

There is no Shared / Common / SharedInfo / generic product Contract bucket.

---

## 2. Core structure

```text
core/
├── command/
│   ├── admission.rs
│   ├── dispatch.rs
│   ├── interface.rs
│   └── types.rs
├── condition/
│   ├── blackboard/
│   │   ├── lifecycle.rs
│   │   ├── control.rs
│   │   ├── hci.rs
│   │   ├── media.rs
│   │   └── transport.rs
│   ├── interface/
│   │   └── snapshot.rs
│   └── reducer/
├── lifecycle/
│   ├── identity.rs
│   ├── machine.rs
│   ├── model.rs
│   └── recovery.rs
└── timer/
```

Round 6 establishes ownership and API boundaries. Full L0 startup / shutdown / recovery composition remains a later V2 phase.

---

## 3. Lifecycle ownership

Core/Lifecycle now owns the product lifecycle identities:

```text
SessionId
StreamId
```

No duplicate identity definitions remain elsewhere.

The existing semantic invariant is preserved:

> StreamId is an L0-supplied identity. Moving its Rust type to Core/Lifecycle does not introduce a new allocation or minting policy.

Core/Lifecycle also owns:

```text
LifecycleState
SessionState
LifecycleSnapshot
LifecycleModel
RecoveryClass
LifecycleMachine
```

The lifecycle state remains deliberately coarse:

```text
Stopped
Starting
Waiting
Running
Recovering
Stopping
```

Detailed Bluetooth, ASE, CIS, Media, or Transport progress does not enter the lifecycle FSM.

The current LifecycleMachine is intentionally minimal. It establishes ownership without inventing startup or recovery sequencing that has not yet been composed and tested.

---

## 4. One Condition, one private Blackboard

There is exactly one read-side Core abstraction:

```text
Condition
    └── private Blackboard
```

Blackboard is private implementation state under core::condition.

Planes cannot import Blackboard types.

Command cannot obtain a Blackboard reference.

External consumers can only use:

- semantic Condition queries;
- immutable value snapshots;
- typed apply methods for provider-owned ConditionUpdate values.

Blackboard is a materialized read model, not operational truth.

Operational truth remains inside each owner Plane.

---

## 5. Condition snapshot

The public snapshot contains value projections:

```text
ConditionSnapshot
├── epoch
├── generation
├── lifecycle
├── control
├── hci
├── media
└── transport
```

HCI and Media use Core-owned materialized views because their semantic state and coalesced telemetry arrive independently:

```text
HciConditionView
├── state
└── iso_telemetry

MediaConditionView
├── state
└── bridge_telemetry
```

This prevents a delayed telemetry publication from overwriting a newer semantic state.

---

## 6. Condition scope and stale fencing

Every Plane update enters Condition with a ConditionScope:

```text
generation
optional SessionId
```

Reducer rules:

1. An update from the wrong generation is rejected as stale.
2. A session-scoped update for a non-current session is rejected as stale.
3. A generation advance resets Plane read projections.
4. A session change resets session-scoped read projections.
5. Identical effective state does not advance the Condition epoch.

These are read-model fences. They do not mutate Plane operational state.

---

## 7. Provider-owned ConditionUpdate types

Each provider Plane defines what it can truthfully report.

### L1-Control

```text
ControlConditionUpdate
```

contains the coarse semantic Control projection:

- ControlState
- optional StreamId
- optional LeAudioFormat

ControlMachine computes this projection from its authoritative local state.

### L1-HCI

HCI uses separate state and telemetry updates:

```text
HciConditionUpdate::State(HciConditionState)

HciConditionUpdate::Telemetry(HciIsoTelemetrySnapshot)

HciConditionUpdate::Final {
    state,
    iso_telemetry,
}
```

State and telemetry are separated deliberately.

A high-rate/coalesced telemetry update cannot replace a newer semantic HCI state.

### L1-Media

Media follows the same pattern:

```text
MediaConditionUpdate::State(MediaConditionState)

MediaConditionUpdate::Telemetry(MediaBridgeTelemetrySnapshot)

MediaConditionUpdate::Final {
    state,
    bridge_telemetry,
}
```

Bridge telemetry cannot overwrite Media semantic state.

### L2-Transport

Transport publishes:

```text
TransportConditionUpdate {
    state,
    rx_packets,
    tx_packets,
    faults,
}
```

Transport counters are accumulated locally in the Reactor at the actual producer event sites.

---

## 8. Three observation lanes remain separate

### Reliable semantic lane

Used for protocol/lifecycle truth such as:

- connection/security changes;
- ASE changes;
- CIG/CIS/path changes;
- route ACK;
- faults;
- lifecycle changes.

This lane must not silently drop.

### Aggregated telemetry lane

Used for already-coalesced metrics such as:

- HCI submitted/completed/retired/in-flight;
- Media submitted/drop/fallback/skipped/stale-frame counters;
- Transport counters.

Producer-local aggregation happens before Condition.

### Raw diagnostic lane

Raw history remains in bounded producer-local rings.

Raw trace records are not embedded in ConditionUpdate values.

Condition sees aggregate telemetry snapshots and ring statistics only, never the raw record vector.

---

## 9. State, telemetry, and forced-final publication contract

Round 6 freezes three publication semantics.

### State change

Semantic state changes are represented as State updates and are intended for immediate publication by the composed runtime.

### High-rate telemetry

Producer-local counters update on each event.

At a bounded publication point:

```text
dirty aggregate
    ->
take_condition_telemetry_update()
    ->
Telemetry(...)
```

The dirty flag is publication bookkeeping only. It does not reset counters or protocol state.

The exact wall-clock cadence is intentionally not frozen in the Plane. A later runtime composition may choose an appropriate bounded cadence without changing the provider API.

### Fault / teardown

HCI and Media expose forced-final publication:

```text
force_condition_update(...)
    ->
Final { semantic state + aggregate telemetry }
```

Forced final publication does not depend on the telemetry dirty bit.

This ensures a teardown/fault can publish the last aggregate state even if the normal telemetry cadence has just consumed the dirty flag.

---

## 10. Producer publication points

### HCI Engine

HCI provides:

```text
condition_state()
condition_state_update()
take_condition_telemetry_update()
force_condition_update()
```

The current HCI state projection honestly derives attach-level state from the HCI owner. Higher semantic states such as StreamBound/Streaming remain available in the schema for the composed HCI/Control runtime to publish when those semantic facts are confirmed.

HCI telemetry remains a mirror of authoritative IsoCreditAudit state.

Number Of Completed Packets still has the dependency chain:

```text
controller event
-> HCI observation
-> credit audit
-> producer telemetry mirror
```

It has no dependency on FrameId, LocalStreamClock, the Media scheduler, PCM production, or LC3 production.

### Media Bridge

Media Bridge provides:

```text
take_condition_telemetry_update()
force_condition_update(MediaConditionState)
```

The Bridge does not claim ownership of the whole Media state. The final semantic Media state is supplied by the Media owner.

Backpressure behavior is unchanged:

```text
full logical ISO lane
-> drop current logical interval
-> continue absolute media grid
```

No retry or time stretching is introduced.

### Transport Reactor

Transport now accumulates locally:

```text
rx_packets
tx_packets
faults
```

and exposes condition_update().

A Transport fault immediately changes the projected state to Faulted and increments the local fault counter.

---

## 11. Command boundary

Core/Command is the write-side API.

Current command vocabulary intentionally contains only already-supported product actions:

```text
StartControl

BeginStreamHandoff {
    SessionId,
    StreamId,
}
```

Execution flow:

```text
CoreCommand
    ->
Condition semantic query
    ->
admission
    ->
ControlPlane.handle_intent(...)
    ->
Control owner validates actual state again
```

Command never reads Blackboard.

Command does not manually unpack ConditionSnapshot fields to duplicate admission logic.

Condition admission is advisory/read-side; the real owner always revalidates execution-time invariants.

---

## 12. RuntimeState / RuntimeAggregator retirement

The following transitional concepts are gone:

```text
RuntimeState
RuntimeObservation
RuntimeAggregator
PlaneTelemetryAccumulator
```

Their responsibilities are now split correctly:

```text
Plane operational model
    = authoritative owner truth

Plane telemetry accumulator
    = producer-local aggregation

Plane ConditionUpdate
    = provider-owned projection/update vocabulary

ConditionReducer
    = Core read-model merge

private Blackboard
    = materialized read model

Condition API
    = only Core read surface
```

There is no central raw telemetry processor.

---

## 13. architecture/ retirement

src/architecture/ is completely removed.

Former concepts now have real owners:

- SessionId / StreamId -> Core/Lifecycle
- Lifecycle model -> Core/Lifecycle
- Control stream contract -> L1-Control Interface
- Control read projection -> L1-Control ConditionUpdate / Core Condition view
- HCI read projection -> L1-HCI ConditionUpdate / Core Condition view
- Media read projection -> L1-Media ConditionUpdate / Core Condition view
- Transport read projection -> L2-Transport ConditionUpdate
- FrameId / StreamClock implementation -> L1-Media
- IsoTxBridge -> L1-Media
- Generation -> runtime/generation
- execution policy -> runtime/context/policy
- generic mailbox policy -> runtime/mailbox/contract
- TelemetryRing<T> -> runtime/telemetry/ring

No architecture-level shared type registry remains.

---

## 14. Regression evidence added in Round 6

Round 6 locks the following properties in software tests:

- new Lifecycle begins Stopped with no session;
- identical Condition update does not churn epoch;
- stale generation update is rejected;
- stale session update is rejected;
- generation advance resets Plane read projections;
- Command admission uses Condition semantic API;
- wrong-session command is rejected before owner invocation;
- coalesced HCI telemetry does not overwrite newer StreamBound semantic state;
- HCI telemetry dirty publication is one-shot/coalesced;
- HCI forced-final publication works even after dirty was consumed;
- HCI teardown retirement remains retired != completed;
- Media telemetry dirty publication is one-shot/coalesced;
- Media forced-final publication works after dirty was consumed;
- Media backpressure still advances the absolute FrameId grid;
- Transport inbox overflow projects Faulted immediately;
- Transport rx/tx/fault counters are preserved in its ConditionUpdate.

---

## 15. ControlModel extraction is deliberately deferred

Round 6 does **not** introduce ControlModel.current/target.

Round 5 audit established that ControlPhase still mixes:

- semantic milestone;
- operation cursor;
- active side/member;
- timer/wait state;
- rollback progress.

Introducing current/target before extracting operations would preserve the same mega-FSM coupling under new names.

The next semantic refactor should therefore follow the existing extraction plan:

1. mechanically extract durable ControlModel facts;
2. extract RollbackOperation;
3. extract AudioDiscovery operation;
4. extract ASE operations;
5. extract StreamingGate;
6. extract StreamRouteBind;
7. isolate conservative cleanup knowledge;
8. only then introduce explicit current/target semantics.

This keeps Round 6 a structural boundary refactor rather than an unreviewable FSM rewrite.

---

## 16. Deferred composition work

Round 6 establishes the APIs required for composition but does not claim the full product runtime is composed.

Still deferred:

- concrete Core startup/shutdown/recovery sequencing;
- actual Core timer owner service;
- scheduling the bounded telemetry publication cadence;
- final MediaRt runtime owner;
- full V2 composed-runtime synthetic/HIL acceptance;
- WASAPI Process Loopback;
- ASRC/clock discipline.

These are subsequent product implementation phases, not missing ownership from this refactor.

---

## 17. Round 6 conclusion

The V2 structural refactor has reached its intended endpoint:

```text
Command
    = Core write API

Condition
    = Core read API
    = one private Blackboard

Lifecycle
    = coarse L0 state and identity owner

Planes
    = authoritative operational owners
    = provider-owned ConditionUpdate vocabulary
    = producer-local telemetry

runtime/
    = cross-layer execution primitives

architecture/
    = removed
```

The system now has no shared mutable world-state shortcut and no central mega-observation processor.

Top-down intent and bottom-up fact are physically separate in the source tree and in the Rust type boundaries.
