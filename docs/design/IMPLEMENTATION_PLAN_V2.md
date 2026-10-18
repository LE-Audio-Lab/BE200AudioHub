# BE200AudioHub Implementation Plan V2

Status: **Frozen implementation direction / migration plan**

Current landing status (2026-10-19): **V2 structural refactor landed on `refactor/v2-architecture`.**

Post-Round8 Control decomposition status: **LANDED**. The earlier implementation
term `ControlModel` is now superseded by the concrete landed ownership split:

```text
ConfirmedStatus       confirmed Control facts
PlannedConfiguration  selected-but-not-yet-confirmed plans
CleanupKnowledge      conservative uncertain-cleanup facts
operation/*           temporary cross-event progress
engine/machine/*      transition/reducer/orchestration flows
```

`ControlPhase` remains an internal Engine cursor/checkpoint for the current
explicit stage APIs; it is not authoritative Control state and must not be
renamed 1:1 to `current`. Explicit current/target semantics remain deferred
until Core runtime composition. See
`docs/design/CONTROL_DECOMPOSITION_POST_R8.md`.

Verified branch state at handoff:

```text
HEAD c4b088a8  docs: freeze Round6 Core Condition architecture
cargo test --workspace --locked   372 passed / 0 failed
cargo check --workspace --locked  PASS
cargo build --workspace --locked  PASS
```

Rounds 1–6 completed the intended source-ownership migration:

- source root reduced to `main.rs / core / plane / runtime / platform / review`;
- `src/architecture/` and transitional `RuntimeState / RuntimeAggregator / RuntimeObservation` are removed;
- Core/Lifecycle owns SessionId/StreamId and coarse lifecycle state;
- Core/Condition owns one private Blackboard and the only Core read API;
- Core/Command queries Condition and then invokes the real owner;
- each Plane owns its operational truth and provider-side ConditionUpdate vocabulary;
- HCI/Media high-rate telemetry remains producer-local and only publishes aggregate/coalesced or forced-final updates.

This landing does **not** mean the final product runtime is composed. Control ownership/operation decomposition has now landed. Remaining work begins with concrete Core startup/shutdown/recovery composition, timer ownership, bounded telemetry scheduling, MediaRt composition, and then full composed-runtime software/HIL acceptance.

This document supersedes the implementation-direction portions of the earlier
Review architecture documents where they conflict with the decisions frozen
here. Historical Round documents and Experiment evidence remain historical
records and are not rewritten by this plan.

The purpose of V2 is not to redesign the proven Bluetooth/LE Audio protocol
path. It is to reorganize ownership, state, command/read boundaries,
observability, and source layout so the product implementation can continue
without growing a global state machine or flat source tree.

---

## 0. What V2 changes

V2 freezes five architectural corrections:

1. **Command and Condition are the two Core-facing API directions.**
   - Command is the top-down write/intent path.
   - Condition is the bottom-up read/fact path.

2. **There is no global detailed state machine.**
   - Core/Lifecycle owns only coarse whole-runtime lifecycle.
   - Each Plane owns its own authoritative operational model and local
     transition/transaction machinery.

3. **Condition owns one private readonly Blackboard.**
   - The Blackboard is a materialized read model.
   - It is not operational truth and is never mutated by Command or a Plane.
   - Command queries Condition APIs; it never receives direct Blackboard
     access.

4. **There is no SharedInfo / Shared / Contract / Common bucket.**
   - A type follows the semantic owner of the concept it represents.
   - Being used by multiple modules is not a reason to create a shared bucket.

5. **Telemetry is producer-local first.**
   - Raw diagnostics stay in bounded per-Plane rings.
   - High-rate statistics are accumulated at the producer event site.
   - Only coalesced Plane observations enter Condition.
   - Condition reduces observations into the Blackboard; it does not process
     raw telemetry.

---

# 1. Frozen vertical architecture

The architectural levels remain:

```text
L0  Core
L1  Control
L1  Media
L1  HCI
L2  Transport
L3  Platform
```

Source-level names use Rust-friendly identifiers:

```text
src/
├── core/
├── plane/
│   ├── l1_control/
│   ├── l1_media/
│   ├── l1_hci/
│   └── l2_transport/
├── runtime/
├── platform/
└── review/
```

Documentation may display the layers as:

```text
L0-Core
L1-Control
L1-Media
L1-HCI
L2-Transport
L3-Platform
```

The layer number is part of the Plane directory name so the physical tree
retains architectural position.

---

# 2. Dependency direction

The default dependency direction is:

```text
             Core
              │
              ▼
       L1 ownership layer
     ┌────────┼────────┐
     ▼        ▼        ▼
 Control    Media     HCI
                      │
                      ▼
                 L2 Transport
                      │
                      ▼
                 L3 Platform

Core / Planes / Transport
          │
          └──────────────► runtime primitives
```

`runtime/` is a lower-level execution substrate, not L0 product
orchestration.

Forbidden convenience dependencies include:

```text
L1-HCI        -> Core/Condition/Blackboard internals
L1-Media      -> Core/Condition internals
L2-Transport  -> Core business state
Platform      -> HCI or LE Audio semantics
Command       -> Blackboard fields directly
Condition     -> mutable Plane implementation objects
```

---

# 3. Core model

Target structure:

```text
core/
├── mod.rs
├── command/
├── condition/
├── lifecycle/
└── timer/
```

Core is product orchestration. It does not become a new home for Bluetooth,
HCI, media, or transport implementation detail.

---

# 4. Command — top-down write path

## 4.1 Responsibility

Command represents:

> what an upper layer wants the running system to do.

Examples eventually include:

- start runtime;
- stop runtime;
- start a stream;
- stop a stream;
- reconnect/recover;
- future UI/product actions.

Command is not a global mutable state owner.

## 4.2 Read-before-write rule

Command never reads the Blackboard directly.

Correct:

```text
Command
   │
   ├── query ──► Condition API
   │
   └── issue ──► real owner API
```

Incorrect:

```text
Command -> &Blackboard
Command -> mutable RuntimeState
Command -> raw Plane internals
```

## 4.3 Admission

Command may use Condition for coarse admission and routing:

```text
condition.can_start_stream(...)
condition.can_stop_stream(...)
condition.runtime_status(...)
condition.stream_status(...)
```

However, the target Plane remains responsible for final invariant validation at
execution time.

A Condition result is an observation, not a lock or transactional reservation.

## 4.4 Target structure

```text
core/command/
├── mod.rs
├── interface.rs
├── admission.rs
├── dispatch.rs
└── types.rs
```

Initial migration must not invent lifecycle commands that do not yet exist.
Existing proven semantics are wrapped first; product command vocabulary grows
only when the L0 lifecycle composition is implemented.

---

# 5. Condition — bottom-up read path

## 5.1 Responsibility

Condition represents:

> what the system has actually observed to be true.

There is exactly one Core Condition instance and exactly one private
Blackboard for the composed runtime.

```text
Core
└── Condition
    └── private Blackboard
```

Planes do not own their own Condition objects.

## 5.2 Blackboard rule

The Blackboard is:

- read-only outside Condition;
- a materialized system read model;
- updated only from typed observations;
- generation/session aware where necessary;
- suitable for snapshot/query APIs;
- not a container for mutable subsystem objects.

The Blackboard is **not** the operational database for Control, HCI, Media, or
Transport.

## 5.3 Condition API

External consumers use Condition APIs:

```text
Command -> semantic Condition queries
UI/CLI  -> Condition snapshot
Logger  -> low-rate Condition snapshot / meaningful transition
```

The Blackboard itself remains private.

## 5.4 Target structure

```text
core/condition/
├── mod.rs
├── interface/
│   ├── mod.rs
│   └── snapshot.rs
├── reducer/
│   └── mod.rs
└── blackboard/
    ├── mod.rs
    ├── lifecycle.rs
    ├── control.rs
    ├── hci.rs
    ├── media.rs
    └── transport.rs
```

The current names `RuntimeState`, `RuntimeObservation`, and
`RuntimeAggregator` are transitional and will be removed as V2 lands.

Preferred V2 concepts are:

```text
Condition
Blackboard
ConditionReducer
<Plane>ConditionUpdate / <Plane>Observation
ConditionSnapshot
```

Exact public Rust type names are frozen only when their module is migrated.

---

# 6. No global detailed state machine

There is no V2 type equivalent to a giant:

```text
GlobalPhase {
    Attaching,
    Scanning,
    Connecting,
    Securing,
    DiscoveringAudio,
    ConfiguringCodec,
    ConfiguringQos,
    Enabling,
    CreatingCis,
    SettingPath,
    Streaming,
    ...
}
```

The system instead has:

```text
Core/Lifecycle       coarse product lifecycle
L1-Control Model     LE Audio semantic state
L1-HCI Model         controller/host state
L1-Media Model       media runtime state
L2-Transport Model   packet transport state
local operations     detailed temporary transaction progress
```

---

# 7. Core/Lifecycle

Lifecycle owns only whole-runtime/session orchestration.

Target coarse lifecycle:

```text
Stopped
Starting
Waiting
Running
Recovering
Stopping
```

Responsibilities:

- desired product state;
- ordered startup/shutdown;
- session lifetime;
- retry/backoff;
- recovery escalation;
- generation advancement/requested invalidation;
- worker supervision policy.

Lifecycle does not contain detailed ASE/CIS/GATT phases.

Target structure:

```text
core/lifecycle/
├── mod.rs
├── model.rs
├── identity.rs
├── machine.rs
└── recovery.rs
```

The existing `SessionState` is reviewed during migration. If it is only a
coarse L0 product/session state it stays here. Detailed stream transition
progress belongs to L1-Control or the relevant local operation.

---

# 8. Runtime identities — no shared bucket

`architecture/types.rs` must be split by semantic ownership.

Initial ownership decision:

- `Generation`: runtime generation-fencing primitive; move toward
  `runtime/generation/`.
- `SessionId`: Core/Lifecycle identity.
- `FrameId`: L1-Media/Clock identity.
- `AudioSide`: logical stream topology vocabulary; move with the L1-Control
  stream interface.
- `StreamId`: requires a focused identity audit before final physical move.

The current code says L0 supplies/mints `StreamId` while Control exports and
HCI/Media consume it. V2 migration must preserve that semantic first. Do not
change who mints StreamId merely to make the directory tree look cleaner.

No `shared/types.rs` replacement is allowed.

---

# 9. Plane ownership pattern

A Plane may use the following conceptual subdivisions:

```text
Model
    authoritative local operational state

Engine
    transition logic operating on the Model

Operation / Transaction
    temporary progress for one asynchronous operation

Interface
    typed boundary exposed to other owners

Telemetry
    producer-local diagnostics and high-rate aggregation
```

These are patterns, not mandatory empty folders. A folder is created when the
Plane has enough concrete content to justify it.

---

# 10. L1-Control

## 10.1 Ownership

L1-Control owns high-level Bluetooth / LE Audio semantics:

- coordinated-set/device semantics;
- PACS / ASCS / CAS / CSIS;
- codec selection;
- QoS planning;
- ASE semantic state;
- semantic CIG/CIS lifecycle requests;
- ISO-path orchestration requests;
- logical stream readiness;
- logical stream binding.

It does not own raw ACL/CIS handles, HCI credits, HCI packet framing, USB, PCM,
or media time.

## 10.2 State model direction

The current `ControlMachine` is a proven semantic implementation but is too
program-phase-centric to remain the permanent architecture.

V2 direction:

```text
ControlModel
├── current semantic state
├── target semantic state
├── active logical stream
├── discovered set/audio facts
└── durable semantic configuration

ControlEngine
├── consumes commands + reliable reports
├── evaluates Model
└── starts/continues local operations

Operations
├── discovery operation
├── ASCS operation
├── streaming gate
├── rollback/teardown operation
└── other bounded transactions
```

Detailed waits such as:

- ATT Write Response;
- ASE Control Point response;
- ASE notification;
- independent readback;
- CIS completion;

remain operation/transaction state, not top-level Control state.

## 10.3 Initial migration structure

```text
plane/l1_control/
├── mod.rs
├── model/
├── engine/
├── operation/
├── discovery/
├── ascs/
├── planning/
└── interface/
```

## 10.4 Current file mapping

Move-only phase first:

```text
control/machine.rs
  -> plane/l1_control/engine/machine.rs

control/plane.rs
  -> plane/l1_control/engine/facade.rs
  (temporary; later replaced by ControlModel + ControlEngine composition)

control/boundary.rs
  -> split:
     Control intents         -> plane/l1_control/interface/command.rs
     Control lower input     -> plane/l1_control/interface/report.rs
     Control transition step -> plane/l1_control/engine/step.rs

control/dispatch.rs
  -> plane/l1_control/interface/dispatch.rs

control/discovery.rs
  -> plane/l1_control/discovery/peer.rs

control/audio_discovery.rs
  -> plane/l1_control/discovery/audio.rs

control/audio_snapshot.rs
  -> plane/l1_control/discovery/snapshot.rs

control/ascs_control.rs
  -> plane/l1_control/ascs/operation.rs

control/streaming_gate.rs
  -> plane/l1_control/ascs/streaming.rs

control/codec_plan.rs
  -> plane/l1_control/planning/codec.rs

control/qos_plan.rs
  -> plane/l1_control/planning/qos.rs

control/enable_plan.rs
  -> plane/l1_control/planning/enable.rs

control/iso_plan.rs
  -> plane/l1_control/planning/iso.rs

control/plan.rs
  -> plane/l1_control/model/set_plan.rs
```

The first directory move does not split the 3800+ line machine semantically.
Behavioral decomposition is a later gated phase.

---

# 11. L1-Control stream interface

The current `architecture/control.rs` mixes read-model data and stream
handoff contracts. It must be split.

The following belong to the L1-Control stream interface because Control defines
the logical stream handoff:

```text
LeAudioFormat
StreamTopology
StreamBinding
AudioSide
```

Target:

```text
plane/l1_control/interface/
├── mod.rs
├── command.rs
├── report.rs
├── dispatch.rs
└── stream.rs
```

`StreamBinding` remains transport-neutral and must continue to exclude:

- ASE handles/IDs used only internally;
- raw ACL handles;
- raw CIS handles;
- controller credits;
- HCI sequence/PB information;
- USB details.

The existing route-ack gate remains unchanged.

---

# 12. L1-HCI

## 12.1 Ownership

L1-HCI owns controller-facing Bluetooth Host state:

- Bumble mutable Device/Host state;
- HCI command/event semantics;
- ACL connection/security state;
- ATT/GATT transactions;
- CIG/CIS;
- ISO paths;
- logical route to raw CIS resolution;
- ISO packet sequence/PB/fragmentation;
- controller credit/completion accounting.

## 12.2 Target structure

```text
plane/l1_hci/
├── mod.rs
├── model/
├── host/
├── acl/
├── gatt/
├── iso/
├── interface/
└── telemetry/
```

## 12.3 Current file mapping

```text
hci_host/engine.rs
  -> plane/l1_hci/host/engine.rs

hci_host/worker.rs
  -> plane/l1_hci/host/worker.rs

hci_host/mailbox.rs
  -> split toward plane/l1_hci/interface/
     command.rs
     report.rs
     iso_tx.rs
     mailbox.rs
  Initial migration may keep one mailbox.rs to avoid semantic edits.

hci_host/link.rs
  -> plane/l1_hci/host/link.rs

hci_host/stack.rs
  -> plane/l1_hci/host/stack.rs

hci_host/connection.rs
  -> plane/l1_hci/acl/connection.rs

hci_host/acl_credit.rs
  -> plane/l1_hci/acl/credit.rs

hci_host/privacy.rs
  -> plane/l1_hci/acl/privacy.rs

hci_host/security.rs
  -> plane/l1_hci/acl/security.rs

hci_host/scan.rs
  -> plane/l1_hci/acl/scan.rs

hci_host/gatt.rs
  -> plane/l1_hci/gatt/transaction.rs

hci_host/iso.rs
  -> plane/l1_hci/iso/cig_cis.rs

hci_host/iso_lifecycle.rs
  -> plane/l1_hci/iso/lifecycle.rs

hci_host/iso_path.rs
  -> plane/l1_hci/iso/path.rs

hci_host/iso_credit.rs
  -> plane/l1_hci/iso/credit.rs

hci_host/iso_tx.rs
  -> plane/l1_hci/iso/tx.rs

hci_host/iso_host_feature.rs
  -> plane/l1_hci/iso/feature.rs

hci_host/stream_route.rs
  -> plane/l1_hci/iso/route.rs

hci_host/smoke.rs
  -> review/support/hci_smoke.rs or removed if only historical support
```

The route table remains HCI-owned. Condition may expose a coarse route condition
but never owns or mutates route entries.

---

# 13. L1-Media

## 13.1 Ownership

L1-Media owns the source/media clock domain:

```text
WASAPI source
-> source buffer
-> clock discipline / ASRC
-> target-domain PCM
-> channel map
-> LC3
-> encoded logical stereo frame
-> interval scheduling / logical ISO bridge
```

Media never owns raw CIS handles, HCI credits, or USB.

## 13.2 Bridge ownership correction

V2 assigns the current IsoTxBridge implementation to L1-Media.

Reason:

- it runs on the Media realtime scheduling domain;
- it consumes Media-owned FrameId/StreamClock semantics;
- it must not wait on HCI credit;
- it emits logical ISO work to the HCI interface.

It remains logically replaceable, but it is no longer a top-level
ownership-less architectural component.

## 13.3 Target structure

```text
plane/l1_media/
├── mod.rs
├── model/
├── source/
├── codec/
├── clock/
├── bridge/
├── capture/
├── interface/
└── telemetry/
```

## 13.4 Current mapping

```text
media/lc3.rs
  -> plane/l1_media/codec/lc3.rs

media/synthetic.rs
  -> plane/l1_media/source/synthetic.rs

stream_clock/mod.rs
  -> plane/l1_media/clock/local.rs
  (later split model/discipline as needed)

iso_tx_bridge/mod.rs
  -> plane/l1_media/bridge/scheduler.rs

iso_tx_bridge/runtime.rs
  -> plane/l1_media/bridge/runtime.rs

architecture/media.rs::EncodedStereoFrame
  -> plane/l1_media/interface/frame.rs

architecture/types.rs::FrameId
  -> plane/l1_media/clock/frame.rs
```

Future WASAPI Process Loopback enters:

```text
plane/l1_media/capture/
```

Future ASRC/discipline remains in the Media clock/processing domain.

---

# 14. L2-Transport

## 14.1 Ownership

L2-Transport owns:

- asynchronous HCI packet movement;
- endpoint completion servicing;
- packet boundary/reassembly;
- cancellation/detach;
- transport backpressure;
- transport faults.

It does not interpret Bluetooth semantic state or controller credits.

## 14.2 Target structure

Do not over-folder this Plane prematurely.

Initial target:

```text
plane/l2_transport/
├── mod.rs
├── reactor.rs
├── worker.rs
├── sink.rs
├── packet/
│   ├── mod.rs
│   └── reassembly.rs
├── interface/
└── telemetry/
```

Mapping:

```text
transport/reactor.rs
  -> plane/l2_transport/reactor.rs

transport/worker.rs
  -> plane/l2_transport/worker.rs

transport/sink.rs
  -> plane/l2_transport/sink.rs

transport/packet.rs
  -> plane/l2_transport/packet/mod.rs

transport/reassembly.rs
  -> plane/l2_transport/packet/reassembly.rs
```

`HciPacketClass` belongs with L2 packet semantics, not Condition.

---

# 15. L3-Platform

Platform remains a top-level implementation domain because it is beneath
Transport rather than another behavioral Plane.

Target:

```text
platform/
├── mod.rs
├── backend/
│   ├── mod.rs
│   └── test.rs
├── usb/
│   ├── mod.rs
│   └── libusb_async.rs
├── windows/
│   ├── mod.rs
│   └── be200.rs
└── firmware/
    ├── mod.rs
    ├── image.rs
    ├── loader.rs
    └── port.rs
```

Mapping:

```text
platform/backend.rs
  -> platform/backend/mod.rs

platform/test_backend.rs
  -> platform/backend/test.rs

platform/libusb_async.rs
  -> platform/usb/libusb_async.rs

platform/windows_be200.rs
  -> platform/windows/be200.rs

platform/be200_firmware.rs
  -> platform/firmware/image.rs

platform/be200_firmware_loader.rs
  -> platform/firmware/loader.rs

platform/be200_firmware_port.rs
  -> platform/firmware/port.rs
```

No HCI semantic logic may move downward into Platform during this cleanup.

---

# 16. Runtime substrate

`runtime/` is retained and clarified as cross-layer execution infrastructure.

Target:

```text
runtime/
├── mod.rs
├── generation/
├── context/
├── mailbox/
├── request/
├── supervisor/
└── telemetry/
    └── ring.rs
```

Current mapping:

```text
runtime/cancellation.rs
  -> runtime/context/cancellation.rs or runtime/cancellation/mod.rs

runtime/context.rs
  -> runtime/context/mod.rs

runtime/generation.rs
  -> runtime/generation/mod.rs

runtime/mailbox.rs
  -> runtime/mailbox/mod.rs

runtime/request.rs
  -> runtime/request/mod.rs

runtime/supervisor.rs
  -> runtime/supervisor/mod.rs

runtime/telemetry.rs
  -> runtime/telemetry/ring.rs
```

The generic `TelemetryRing<T>` remains here.

---

# 17. Runtime mailbox architecture contract cleanup

The current `architecture/mailbox.rs` contains both generic delivery semantics
and domain-specific MessageScope identity.

V2 must separate them.

Generic runtime concepts belong in:

```text
runtime/mailbox/
├── mod.rs
├── contract.rs
└── envelope.rs
```

Generic concepts:

- DeliveryClass;
- OverflowAction;
- MailboxContract;
- reliable/coalesced/realtime delivery rules.

The current `MessageScope { generation, session_id, stream_id }` is not allowed
to force runtime primitives to depend on a global shared identity module.

During migration:

1. preserve behavior;
2. audit every current Envelope use;
3. either make Envelope scope-generic or move product-specific scope ownership
   upward;
4. remove the architecture/types dependency only after tests prove equivalent
   generation/session fencing.

---

# 18. Telemetry architecture

## 18.1 Three lanes remain separate

### Reliable semantic lane

Examples:

- connection/security transition;
- ASE transition;
- CIG/CIS/path transition;
- route ACK;
- fault;
- lifecycle transition.

Properties:

- bounded;
- producer non-blocking;
- mailbox-full is a fault;
- never silently drop or coalesce away.

### Aggregated telemetry lane

Examples:

- submitted/completed/in-flight;
- queue depths;
- ring fill;
- deadline miss;
- encode/submit latency summaries.

Properties:

- producer-local accumulation;
- coalesced Condition publication;
- never drives media time or protocol truth.

### Raw diagnostic lane

Examples:

- raw/summary HCI event trace;
- ISO submit/completion trace;
- USB completion trace;
- scheduler wake trace;
- ATT transaction trace.

Properties:

- bounded;
- lossy by explicit policy;
- normally in memory only;
- never protocol truth.

---

# 19. TelemetryRing placement

Generic implementation:

```text
runtime/telemetry/ring.rs
```

Owned instances live with the producer:

```text
plane/l1_hci/telemetry/trace.rs
plane/l1_media/telemetry/trace.rs
plane/l2_transport/telemetry/trace.rs
```

Control only gets a telemetry folder if it develops meaningful high-rate
producer-local telemetry. Do not create an empty folder merely for symmetry.

Default raw-trace policy remains recent-history preservation:

```text
fixed capacity
-> overwrite oldest
-> accept newest
-> count overwritten_oldest
-> never block
-> no default persistent write
```

`DropNewest` remains available explicitly.

---

# 20. Two-stage telemetry aggregation

## Stage A — producer-local event-time accumulation

Aggregation happens when the event occurs.

Examples:

```text
ISO submitted
  -> submitted += N
  -> in_flight += N
  -> max_in_flight = max(...)

ISO completion
  -> completed += N
  -> in_flight -= N

queue depth sample
  -> current_queue_depth = depth
  -> max_queue_depth = max(...)
```

Requirements:

- O(1) or otherwise strictly bounded;
- no blocking;
- no filesystem writes;
- no synchronous Core query;
- no general-purpose lock on RT paths;
- avoid allocation on Media RT/HCI hot paths.

## Stage B — coalesced Plane observation publication

High-rate accumulator changes set a local dirty condition.

At a bounded publication point:

```text
local accumulator
-> snapshot
-> <Plane>ConditionUpdate
-> Condition API
-> ConditionReducer
-> Blackboard
```

Exact publication cadence is deliberately not frozen yet.

State-changing facts publish immediately.

High-rate numeric telemetry publishes coalesced.

Fault and teardown force a final observation publication.

---

# 21. Plane-to-Condition boundary

A Plane must not construct or mutate Blackboard objects directly.

Each Plane owns its public Condition projection/update type in its Interface
because that Plane defines what it can truthfully report.

Examples:

```text
plane/l1_control/interface/condition.rs
plane/l1_hci/interface/condition.rs
plane/l1_media/interface/condition.rs
plane/l2_transport/interface/condition.rs
```

Conceptually:

```text
HciConditionUpdate
MediaConditionUpdate
ControlConditionUpdate
TransportConditionUpdate
```

Condition consumes these and maps them into its own private Blackboard fields.

This avoids the forbidden dependency:

```text
Plane -> core::condition::blackboard::*
```

---

# 22. Condition reduction

Condition performs cross-Plane reduction only.

```text
ControlConditionUpdate   ─┐
HciConditionUpdate       ─┤
MediaConditionUpdate     ─┼──► ConditionReducer
TransportConditionUpdate ─┤            │
Lifecycle change         ─┘            ▼
                                  private Blackboard
```

Condition does not:

- count every packet;
- scan raw trace rings;
- perform HCI transactions;
- execute media work;
- own route tables;
- infer protocol success from telemetry counters.

Identical effective Blackboard state does not need to advance the observation
epoch.

---

# 23. Blackboard shape

The Blackboard starts coarse and expands only from product needs.

Conceptually:

```text
Blackboard
├── epoch
├── lifecycle
├── control
├── hci
├── media
└── transport
```

It may expose nested coarse health such as HCI ISO condition later, but it must
not grow raw implementation collections such as:

```text
Vec<HciEvent>
Vec<IsoPacket>
Vec<UsbCompletion>
Vec<LogLine>
raw mutable registries
raw Bumble objects
raw OS handles
```

---

# 24. AOSP-derived state principle adopted by V2

V2 adopts the following structural principle:

> authoritative state belongs to the domain object/owner; a state machine or
> engine operates on that state; detailed asynchronous operation progress stays
> local to the operation.

For Control, the long-term semantic shape is closer to:

```text
current = QosConfigured
target  = Streaming
active operation = Enable / CIS / Path / StreamingGate
```

than to one global phase for every callback wait.

V2 does not require copying AOSP class layout or algorithms. It adopts the
ownership/state pattern only.

---

# 25. Existing ControlMachine migration strategy

The current machine is heavily tested and already proved against HIL. Therefore
it must not be rewritten in one patch.

## Step C1 — move only

Move it to:

```text
plane/l1_control/engine/machine.rs
```

Update module paths only.

Acceptance:

- exact existing unit-test behavior;
- no semantic diff;
- cargo test/check/build green.

## Step C2 — extract durable ControlModel

Identify state that describes durable Control truth rather than callback
position:

- resolved members;
- connections by logical member;
- discovered audio set;
- codec plan/status;
- QoS plan/status;
- enable status;
- semantic live CIS set;
- path readiness;
- streaming confirmation;
- route ACK/binding;
- current semantic stream state;
- target semantic stream state.

Introduce `ControlModel` without changing transition ordering.

## Step C3 — extract operations

Move detailed temporary state from the mega machine into bounded operation
objects incrementally:

- discovery operation;
- ASCS prepare/configure operation;
- streaming gate operation;
- CIG/CIS operation coordination;
- path operation;
- rollback/teardown operation.

Each extraction requires deterministic regression tests before deleting the old
fields.

## Step C4 — narrow ControlEngine

The final engine:

```text
command/report
    +
ControlModel
    +
active operation
    ↓
actions + model mutation
```

It must remain non-blocking and I/O-free.

---

# 26. Stream path invariants preserved through V2

The proven Round7 stream path remains:

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

The following invariants are non-negotiable:

1. Media time never waits for HCI credits.
2. Number Of Completed Packets releases controller capacity only.
3. Media never receives raw CIS handles.
4. HCI owns ISO packet sequence/PB/fragmentation.
5. route resolution remains session-fenced.
6. logical stereo groups remain transport-handle-free above HCI.
7. stale media is not retained to create unbounded latency.
8. route bind ACK still gates StreamBinding handoff.
9. teardown remains route-first then ISO/path/CIS/ASE/CIG/ACL cleanup in the
   established safe order.

A directory refactor must not alter this path.

---

# 27. Review/HIL source cleanup

All Review/HIL harness code leaves the source root.

Target:

```text
review/
├── mod.rs
├── support/
├── review2/
│   ├── mod.rs
│   ├── attach.rs
│   ├── bond.rs
│   ├── bond_smoke.rs
│   ├── gatt_smoke.rs
│   └── iso_smoke.rs
└── round7/
    ├── mod.rs
    └── first_sound.rs
```

Current mapping:

```text
review2.rs
  -> review/review2/attach.rs

review2_bond.rs
  -> review/review2/bond.rs

review2_bond_smoke.rs
  -> review/review2/bond_smoke.rs

review2_gatt_smoke.rs
  -> review/review2/gatt_smoke.rs

review2_iso_smoke.rs
  -> review/review2/iso_smoke.rs

round7_first_sound.rs
  -> review/round7/first_sound.rs
```

Historical behavior/evidence is preserved. Review code may import production
interfaces; production modules must not depend on Review harnesses.

---

# 28. Architecture module retirement

The current `src/architecture/` directory is transitional and should
eventually disappear.

Planned split:

```text
architecture/control.rs
  StreamBinding/format/topology -> L1-Control interface
  ControlSnapshot/State         -> V2 Condition model/update migration

architecture/lifecycle.rs
  lifecycle model               -> Core/Lifecycle
  snapshot                      -> V2 Condition migration

architecture/media.rs
  EncodedStereoFrame            -> L1-Media interface
  snapshot/state                -> V2 Condition migration

architecture/hci_host.rs
  snapshot/state                -> V2 Condition migration

architecture/transport.rs
  HciPacketClass                -> L2-Transport packet
  snapshot/state                -> V2 Condition migration

architecture/stream_clock.rs
  clock model                   -> L1-Media clock
  read projection               -> Condition migration

architecture/iso_tx_bridge.rs
  -> L1-Media bridge

architecture/execution.rs
  -> runtime context policy

architecture/mailbox.rs
  -> runtime mailbox contracts + scoped-message audit

architecture/platform.rs
  -> L3 Platform model/backend state

architecture/runtime_state.rs
  -> replaced by Core/Condition Blackboard
```

There must be no final `architecture/common.rs` or equivalent dumping ground.

---

# 29. Existing runtime aggregation retirement

The current:

```text
runtime/aggregate.rs
RuntimeObservation
RuntimeAggregator
PlaneTelemetryAccumulator
RuntimeState
```

is an intermediate implementation from the previous design step.

V2 replacement:

```text
RuntimeAggregator
    -> Core/Condition/ConditionReducer

RuntimeState
    -> private Core/Condition Blackboard

RuntimeObservation
    -> Plane-owned typed ConditionUpdate values

PlaneTelemetryAccumulator
    -> producer-local telemetry abstraction;
       generic helper/trait may live under runtime/telemetry only if useful
```

Do not perform this rename mechanically before the Plane update boundaries are
defined.

---

# 30. Detailed migration phases

## Phase V2-0 — freeze plan and baseline

Actions:

- commit this document;
- capture current HEAD;
- run cargo test/check/build;
- confirm clean working tree;
- record current test count;
- do not change runtime behavior.

Gate:

```text
cargo test --workspace --locked
cargo check --workspace --locked
cargo build --workspace --locked
git status clean
```

---

## Phase V2-1 — source tree scaffolding

Create only the module skeleton needed for immediate moves:

```text
core/
plane/l1_control/
plane/l1_hci/
plane/l1_media/
plane/l2_transport/
review/
```

Do not create every future empty folder.

No semantic changes.

Gate: full test/check/build.

---

## Phase V2-2 — Review/HIL relocation

Move Review files out of `src/` root first because they have no product
ownership ambiguity.

Goals:

- clean source root;
- prove move-only workflow;
- validate module alias/update technique.

No code behavior changes.

Gate: full test/check/build.

---

## Phase V2-3 — L2 Transport relocation

Move Transport into `plane/l2_transport/`.

Why early:

- relatively small;
- ownership already frozen;
- lower dependency surface;
- establishes L-numbered Plane layout.

No semantic edits.

Gate: full test/check/build plus existing transport tests.

---

## Phase V2-4 — L3 Platform subdivision

Reorganize Platform into backend/usb/windows/firmware.

No behavioral changes.

Gate:

- full test/check/build;
- firmware/bootstrap tests unchanged;
- TestBackend behavior unchanged.

---

## Phase V2-5 — L1-HCI structural relocation

Move HCI files into host/acl/gatt/iso without changing algorithms.

Important rule:

> Do not split mailbox.rs or gatt.rs semantically in the same commit as moving
> them.

First obtain a green move-only state.

Then perform small interface subdivisions.

Gate:

- all HCI unit tests;
- full suite;
- no HIL required for pure path changes.

---

## Phase V2-6 — L1-Media structural relocation

Move:

- LC3;
- synthetic source;
- StreamClock;
- IsoTxBridge;

under L1-Media.

This phase formally resolves the old “Bridge belongs to neither Plane” design.

No scheduler/clock behavior changes.

Gate:

- StreamClock tests;
- LC3 tests;
- Bridge tests;
- synthetic LC3->Bridge->logical ISO test;
- full suite.

---

## Phase V2-7 — L1-Control structural relocation

Move current Control implementation into L1-Control subdivisions.

This is still a move-only phase.

Do not redesign ControlMachine while import paths are changing.

Gate:

- all Control tests;
- full suite.

---

## Phase V2-8 — runtime primitive subdivision

Move runtime files into explicit submodules and move generic TelemetryRing to:

```text
runtime/telemetry/ring.rs
```

Move execution/mailbox architecture contracts down into runtime where
appropriate.

Perform the MessageScope dependency audit separately.

Gate: runtime tests + full suite.

---

## Phase V2-9 — Core/Lifecycle ownership cleanup

Move coarse lifecycle types/behavior into:

```text
core/lifecycle/
```

Resolve identities one at a time.

Required invariants:

- generation fencing behavior unchanged;
- stale work still rejected;
- session identity semantics unchanged;
- StreamId minting semantics unchanged until explicitly audited.

Gate: runtime generation/context tests + full suite.

---

## Phase V2-10 — Condition V2 foundation

Introduce:

```text
core/condition/
├── interface/
├── reducer/
└── blackboard/
```

Requirements:

- one Condition owner;
- one private Blackboard;
- no public mutable Blackboard access;
- snapshot API;
- semantic query API scaffold;
- no raw telemetry ingestion.

At this phase the old RuntimeState path may coexist temporarily behind an
adapter if needed.

Gate:

- deterministic reducer tests;
- identical update coalescing;
- independent Plane projection tests;
- generation/session stale-update tests where relevant.

---

## Phase V2-11 — Plane ConditionUpdate boundaries

Add producer-owned Condition update schemas one Plane at a time.

Order:

1. Transport;
2. HCI;
3. Media;
4. Control;
5. Lifecycle direct Core integration.

Each update type contains only what that producer can truthfully assert.

No Plane imports private Blackboard structs.

Gate after every Plane.

---

## Phase V2-12 — telemetry producer wiring

For each high-rate producer:

```text
event
├── semantic path if state-changing/reliable
├── O(1) local telemetry accumulator
└── optional bounded raw trace ring
```

Implement HCI/ISO first because existing credit audit data already provides a
strong basis.

Then Transport.

Then Media.

Do not wire all three in one patch.

Required tests:

- accumulator counters;
- overflow/loss accounting;
- coalesced publication;
- immediate state publication;
- forced fault/teardown publication;
- no reliable semantic event can enter a lossy ring.

---

## Phase V2-13 — Command V2 foundation

Introduce Core Command facade only after Condition queries are stable.

Flow:

```text
Command request
-> Condition semantic query
-> admission result
-> typed owner command
-> owner validates again
```

No Blackboard access.

No direct lower-plane mutable access.

---

## Phase V2-14 — ControlModel extraction

Begin semantic refactor of ControlMachine.

This is the first phase where significant Control behavior structure changes.

Rules:

- one bounded extraction at a time;
- existing tests remain the oracle;
- add new model/operation tests before deleting old fields;
- do not change HIL protocol ordering;
- do not change ASCS success criteria;
- do not change route ACK gate;
- do not change rollback safety semantics.

---

## Phase V2-15 — Core runtime composition

Compose:

```text
Lifecycle
Command
Condition
Core timers
Control owner
HCI owner
Media owner
Transport owner
```

Startup/shutdown/recovery remains the previously frozen ordering unless a new
test-supported correction is required.

This phase produces the first true V2 product runtime rather than Review/HIL
oracles stitched around product components.

---

## Phase V2-16 — full synthetic composed-runtime acceptance

Acceptance must use the composed V2 runtime:

```text
Command
-> Lifecycle/Control
-> HCI
-> StreamBinding
-> Media
-> Bridge
-> logical ISO
```

Synthetic First Sound acceptance remains distinct from software-only tests.

The old Round7 HIL remains evidence, not automatically proof of the new
composition.

---

## Phase V2-17 — WASAPI / ASRC continuation

Only after V2 composition is stable:

- Process Loopback Capture;
- source SPSC;
- clock estimator;
- ASRC;
- discontinuity policy.

The V2 restructuring must not drag this work forward prematurely.

---

# 31. Commit discipline

The refactor must use small commits.

Preferred pattern:

```text
refactor(review): move Review2 harness under review tree
refactor(transport): move L2 transport into plane hierarchy
refactor(platform): split backend firmware usb windows layout
refactor(hci): move host core
refactor(hci): move acl modules
refactor(hci): move iso modules
refactor(media): move codec/source
refactor(media): move clock and bridge
refactor(control): move planning modules
refactor(control): move discovery and ASCS
refactor(control): move engine facade
runtime: split telemetry ring module
core(condition): introduce private Blackboard
...
```

Do not combine:

- large path moves;
- type ownership changes;
- FSM redesign;
- telemetry wiring;

in one commit.

---

# 32. Test policy during structural migration

Every structural batch must run:

```text
cargo test --workspace --locked
```

Milestone batches additionally run:

```text
cargo check --workspace --locked
cargo build --workspace --locked
```

HIL is not required for pure path/module moves if:

- code semantics are unchanged;
- full software suite is green.

HIL is required when:

- Control transition semantics change;
- HCI transaction ordering changes;
- ISO route/submit behavior changes;
- stream timing/scheduling changes;
- product runtime composition replaces an old Review oracle.

Evidence classes remain separate:

```text
Build
Unit/Regression
Software Integration
HIL
Acoustic
```

---

# 33. Source-root target

After structural migration the source root should be boring:

```text
src/
├── main.rs
├── core/
├── plane/
├── runtime/
├── platform/
└── review/
```

A new top-level directory requires a real ownership/layer justification.

The following top-level names are explicitly banned as generic buckets:

```text
shared/
common/
contract/
shared_info/
helpers/
misc/
utils/
```

A domain-local `util` may exist only for tightly scoped implementation detail
inside that owner and should be avoided when a semantic name is available.

---

# 34. State-change flow

The final intended closed loop is:

```text
                      Core
             ┌─────────┴─────────┐
             │                   │
          Command             Condition
          Write API            Read API
             │                   │
             │ query API         │
             ├──────────────────►│
             │             private Blackboard
             │                   ▲
             ▼                   │
         owner Plane             │
             │                   │
        authoritative Model      │
             │                   │
            Engine               │
             │                   │
      local Operation/Tx         │
             │                   │
          lower owner/HW         │
             │                   │
        real completion          │
             │                   │
       local model update        │
             │                   │
       ConditionUpdate ──────────┘
```

The top-down side expresses desired action.

The bottom-up side expresses observed fact.

No shared mutable world state bridges the two.

---

# 35. Telemetry flow

```text
producer event
    │
    ├──────────────► bounded TelemetryRing<T>
    │                  raw diagnostics
    │                  lossy by policy
    │
    ├──────────────► local TelemetryAccumulator
    │                  O(1) counters/gauges
    │                         │
    │                         └─ coalesced snapshot
    │                                  │
    └─ reliable semantic path           ▼
         when state-changing      Plane ConditionUpdate
                                         │
                                         ▼
                                     Condition
                                         │
                                         ▼
                                     Blackboard
```

Normal logging consumes meaningful state/Condition information.

Raw ring dumps are explicit diagnostics, faults, or debug requests.

---

# 36. Superseded design statements

When V2 is implemented, the following older statements are no longer
authoritative:

- “Control is the single writer of RuntimeState.”
- “RuntimeState reducer belongs to Control.”
- “IsoTxBridge is deliberately outside all Plane ownership.”
- “Plane -> Control reports” as the universal observation path.
- any implication that a shared RuntimeState is operational truth.
- any implication that high-rate telemetry is aggregated centrally by
  Condition/Core.

Their replacement is:

- Condition owns the read model.
- Planes own operational truth.
- L1-Media owns the current Bridge implementation.
- producer-local telemetry is aggregated before Condition.
- Command and Condition form the Core write/read API boundary.

Historical docs should gain a superseded note when touched during migration;
their original experiment/review evidence must not be rewritten.

---

# 37. Explicit non-goals of the V2 refactor

V2 structural migration does not by itself implement:

- Windows Process Loopback;
- ASRC;
- controller-anchored StreamClock;
- new codec policy;
- new Bluetooth pairing policy;
- a new HCI backend;
- UI;
- persistent raw telemetry logging;
- changed mailbox capacities;
- changed thread count;
- changed acoustic levels.

Those remain separate product phases.

---

# 38. First landing sequence

The recommended immediate execution sequence after this plan is committed:

```text
A. Baseline gate + clean HEAD
B. Create minimal module scaffolding
C. Move Review/HIL tree
D. Move L2-Transport
E. Re-run full gates
F. Move L3-Platform
G. Move L1-HCI by host/acl/gatt/iso batches
H. Move L1-Media + clock + bridge
I. Move L1-Control
J. Split runtime primitives
K. Introduce Condition V2
L. Wire Plane observations/telemetry
M. Introduce Command V2
N. Decompose ControlMachine
O. Compose V2 runtime
P. HIL
```

Structural movement comes before semantic restructuring.

---

# 39. Definition of success

The V2 architecture is considered landed when all of the following are true:

- source root contains only real ownership/layer domains;
- every Plane path carries its L-number;
- there is no Shared/Common/Contract bucket;
- no global detailed FSM exists;
- Core/Lifecycle is coarse only;
- each Plane owns its authoritative operational model;
- Command is the only Core write-side public path;
- Condition is the only Core read-side public path;
- Blackboard is private to Condition;
- Command queries Condition API rather than Blackboard;
- Planes report typed Condition updates without importing Blackboard internals;
- raw telemetry stays producer-local and bounded;
- high-rate statistics aggregate locally before publication;
- Condition only performs cross-Plane read-model reduction;
- generic TelemetryRing lives in runtime infrastructure;
- Review/HIL code is isolated under review/;
- the Round7 product data-plane invariants remain intact;
- software gates are green;
- composed-runtime HIL is separately re-proven after semantic composition
  changes.

---

# 40. Frozen V2 summary

> **Command expresses intent downward. Condition exposes observed fact upward.
> Each Plane owns its operational truth and local transition machinery.
> Condition owns one private readonly Blackboard, but the Blackboard is only a
> read model. There is no SharedInfo and no global detailed state machine.
> Raw telemetry stays bounded at the producer, statistics aggregate locally,
> and only coalesced Plane observations enter Condition. Source layout mirrors
> ownership and architectural level so paths themselves communicate where code
> belongs.**
