# BE200AudioHub V2 Structural Refactor — Round 6 Handoff

Date: 2026-10-19  
Branch: `refactor/v2-architecture`

## 1. Authoritative checkpoints

Implementation/design checkpoint:

```text
c4b088a8  docs: freeze Round6 Core Condition architecture
```

Documentation landing marker:

```text
252aadd1  docs: mark V2 structural refactor landed
```

The latter updates plan/status text only. The source architecture described below is the state frozen by the Round 6 implementation series ending at `c4b088a8`.

Fresh software gate run after the Round 6 landing:

```text
cargo test --workspace --locked
372 passed / 0 failed

cargo check --workspace --locked
PASS

cargo build --workspace --locked
PASS
```

Working tree was clean and tracking `origin/refactor/v2-architecture`.

---

## 2. What Round 1–6 completed

The V2 structural refactor is complete.

Final source root:

```text
src/
├── main.rs
├── core/
├── plane/
├── runtime/
├── platform/
└── review/
```

Removed top-level/transitional ownership trees:

```text
src/architecture/
src/control/
src/hci_host/
src/media/
src/stream_clock/
src/iso_tx_bridge/
src/transport/
src/runtime/aggregate.rs
```

Removed transitional concepts:

```text
RuntimeState
RuntimeObservation
RuntimeAggregator
PlaneTelemetryAccumulator
```

There is no Shared / Common / SharedInfo / generic product Contract bucket.

---

## 3. Final ownership model

### L0 Core

```text
core/
├── command/
├── condition/
├── lifecycle/
└── timer/
```

Core owns orchestration-facing APIs, not Bluetooth/Media/Transport implementation state.

### L1-Control

Owns:

- LE Audio semantic planning;
- PACS/ASCS/CAS/CSIS/set semantics;
- codec/QoS/enable/ISO semantic planning;
- tested ControlMachine;
- logical stream contract;
- typed command/report boundary.

### L1-HCI

Owns:

- Bumble mutable Host/Device;
- ACL/security/GATT;
- CIG/CIS/path lifecycle;
- logical route -> raw CIS resolution;
- ISO packet framing/sequence/PB;
- controller credit/completion accounting;
- HCI producer telemetry.

### L1-Media

Owns:

- FrameId;
- LocalStreamClock;
- synthetic source/LC3;
- IntervalGroupScheduler;
- LogicalIsoTxBridge;
- Media producer telemetry.

### L2-Transport

Owns:

- async packet movement;
- reassembly;
- transport reactor/worker;
- transport health/counters.

### L3-Platform

Owns:

- raw backend/device/USB/firmware/platform details.

### runtime/

Owns cross-layer execution primitives only:

- generation fencing;
- cancellation;
- context execution;
- bounded mailbox primitive/policy;
- request/reply correlation;
- supervisor faults;
- generic TelemetryRing.

---

## 4. Command / Condition / Lifecycle freeze

### Command

```text
Command = Core write API
```

Current command vocabulary intentionally remains small:

```text
StartControl

BeginStreamHandoff {
    SessionId,
    StreamId,
}
```

Flow:

```text
CoreCommand
    -> Condition semantic query
    -> admission
    -> ControlPlane.handle_intent(...)
    -> Control owner validates actual state again
```

Command does not read Blackboard and does not manually reimplement admission by unpacking ConditionSnapshot fields.

### Condition

There is exactly one:

```text
Condition
    └── private Blackboard
```

Blackboard is a materialized read model only.

Planes do not import Blackboard types.

Condition accepts provider-owned typed updates and exposes:

- semantic query API;
- immutable value snapshot API.

### Lifecycle

Core/Lifecycle owns:

```text
SessionId
StreamId
LifecycleState
SessionState
LifecycleSnapshot
LifecycleModel
RecoveryClass
LifecycleMachine
```

StreamId is now physically owned by Core/Lifecycle but still semantically L0-supplied. Round 6 did **not** invent a new StreamId allocator/minting policy.

Lifecycle remains coarse:

```text
Stopped
Starting
Waiting
Running
Recovering
Stopping
```

It does not encode ASE/CIS/GATT/Media detail.

---

## 5. Condition stale fencing

Each Plane update is applied with:

```text
ConditionScope {
    generation,
    optional SessionId
}
```

Reducer invariants:

1. wrong-generation update is rejected as stale;
2. session-scoped update for a non-current session is rejected;
3. generation advance resets Plane read projections;
4. session change resets session-scoped projections;
5. identical effective read state does not churn Condition epoch.

These fences affect only the read model. They never mutate Plane operational truth.

---

## 6. Provider-owned ConditionUpdate schemas

### Control

```text
ControlConditionUpdate {
    ControlState,
    optional StreamId,
    optional LeAudioFormat
}
```

ControlMachine derives this from local authoritative state.

### HCI

State and telemetry are deliberately separated:

```text
HciConditionUpdate::State(HciConditionState)

HciConditionUpdate::Telemetry(HciIsoTelemetrySnapshot)

HciConditionUpdate::Final {
    state,
    iso_telemetry,
}
```

This prevents delayed/coalesced telemetry from replacing newer semantic HCI state.

### Media

Same pattern:

```text
MediaConditionUpdate::State(MediaConditionState)

MediaConditionUpdate::Telemetry(MediaBridgeTelemetrySnapshot)

MediaConditionUpdate::Final {
    state,
    bridge_telemetry,
}
```

### Transport

```text
TransportConditionUpdate {
    state,
    rx_packets,
    tx_packets,
    faults,
}
```

---

## 7. Telemetry contract frozen by R3/R4/R6

Three lanes remain distinct.

### Reliable semantic lane

Never silently drops semantic truth.

Examples:

- connection/security changes;
- ASE transitions;
- CIG/CIS/path transitions;
- route ACK;
- faults;
- lifecycle changes.

### Aggregate telemetry lane

High-rate producer-local data is accumulated locally first.

HCI examples:

- submitted/completed/retired/in-flight;
- peak in-flight;
- controller limit.

Media examples:

- submitted intervals;
- backpressure drops;
- fallback intervals;
- skipped intervals;
- stale-frame drops.

Transport examples:

- rx/tx/fault counters.

Condition receives only aggregate/coalesced snapshots.

### Raw diagnostic lane

Raw history remains in bounded producer-local TelemetryRing instances.

Raw trace record vectors do not enter Condition.

Condition sees only aggregate telemetry plus ring statistics.

### Publication semantics

State update:
- semantic change;
- intended for immediate publication.

Telemetry update:
- dirty/coalesced;
- bounded cadence chosen by later runtime composition.

Forced-final update:
- used for fault/teardown;
- contains latest semantic state + aggregate telemetry;
- must work even after dirty bit was already consumed.

---

## 8. Critical data-plane invariants still frozen

Exact product data path proven by Round7 First Sound HIL:

```text
PCM
-> valid LC3
-> absolute StreamClock
-> IntervalGroupScheduler
-> LogicalIsoTxBridge
-> HciLogicalIsoTx
-> logical route
-> CIS
-> BE200
-> Buds3 Pro
```

Non-negotiable invariants:

1. Media time never waits for HCI credits.
2. Number Of Completed Packets releases controller capacity only.
3. HCI completion never advances FrameId.
4. Media never sees raw CIS handles.
5. HCI owns ISO packet sequence/PB/fragmentation.
6. logical route remains session-fenced.
7. route ACK gates StreamBinding handoff.
8. stale media is dropped rather than retained into unbounded latency.
9. teardown remains route-first then path/CIS/ASE/CIG/ACL/privacy.
10. teardown retirement does not count as successful controller completion.

---

## 9. Control status after structural refactor

ControlMachine remains the proven mega-FSM.

Round 5 classified fields but Round 6 deliberately did not start a broad semantic rewrite.

Key conclusion:

> ControlPhase is not a valid future `current` state.

It currently mixes:

- semantic milestone;
- active operation;
- active member/side;
- timer/wait cursor;
- rollback/fault progress.

Recommended extraction order remains:

1. mechanically extract durable ControlModel facts;
2. extract RollbackOperation;
3. extract AudioDiscovery operation;
4. extract ASE operations;
5. extract StreamingGate;
6. extract StreamRouteBind;
7. isolate conservative cleanup knowledge;
8. only then introduce explicit current/target semantics.

Do not map every old ControlPhase 1:1 into current/target.

---

## 10. Recent Round 6 commit sequence

```text
6829a63b  core(condition): define provider-owned condition updates
2ea10d27  core(condition): add private Blackboard and reducer
13f450b6  fix(condition): correct module exposure and Blackboard visibility
f266ba49  core(command): add Condition-gated write facade
e3478848  refactor(condition): move active Control and clock projections to providers
2422bf8e  refactor(v2): retire architecture and RuntimeAggregator
8e29d474  refactor(core): switch remaining product identities to Lifecycle owner
01ea33c2  core(condition): separate state telemetry and forced-final updates
27274cca  runtime(condition): wire producer condition publication points
420440d9  test(condition): lock coalesced and forced-final publication
f4c88ee8  test(condition): preserve semantic state across telemetry merge
c4b088a8  docs: freeze Round6 Core Condition architecture
252aadd1  docs: mark V2 structural refactor landed
```

---

## 11. Regression evidence added in Round 6

Round 6 software tests lock:

- Lifecycle starts Stopped without session;
- identical Condition update does not churn epoch;
- stale generation update rejected;
- stale session update rejected;
- generation advance resets Plane read projection;
- Command admission uses Condition API;
- wrong-session command rejected before owner call;
- coalesced HCI telemetry cannot clobber newer semantic state;
- HCI telemetry dirty publication is one-shot/coalesced;
- HCI forced-final works after dirty consumed;
- Media telemetry dirty publication is one-shot/coalesced;
- Media forced-final works after dirty consumed;
- Transport fault projects Faulted immediately;
- Transport counters survive into ConditionUpdate;
- HCI teardown retirement remains retired != completed;
- Media backpressure still advances the absolute FrameId grid.

Current full suite at handoff:

```text
372 passed / 0 failed
```

---

## 12. What is NOT complete

V2 structural ownership is complete. Product composition is not.

Still deferred:

### Core runtime composition

- concrete startup ordering;
- concrete shutdown ordering;
- recovery escalation;
- actual CoreContext composition.

### Core timer owner

The architectural Command/Control timer directives exist, but the final L0 timer service/expiry/generation fencing composition is not yet complete.

### Telemetry scheduler

Provider APIs exist for:

- state update;
- dirty/coalesced telemetry update;
- forced-final update.

The actual bounded periodic cadence is not yet scheduled by Core runtime composition.

### MediaRt runtime owner

Data-plane algorithms exist and First Sound HIL passed, but the final MediaRt runtime owner/session/start-stop/report composition remains pending.

### Control semantic decomposition

ControlModel/operations/current-target decomposition remains pending.

### Full composed-runtime acceptance

The historical Round7 First Sound HIL proved the new product data plane but setup/teardown still reused the sealed Review2D orchestration oracle.

A new full composed-runtime software + HIL acceptance is still required after L0 composition.

### Future product work

- WASAPI Process Loopback;
- ASRC / clock discipline;
- optional controller-anchored clock;
- profiling/thread remap;
- product hardening/Daily Test.

---

## 13. Recommended next sequence

Do not reopen the structural directory refactor.

Recommended next implementation sequence:

### Post-V2-A — ControlModel extraction

Start with the smallest semantic extraction:

```text
ControlMachine {
    model: ControlModel,
    existing phase,
    existing operation fields...
}
```

Move only the durable confirmed facts classified in `CONTROL_OWNERSHIP_AUDIT_R5.md`.

Do not alter transition ordering.

Then extract RollbackOperation as the first bounded operation object.

### Post-V2-B — Core runtime composition

Compose:

```text
LifecycleMachine
Condition
Core Command
Core timer owner
Control owner
HCI owner
Media owner
Transport owner
```

Define exact startup/shutdown/recovery wiring with tests.

### Post-V2-C — bounded telemetry scheduling

Wire:

```text
state change -> immediate Condition apply
dirty telemetry -> bounded cadence apply
fault/teardown -> forced-final apply
```

Do not put raw trace into Core.

### Post-V2-D — full composed synthetic acceptance

Prove:

```text
Core Command
-> Lifecycle/Control
-> HCI
-> StreamBinding
-> Media
-> Bridge
-> logical ISO
```

in the composed runtime.

### Post-V2-E — real HIL

Re-run First Sound-style HIL through the composed runtime, not the old Review2D setup oracle.

Only after that continue WASAPI/ASRC.

---

## 14. Next-chat startup prompt

```text
继续 BE200AudioHub，实际仓库状态为准，不把 handoff 摘要当事实。

工作 branch：
refactor/v2-architecture

先核对：
- GitHub branch HEAD；
- 本地 git-status；
- cargo test/check/build 当前 gate；
- docs/design/CORE_CONDITION_COMMAND_R6.md；
- docs/design/CONTROL_OWNERSHIP_AUDIT_R5.md；
- docs/design/ROUND6_HANDOFF_2026-10-19.md。

V2 structural refactor 已结束，不要重新设计目录，不要恢复 architecture/shared/common。

下一步从 Post-V2 semantic/runtime work 开始。

优先任务：
1. 精读当前 ControlMachine 和 R5 audit；
2. 机械抽取 durable ControlModel，只移动字段，不改变 transition ordering；
3. 每个小 extraction 都跑 full cargo test；
4. ControlModel 绿后再考虑 RollbackOperation；
5. 不要同时做 Core runtime composition 和大规模 FSM rewrite。

冻结不变量：
- Command 只调 Condition API，不碰 Blackboard；
- Blackboard private；
- Plane operational truth stays in owner Plane；
- raw telemetry never enters Condition；
- Media time never waits for HCI credit/completion；
- Number Of Completed Packets never advances FrameId；
- route ACK still gates StreamBinding；
- teardown ordering unchanged。

多回报，小 commit，不要一次写大量代码。
```

---

## 15. Files to read first in the next chat

Primary:

```text
docs/design/IMPLEMENTATION_PLAN_V2.md
docs/design/CORE_CONDITION_COMMAND_R6.md
docs/design/CONTROL_OWNERSHIP_AUDIT_R5.md
docs/design/MEDIA_RUNTIME_OWNERSHIP_R4.md
docs/design/HCI_TELEMETRY_SCHEMA_R3.md
docs/design/HCI_OWNERSHIP_AUDIT_R2.md
```

Code:

```text
src/core/condition/
src/core/command/
src/core/lifecycle/
src/plane/l1_control/engine/machine.rs
src/plane/l1_control/model/
src/plane/l1_hci/host/engine.rs
src/plane/l1_media/bridge/runtime.rs
src/plane/l2_transport/reactor.rs
```

Historical evidence:

```text
review/review2/
review/round7/first_sound.rs
```

Do not treat historical Review/HIL code as product ownership.
