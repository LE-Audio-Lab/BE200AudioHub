# Post-Round8 L1-Control Decomposition

Status: **landed / software-gated**

This document records the implemented L1-Control decomposition after the V2
structural refactor. It supersedes the old implementation-name suggestion
`ControlModel` where naming conflicts with the structures that actually
landed. It does not change the frozen L1-Control architecture or the proven
protocol ordering.

## 1. Architecture did not change

The existing ownership tree remains:

```text
plane/l1_control/
├── engine/
├── model/
├── operation/
├── interface/
├── discovery/
├── planning/
└── ascs/
```

The work in this phase is semantic redistribution inside the already-frozen
L1-Control Plane, not a new Control architecture.

## 2. Implemented state ownership

### ConfirmedStatus

`model/confirmed_status.rs`

Contains durable facts that L1-Control has actually confirmed from protocol or
runtime evidence, including:

- resolved members;
- live generation-fenced ACL connections;
- encryption-key-size evidence;
- controller privacy activation;
- completed audio-set discovery;
- prepared ASE evidence;
- codec/QoS/enable readback status;
- live semantic CIS set;
- confirmed ISO-path readiness;
- confirmed streaming status;
- route-ACK-promoted active StreamBinding.

It is authoritative L1-Control operational truth. It is **not** the Core
Condition read model.

### PlannedConfiguration

`model/planned_configuration.rs`

Contains selected configuration that Control intends to apply:

- codec plan;
- ISO plan;
- QoS plan;
- enable plan.

A plan is not evidence that the remote/controller state has accepted or reached
that configuration. Confirmation remains in `ConfirmedStatus`.

### CleanupKnowledge

`model/cleanup_knowledge.rs`

Contains conservative safety knowledge after uncertain controller outcomes:

- CIG may exist;
- ISO paths may exist.

These values are deliberately not happy-path confirmed status. They exist only
to preserve fail-closed cleanup behavior.

## 3. Implemented operation ownership

Temporary cross-event progress now lives under `operation/`:

```text
operation/
├── audio_discovery.rs
├── ase_control.rs
├── rollback.rs
├── stream_route.rs
└── streaming.rs
```

### AudioDiscoveryOperation

Owns an in-progress `AudioDiscoveryMachine` and partial member accumulation.
Only the completed `AudioSetSnapshot` is promoted to `ConfirmedStatus`.

### AseControlOperation

Owns the active ASCS preparation/control local FSMs. The existing proven ASCS
success contract remains unchanged:

```text
ATT Write Response
+ ASCS Control Point success
+ ASE notification
+ independent readback
= semantic success
```

### StreamingOperation

Owns the in-progress `StreamingGateMachine`. Only verified STREAMING evidence
is promoted to `ConfirmedStatus`.

### StreamRouteOperation

Owns the pending transport-neutral `StreamBinding` while waiting for HCI route
ACK. Only `StreamRouteState::Bound` promotes that binding into confirmed
active state.

### RollbackOperation

Owns teardown queues/cursors and accumulated cleanup failures. The established
ordering remains:

```text
route
→ ISO path
→ CIS
→ ASE Release
→ CIG
→ ACL
→ privacy
```

Timeout/uncertain states preserve the previous fail-closed behavior.

## 4. Engine decomposition

The old `engine/machine.rs` mega implementation has been split into focused
Control-engine flow modules:

```text
engine/machine/
├── audio_flow.rs
├── connection_flow.rs
├── event_reducer.rs
├── report_reducer.rs
├── rollback_flow.rs
├── stream_flow.rs
└── tests.rs
```

Responsibilities:

- `connection_flow.rs`: startup/member connection helpers;
- `audio_flow.rs`: audio discovery, ASCS orchestration and streaming semantic
  verification glue;
- `stream_flow.rs`: codec/ISO/QoS/enable planning, CIG/CIS/path lifecycle and
  logical stream handoff;
- `rollback_flow.rs`: rollback/teardown orchestration;
- `event_reducer.rs`: timer/local Control events;
- `report_reducer.rs`: reliable lower semantic HCI reports;
- `tests.rs`: deterministic Control regression fixtures.

`engine/machine.rs` remains the central type/vocabulary + composition shell,
not the previous multi-thousand-line implementation bucket.

## 5. ControlPhase status

`ControlPhase` is **not** authoritative Control state and must not be renamed
1:1 to `current`.

After operation extraction it still serves as an internal Engine
cursor/checkpoint for two reasons:

1. active asynchronous steps need a precise expected-report/timer cursor;
2. the current not-yet-composed runtime exposes explicit stage APIs whose
   admission points include checkpoints such as `SetConnected`,
   `CigConfigured`, `CisEstablished` and `IsoPathsReady`.

Those checkpoints cannot be removed without changing execution semantics.

Therefore explicit current/target semantic state remains deferred until Core
runtime composition decides whether stage APIs become automatically chained.
At that point `ControlPhase` may be reduced or replaced based on evidence,
not by a mechanical 1:1 rename.

## 6. Condition boundary remains unchanged

The upward projection remains deliberately coarse:

```text
Confirmed/engine facts
        ↓ derive
ControlConditionUpdate
        ↓
Core Condition
        ↓
private Blackboard
```

Condition is a materialized read model. It does not become owner of Control's
connections, ASE status arrays, CIS set, operation cursors, rollback progress,
or planner state.

## 7. Frozen non-regression invariants

This decomposition does not change:

- Control transition ordering;
- stored-LTK/privacy behavior;
- ASCS semantic-success criteria;
- CIG/CIS/path lifecycle semantics;
- route ACK before StreamBinding handoff;
- raw CIS handle ownership below Control;
- route-first reverse teardown;
- fail-closed uncertain cleanup;
- Media time independence from HCI credit/completion.

## 8. Software evidence

Every bounded extraction was gated with:

```text
cargo test --workspace --locked
372 passed / 0 failed
```

Final milestone:

```text
cargo test --workspace --locked   PASS (372/372)
cargo check --workspace --locked  PASS
cargo build --workspace --locked  PASS
```

No HIL is required for this phase because the changes are ownership and source
decomposition only; no Control/HCI protocol ordering changed.

## 9. Next boundary

L1-Control is now decomposed enough for product runtime composition.

The next work should not reopen this ownership split. The next cross-layer
milestone is:

```text
Core Lifecycle
+ Command
+ Condition
+ timer owner
+ ControlPlane
+ HCI owner
+ Media owner
+ Transport owner
        ↓
composed runtime
```

Only after composed runtime behavior is explicit should the remaining
`ControlPhase` checkpoint/cursor model be reduced into any new
current/target semantic representation.
