# Runtime / Execution Context Reference Index

## BE200AudioHub contract

Target modules:

- `src/runtime/context.rs`
- `src/runtime/mailbox.rs`
- `src/runtime/generation.rs`
- `src/runtime/cancellation.rs`
- `src/runtime/request.rs`
- `src/runtime/supervisor.rs`
- `docs/design/EXECUTION_MODEL.md`

Human-decided invariants:

- mutable subsystem state has one execution-domain owner;
- cross-context traffic is bounded and nonblocking at the producer;
- request/reply never becomes synchronous cross-thread waiting;
- stale generation/session work cannot mutate current state;
- stop/fault must wake idle contexts;
- physical thread placement is deployment policy, not module API.

## Primary oracle — AOSP media execution context usage

Project: AOSP Bluetooth

File: `system/bta/le_audio/audio_hal_client/audio_source_hal_client.cc`

### `SourceImpl::InitAudioSinkThread()`

Read for:

- dedicated serialized worker ownership;
- worker startup separated from media session state;
- explicit real-time scheduling enablement.

Adopt:

- modules receive/own an execution context;
- RT scheduling belongs to runtime deployment.

Do not copy:

- exact Android thread class or scheduler API.

### `SourceImpl::StartAudioTicks()`

Read for:

- periodic work scheduled onto the media worker;
- interval derived from stream configuration;
- ASRC constructed before periodic callbacks begin.

Adopt:

- fixed interval work belongs to the owning context;
- timer callback should enqueue/execute bounded work on that context.

### `SourceImpl::StopAudioTicks()`

Read for:

- timer cancellation and wait before destroying ASRC state.

Adopt:

- lifetime ordering: stop producer/timer before releasing state it may touch.

### `SourceImpl::OnSuspendReq()`

Read for:

- lifecycle request dispatch through `worker_thread_->DoInThread(...)`.

Adopt:

- external lifecycle callbacks do not directly mutate context-owned media state.

### `SourceImpl::Start()`, `SourceImpl::Stop()`, `SourceImpl::ConfirmStreamingRequest()`

Read together.

Key distinction:

- session/source is prepared by `Start()`;
- media ticks do not begin until stream confirmation;
- `Stop()` dispatches owned cleanup.

This is the main precedent for BE200AudioHub’s Prepare → Prime/Ready → Running boundary.

## Platform precedent — LE-Audio-Relay supervisor

Project: `STanJK/LE-Audio-Relay`

File: `Supervision/BackendSupervisor.cs`

### `BackendSupervisor.Start()`

Starts exactly one reconciliation loop and wakes it.

Reference for:

- one serialized lifecycle owner;
- coalesced wakeup rather than spawning work per event.

### `BackendSupervisor.RequestRestart()`

Increments restart revision, invalidates endpoint probe revision, publishes desired-state change, wakes loop.

Reference for:

- revision/generation invalidation;
- event callback changes desired state, not lower-plane objects.

### `BackendSupervisor.SetMode()`

Updates configuration plus restart revision under one gate.

Reference for:

- configuration snapshots associated with a revision.

### `BackendSupervisor.ReconcileLoopAsync()`

Main lifecycle reconciliation loop.

Read for:

- desired state vs observed reality;
- replacement of stale worker generations;
- recovery/backoff as loop state rather than nested blocking retry;
- stop old generation before installing a replacement.

Do not copy:

- process-worker architecture itself into every BE200AudioHub plane.

### `BackendSupervisor.OnEndpointTopologyChanged()`

Important comment in source:

- callback only invalidates revision and wakes supervision;
- it explicitly does **not** enumerate endpoints or run policy on the Windows callback thread.

Direct precedent for bounded foreign-thread callbacks.

### `BackendSupervisor.Wake()`

Uses a one-slot semaphore and intentionally coalesces wakeups.

Reference for:

- latest-state/reconcile notifications where N identical wakeups have no extra semantic value.

### `BackendSupervisor.WaitForWakeOrDelayAsync()`

Reference for:

- cancellation-aware backoff;
- wake event can preempt a recovery delay.

### `BackendSupervisor.GetRecoveryDelay()`

Maps failure count into bounded escalation table.

Reference for deterministic recovery-policy tests.

## Platform precedent — worker generation boundary

Project: LE-Audio-Relay

File: `Host/BackendWorker.cs`

### `BackendWorker.RunAsync()`

Read for:

- explicit generation carried across control protocol;
- startup handshake → running → failure/stopping/stopped lifecycle;
- cleanup in `finally`.

### `BackendWorker.WatchCommandsAsync()`

Read for:

- reject commands for stale generation;
- shutdown becomes cancellation, not arbitrary mutation from pipe reader.

### `BackendWorker.HeartbeatLoopAsync()`

Use only as telemetry precedent:

- aggregate counters periodically;
- do not route packet-rate telemetry into reliable control traffic.

## Deterministic test obligations

For any Runtime change, keep or add tests for:

1. serial execution order;
2. single-owner mutation;
3. producer does not block on full mailbox;
4. Full and Disconnected are distinguishable;
5. stale generation rejected before handler execution;
6. generation advance invalidates queued old work;
7. request/reply can complete out of order by correlation ID;
8. duplicate/unknown reply rejected;
9. cancellation idempotent;
10. idle context stop wakes promptly;
11. handler panic becomes supervisor fault;
12. supervisor fault mailbox cannot silently drop correctness faults;
13. wake/reconcile notifications can coalesce where contract says latest-state semantics.

## Non-oracles

Do not use these as justification for architecture changes:

- arbitrary `std::thread::spawn` in sample code;
- `Arc<Mutex<SubsystemState>>` because an upstream project uses shared objects;
- synchronous command/reply waiting across contexts.

If an upstream API requires one of those patterns, wrap or replace it at the adapter boundary.
