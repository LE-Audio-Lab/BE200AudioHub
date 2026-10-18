
# Round 5 — L1 Control Ownership and Extraction Audit

Status: Round 5 structural migration handoff.

This note records the actual Control ownership after the V2 Round 5 migration and classifies the current ControlMachine fields for later semantic decomposition. Round 5 deliberately does not change transition ordering, timer behavior, rollback policy, failure semantics, or HIL protocol ordering.

## 1. Active L1-Control layout

- plane/l1_control/ascs/
  - operation.rs
  - streaming.rs
- plane/l1_control/discovery/
  - peer.rs
  - audio.rs
  - snapshot.rs
- plane/l1_control/engine/
  - facade.rs
  - machine.rs
  - step.rs
- plane/l1_control/interface/
  - command.rs
  - report.rs
  - dispatch.rs
  - stream.rs
- plane/l1_control/model/
  - set_plan.rs
- plane/l1_control/planning/
  - codec.rs
  - iso.rs
  - qos.rs
  - enable.rs

The old top-level src/control tree is removed.

The existing mega-FSM remains in plane/l1_control/engine/machine.rs. Its behavior was not redesigned during this round.

## 2. Control boundary split

The old flat boundary.rs is gone.

The boundary is now separated by direction:

- interface/command.rs: ControlIntent, Core/L0 -> Control.
- interface/report.rs: ControlLowerReport, lower semantic evidence -> Control.
- engine/step.rs: ControlStep, one nonblocking semantic transition result.
- interface/dispatch.rs: ControlAction -> HCI/Core timer dispatch.

No compatibility facade for the old boundary path remains.

## 3. Logical stream contract ownership

Round 5 establishes one physical owner for:

- AudioSide
- LeAudioFormat
- StreamTopology
- StreamBinding

Owner: plane/l1_control/interface/stream.rs.

The contract remains transport-neutral. It contains SessionId, StreamId, format, presentation delay, and logical topology. It contains no ASE handle, raw ACL/CIS handle, controller credit, HCI sequence/PB state, or USB detail.

Media and HCI consumers now import this language from the Control interface rather than architecture/control.rs or architecture/types.rs.

## 4. StreamId ownership audit

Round 5 intentionally does not move or remint StreamId.

Audited production flow:

ControlIntent::BeginStreamHandoff receives StreamId from above.
ControlMachine::begin_stream_handoff consumes it.
StreamBinding carries it.
HCI uses it as a logical route key.
Media carries it through LocalStreamClock and logical ISO requests.

No audited production path constructs a new StreamId value. Construction remains in tests/Review fixtures.

Therefore the semantic contract remains: StreamId is supplied by L0 orchestration. L1-Control defines how it participates in the stream handoff but does not mint it.

Final physical ownership is deferred until Core/Lifecycle composition confirms the minting point.

## 5. ControlMachine field classification

### 5.1 Durable Control Model candidates

The following fields describe semantic facts or configuration that survive individual callback/transaction boundaries:

- plan
- resolved_members
- connections
- encryption_key_sizes
- privacy_active
- audio_set
- codec_plan
- prepared_ases
- codec_statuses
- iso_plan
- qos_plan
- qos_statuses
- enable_plan
- enable_statuses
- live_cis
- iso_paths_ready
- streaming_statuses
- active_stream_binding

Notes:

- connections are generation-fenced ConnectionToken values. Control does not make policy from raw ACL handles.
- prepared_ases are confirmed preparation facts, distinct from the temporary AsePrepareMachine.
- codec/qos/enable/streaming status arrays are confirmed readback/evidence.
- active_stream_binding exists only after HCI route ACK and is therefore confirmed stream truth.

### 5.2 Current/target semantic-state candidates

The current implementation has no explicit general target-state field.

ControlPhase currently mixes:
- semantic milestone;
- active asynchronous operation;
- active member/side;
- rollback/fault cursor.

Therefore ControlPhase must not simply be renamed to current.

A future current semantic state should be much coarser, around facts such as:
Idle, SetConnected, AudioDiscovered, CodecConfigured, CigConfigured, QosConfigured, Enabled, CisEstablished, PathsReady, Streaming, StreamBound, Faulted.

Exact enum naming is deferred until operations are extracted.

The future target is currently implicit in the command/operation being executed. pending_stream_binding is only the target payload of the route-bind operation; it is not a general Control target state.

### 5.3 Engine cursor and temporary operation state

These fields primarily describe in-progress work rather than durable truth:

- phase
- next_gatt_request_id
- audio_discovery
- audio_members
- ase_prepare
- ase_operation
- streaming_gate
- pending_stream_binding

audio_members is incremental discovery accumulation; audio_set is the completed durable result.

### 5.4 Conservative cleanup/safety knowledge

These fields are not equivalent to confirmed happy-path state:

- cig_may_exist
- iso_paths_may_exist

They exist because timeout/uncertain-controller outcomes require fail-closed cleanup. They should eventually live in recovery/cleanup safety state rather than user-facing current semantic state.

### 5.5 Rollback-only state

The following form one obvious future RollbackOperation:

- rollback_queue
- rollback_current
- cleanup_failures
- rollback_cig_pending
- rollback_stream_route
- rollback_stream_route_unbind_pending
- rollback_iso_path_queue
- rollback_iso_path_current
- rollback_cis_queue
- rollback_cis_current
- rollback_ase_queue
- rollback_ase_current
- rollback_ase_operation

These should not remain individual top-level fields in the final ControlModel.

### 5.6 Fault state

failure has two roles today:
1. retain the primary failure while rollback is in progress;
2. retain the final failure after rollback, possibly wrapped as RollbackFailed with cleanup failures.

Future extraction should separate terminal Control fault state from RollbackOperation primary/cleanup error storage without changing the existing failure values.

### 5.7 Read-model-only state

There are no dedicated ControlMachine fields whose only purpose is ControlSnapshot.

snapshot() derives the old coarse projection from:
- phase
- active_stream_binding or pending_stream_binding

Therefore ControlSnapshot is a read projection, not authoritative Control model storage. It should migrate into the future Condition/Blackboard path.

## 6. Recommended extraction order

C1 — Introduce a ControlModel container.
Move only durable confirmed facts into ControlModel. Keep ControlMachine with model + existing phase/operation fields. Preserve every transition.

C2 — Extract RollbackOperation.
Move rollback queues/current items, cleanup failures, pending cleanup flags, and in-progress primary failure. Preserve route-first cleanup ordering: route -> path -> CIS -> ASE -> CIG -> ACL -> privacy.

C3 — Extract AudioDiscovery operation.
Move audio_discovery, audio_members, and associated progress. Keep completed audio_set in ControlModel.

C4 — Extract ASE preparation/control operations.
Move ase_prepare and ase_operation plus temporary side/request progress. Keep confirmed prepared_ases and status arrays in ControlModel.

C5 — Extract StreamingGate operation.
Move streaming_gate and retry/deadline progress. Keep confirmed streaming_statuses in ControlModel.

C6 — Extract StreamRouteBind operation.
Move pending_stream_binding and route-bind progress. Only route ACK may promote a binding into model.active_stream_binding.

C7 — Separate conservative cleanup knowledge.
Move cig_may_exist and iso_paths_may_exist into explicit recovery/safety state once rollback has a stable owner.

C8 — Only then introduce explicit current/target semantics.
After temporary operation progress is no longer encoded in the top-level phase, add ControlModel.current, ControlModel.target, and a separate active operation. Do not map every old ControlPhase 1:1.

## 7. Remaining src/architecture ownership audit

architecture/control.rs:
Contains old ControlState/ControlSnapshot read projection.
Disposition: Core/Condition Control projection. Not future Control operational model.

architecture/hci_host.rs:
Contains old coarse HCI state/snapshot.
Disposition: provider-owned HCI ConditionUpdate + Core/Condition projection.

architecture/iso_tx_bridge.rs:
Contains old bridge architecture declarations.
Disposition: superseded by L1-Media ownership; remove when architecture is retired.

architecture/lifecycle.rs:
Contains LifecycleState, SessionState, RecoveryClass, LifecycleSnapshot.
Disposition: Core/Lifecycle, with a Condition projection where appropriate.
This is also the strongest current candidate owner for SessionId.

architecture/mailbox.rs:
Contains MessageScope, Envelope, SOURCE_PCM_RING, TARGET_FRAME_STORE.
MessageScope/Envelope should be assigned only if real Core composition proves they are needed, otherwise delete.
SOURCE_PCM_RING/TARGET_FRAME_STORE describe Media-specific latency/drop policy and should move with real capture/source buffering if retained.

architecture/media.rs:
Contains old Media state/snapshot.
Disposition: L1-Media provider ConditionUpdate + Core/Condition projection, not authoritative Media storage.

architecture/platform.rs:
Contains old platform/backend/bootstrap declarations.
Disposition: L3 Platform model/observation if still useful; otherwise remove.

architecture/runtime_state.rs:
Contains old RuntimeState.
Disposition: replaced by private Core/Condition Blackboard in Round 6.

architecture/stream_clock.rs:
Contains ClockLock and StreamClockSnapshot.
Disposition: L1-Media clock/condition projection. Authoritative LocalStreamClock already lives in L1-Media.

architecture/transport.rs:
Contains old Transport state/snapshot and packet-class declaration.
Disposition: Transport projection -> L2 provider ConditionUpdate. Packet-class concept -> L2 packet domain only if still required.

architecture/types.rs:
Now contains only SessionId and StreamId.
SessionId is likely Core/Lifecycle identity.
StreamId remains L0-supplied and intentionally unresolved until Core composition confirms the minting point.
This file must not evolve back into a shared-type bucket.

## 8. Round 5 non-regression invariants

Round 5 preserves:
- exact Control transition ordering;
- ASCS success = write response + CP success + ASE notification + readback;
- stored-LTK/privacy behavior;
- semantic CIG/CIS/path lifecycle;
- route ACK before StreamBinding handoff;
- no raw CIS handles in StreamBinding;
- rollback ordering;
- no synchronous waits in ControlMachine;
- Media time independence from HCI credits/completions.

## 9. Round 5 conclusion

L1-Control now owns the semantic implementation and logical stream contract and exposes directional command/report interfaces.

ControlMachine remains a proven mega-FSM, but its field ownership is now classified. The next semantic refactor should begin with a mechanical ControlModel container extraction, not a state-machine rewrite.


---

## 10. Post-Round8 implementation status

The extraction plan above has now been executed after the V2 structural
refactor. The original `ControlModel` name was intentionally not retained as
the concrete type name because the audit categories proved more precise when
kept separate.

Landed ownership:

```text
model/confirmed_status.rs
    ConfirmedStatus

model/planned_configuration.rs
    PlannedConfiguration

model/cleanup_knowledge.rs
    CleanupKnowledge

operation/audio_discovery.rs
operation/ase_control.rs
operation/streaming.rs
operation/stream_route.rs
operation/rollback.rs
```

The Engine implementation was also physically decomposed into connection,
audio, stream, rollback, event-reducer and report-reducer flows. Regression
tests were separated from the production machine source.

All bounded extraction gates remained:

```text
cargo test --workspace --locked
372 passed / 0 failed
```

Final check/build also pass.

The only intentionally deferred item from the old C8 recommendation is explicit
`current/target` semantic state. `ControlPhase` still has real execution
semantics as the admission/cursor for explicit stage APIs. It therefore remains
an internal Engine cursor/checkpoint until Core runtime composition determines
whether those stages become automatically chained.

The authoritative current description is
`docs/design/CONTROL_DECOMPOSITION_POST_R8.md`.
