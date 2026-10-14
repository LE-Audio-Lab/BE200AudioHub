# Lifecycle / Generation / Recovery Reference Index

## BE200AudioHub contract

L0 owns:

- desired state;
- current generation/session;
- startup/teardown ordering;
- retry/backoff;
- recovery escalation;
- subsystem replacement.

L0 consumes coarse typed reports. It does not parse HCI packets or WASAPI callbacks.

Generation is a correctness boundary:

> late callbacks from an old generation are harmless and ignored.

## Primary product precedent — LE-Audio-Relay supervisor

Project: `STanJK/LE-Audio-Relay`

File: `Supervision/BackendSupervisor.cs`

### `UpdatePowerSnapshot()`

Updates observed power state, increments probe revision, wakes reconciliation.

Reference for observed-world revisioning.

### `Start()`

Creates one reconciliation loop and wakes it.

### `RequestRestart()`

Increments restart revision and publishes restart desired state.

Reference for manual restart as generation invalidation, not in-place lower-plane mutation.

### `SetMode()`

Updates desired configuration and restart revision under one owner.

### `ReconcileLoopAsync()`

Primary lifecycle oracle.

Read the following branches:

- suspended;
- endpoint observer unavailable;
- endpoint probe timeout/failure;
- endpoint absent;
- topology blocked;
- pending worker failure;
- stale generation;
- start new generation;
- worker completion;
- final shutdown.

Core pattern:

```text
snapshot desired
+ snapshot reality
→ decide one transition
→ execute bounded async action
→ return to reconcile loop
```

Adopt this shape for L0.

### stale generation predicate inside `ReconcileLoopAsync()`

Old worker becomes stale if:

- power revision changed;
- restart revision changed;
- configuration changed.

BE200AudioHub generalization:

- any session-defining desired-state revision change invalidates lower-plane generation.

### `OnEndpointTopologyChanged()`

The source comment explicitly forbids endpoint enumeration/policy in the Windows callback.

Adopt:

- foreign callback only records/invalidate/wakes.

### `StartGenerationAsync()`

Reference for:

- monotonically increasing generation ID;
- startup handshake;
- only expose generation as active after readiness is proven.

### `WaitForRunningAsync()`

Reference for startup as an explicit state with deadline and fault outcomes.

### `MonitorWorkerAsync()`

Reference for:

- heartbeat/fault/stopped reporting;
- generation filtering.

### `StopGenerationAsync()`

Strong teardown precedent:

- send courtesy shutdown request with bounded timeout;
- wait bounded time for completion;
- escalate to process kill if lower layer cannot stop;
- cleanup always completes.

BE200AudioHub recovery classes may use controller/context restart rather than process kill, but the bounded-escalation principle transfers.

### `WaitForWorkerOrWakeAsync()`

Allows desired-state wake to preempt waiting on current generation completion.

### `WaitForWakeOrDelayAsync()`

Recovery delay can be preempted by a meaningful new event.

### `GetRecoveryDelay()`

Maps consecutive failures into bounded backoff table:

1 s → 2 s → 5 s → 10 s → 30 s.

The exact numbers are not mandatory; the deterministic escalation model is.

### `Wake()`

Coalesces redundant wake events with one-slot semaphore.

Use when event semantics are “reconcile latest state”, not “process every edge”.

## Worker-generation precedent

Project: LE-Audio-Relay

File: `Host/BackendWorker.cs`

### `RunAsync()`

Lifecycle sequence:

`HELLO → route start → RUNNING → heartbeat/fault/command → STOPPING → STOPPED`

Reference for typed lifecycle reports and cleanup in `finally`.

### `WatchCommandsAsync()`

Rejects stale generation commands and maps Shutdown to cancellation.

### `HeartbeatLoopAsync()`

Reference for periodic aggregated health, not packet-rate logs.

## Route teardown precedent

File: `Routing/RouteSession.cs`

### `DisposeAsync()`

Ordering:

- mark stopping;
- disable producer/consumer boundary;
- stop capture;
- allow short settle;
- stop render;
- unsubscribe callbacks;
- dispose owned resources.

Transfer lesson:

- mark intentional shutdown before callbacks can report expected stop as fault.

## LE Audio semantic teardown cross-check — AOSP

Project: AOSP Bluetooth

File: `system/bta/le_audio/state_machine.cc`

Relevant anchors:

- `PrepareAndSendDisableToTheGroup()`
- `PrepareAndSendReleaseToTheGroup()`
- `ProcessHciNotifRemoveIsoDataPath()`
- `RemoveDataPathByCisHandle()`
- `ReleaseCisIds()`
- `RemoveCigForGroup()`
- `ProcessHciNotifAclDisconnected()`

Use to define Bluetooth semantic teardown once Control Plane is implemented.

## BE200AudioHub planned startup order

```text
L0/Core
→ Transport attach/start
→ HCI Host ready
→ Control prepares Bluetooth stream
→ Capture/Media prepare
→ StreamClock established
→ Bridge/HCI binding ready
→ Media prime
→ Media start
```

## Planned shutdown order

```text
L0/Core
→ invalidate generation / stop Media production
→ stop Bridge submissions
→ bounded ISO drain/cancel
→ Control/HCI semantic teardown
→ stop Capture
→ stop/release HCI/Transport as lifecycle requires
```

New generation invalidation occurs before waiting for old callbacks.

## Recovery ladder model

Keep recovery classes explicit. Example classes already in architecture:

- media restart;
- stream rebuild;
- ACL reconnect;
- controller restart;
- worker/process restart.

Test policy decides which fault maps to which class.

Do not let lower modules autonomously escalate outside their assigned class.

## Deterministic lifecycle tests

Build a fake subsystem world with scripted events/timers.

Required scenarios:

1. clean startup;
2. startup fault at every stage;
3. user stop during every startup stage;
4. generation change during in-flight operation;
5. late old-generation success callback;
6. late old-generation failure callback;
7. repeated fault/backoff escalation;
8. meaningful wake preempts backoff;
9. suspend → old generation invalid → resume/rebuild;
10. endpoint/controller absent;
11. subsystem stop timeout → escalation;
12. stop is idempotent;
13. one subsystem failure cannot leave RuntimeState claiming Running;
14. recovery never hides the original typed fault from observability.

## HIL boundary

HIL validates which external failures occur in practice and which recovery class is sufficient. The generation/reducer/retry state machine itself should be fully test-driven.
