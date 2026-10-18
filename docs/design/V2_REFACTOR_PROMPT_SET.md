# V2 Refactor Prompt Set

Branch: `refactor/v2-architecture`

Baseline:
- main/V2 plan commit: `30ab166f`
- scaffold commit: `a0cb4f9d`
- baseline software gate: 362 tests PASS; cargo check/build PASS
- new V2 files are intentionally empty and initially not wired into `main.rs`

Global rules for every round:
- Actual repository state is authoritative. Read the branch before acting.
- Work only on `refactor/v2-architecture`.
- Never use the handoff/plan as proof of current code state.
- Fill new files manually from the old implementation; do not bulk blindly move the whole tree.
- Prefer small commits grouped by one ownership boundary.
- Do not combine path migration with semantic redesign unless the round explicitly says so.
- After each bounded batch, run `cargo test --workspace --locked`.
- At milestones also run `cargo check --workspace --locked` and `cargo build --workspace --locked`.
- Do not run HIL for pure source relocation.
- Keep reliable semantic events separate from telemetry.
- Do not create Shared/Common/Contract/Utils dumping grounds.
- Do not let Command read Blackboard directly.
- Do not change Round7 stream-path invariants.
- Report progress after each 2-3 bounded edits/commits.
- If a hidden dependency or architectural contradiction appears, stop that sub-migration, document the actual dependency, and repair it explicitly rather than adding a compatibility junk layer.

---

## Round 1 — Review + L2 Transport + L3 Platform

Start by reading:
- `docs/design/IMPLEMENTATION_PLAN_V2.md`
- current `src/main.rs`
- current Review module declarations
- current `src/transport/*`
- current `src/platform/*`
- only the directly referenced modules needed to fix imports

Goals:
1. Fill and activate `src/review/*`.
   - Move Review2 / Round7 HIL source out of the source root manually.
   - Preserve CLI behavior exactly.
   - Production code must not depend on `review/`.
2. Fill and activate `src/plane/l2_transport/*`.
   - Preserve packet/reactor/reassembly/sink/worker behavior.
   - Keep Transport free of Bluetooth semantic knowledge.
3. Fill Platform subdivisions:
   - `platform/backend/`
   - `platform/usb/`
   - `platform/windows/`
   - `platform/firmware/`
   - Keep the existing top-level `platform/mod.rs` only as the module facade while transitioning.
4. Delete old source files only after every new path compiles and tests pass.
5. Do not touch HCI/Media/Control algorithms.

Special audit:
- Record any imports where Transport or Platform unexpectedly reach upward into HCI/Control/Review.
- Record any Review helper that production code depends on.

Exit gate:
- full cargo test/check/build green;
- source root no longer contains Review2/Round7 implementation files;
- old `src/transport/` removed;
- Platform behavior unchanged;
- clean git status;
- provide a concise dependency-strangler report for the next round.

---

## Round 2 — L1 HCI Host + ACL + GATT

Read only the actual HCI files in scope and the interfaces they directly consume.

Goals:
1. Fill:
   - `plane/l1_hci/host/`
   - `plane/l1_hci/acl/`
   - `plane/l1_hci/gatt/`
   - `plane/l1_hci/model/` only where an actual owner model already exists
2. Migrate manually:
   - engine
   - worker
   - link
   - stack
   - connection
   - ACL credit
   - privacy
   - security
   - scan
   - GATT transaction machinery
3. Keep existing `hci_host/mailbox.rs` and ISO files temporarily if needed; do not force interface redesign into this round.
4. Preserve Bumble single-owner assumptions.
5. Preserve all stored-LTK/privacy/connection/GATT transaction behavior.
6. Explicitly identify state currently stored inside `HciEngine` that is durable HCI Model versus temporary transaction state. Document findings, but do not redesign it yet unless extraction is trivial and fully test-covered.

Special audit:
- Look for raw handle ownership leaks.
- Look for Review-specific helpers imported by product HCI.
- Look for transaction state accidentally acting as global state.
- Look for HCI code depending on old flat module paths in ways that reveal hidden coupling.

Exit gate:
- HCI host/ACL/GATT tests green;
- full cargo test/check/build green;
- no semantic behavior change;
- provide an HCI ownership map for Round 3.

---

## Round 3 — L1 HCI ISO + Interface + First Telemetry Producer

This is the HCI-heavy round. Work in small commits.

Goals:
1. Fill and migrate:
   - `plane/l1_hci/iso/`
     - cig_cis
     - lifecycle
     - path
     - credit
     - tx
     - feature
     - route
2. Fill `plane/l1_hci/interface/`.
   - Start by relocating existing mailbox/control/report/logical-ISO types without semantic change.
   - Only split the old mailbox file after a green move-only checkpoint.
3. Preserve:
   - session-fenced logical routes;
   - route bind ACK;
   - logical-to-raw CIS resolution;
   - controller ISO credit accounting;
   - group-atomic admission;
   - drain semantics;
   - raw/logical ISO shared bounded lane.
4. Introduce the first real producer-local telemetry structure under:
   - `plane/l1_hci/telemetry/`
5. Use `runtime::telemetry::TelemetryRing<T>` as infrastructure only.
6. First-stage aggregation happens at HCI event/submit/completion sites with bounded O(1) counter/gauge updates.
7. Do NOT send raw packet/completion events to Core Condition yet.
8. Define a coarse HCI Condition-update projection only if enough state is already trustworthy; otherwise leave observation wiring to Round 6.

Special audit:
- Search for any coupling between Number Of Completed Packets and media time.
- Search for any raw CIS handle escaping above HCI.
- Search for counters that currently exist only in Review/HIL code and decide whether they belong to the product HCI telemetry owner.

Exit gate:
- all ISO/route/credit/worker tests green;
- full cargo test/check/build green;
- telemetry ring remains bounded and non-authoritative;
- write a short HCI telemetry schema summary.

---

## Round 4 — L1 Media + StreamClock + Bridge + Runtime Primitives

Goals:
1. Fill and activate:
   - `plane/l1_media/source/`
   - `plane/l1_media/codec/`
   - `plane/l1_media/clock/`
   - `plane/l1_media/bridge/`
   - `plane/l1_media/interface/`
   - `plane/l1_media/model/` only for real durable media state
2. Move:
   - synthetic PCM source;
   - LC3 wrapper;
   - FrameId ownership;
   - LocalStreamClock;
   - IntervalGroupScheduler / LogicalIsoTxBridge runtime.
3. Formally make Bridge part of L1-Media ownership while preserving its transport-neutral HCI interface.
4. Subdivide runtime primitives manually:
   - generation
   - context
   - cancellation
   - mailbox
   - request
   - supervisor
   - telemetry/ring
5. Move generic mailbox delivery policy from old architecture contracts into runtime infrastructure where appropriate.
6. Audit MessageScope rather than blindly moving it:
   - do not create a shared identity bucket;
   - preserve generation/session/stream fencing semantics.
7. Add Media producer-local telemetry skeleton only where a real event source already exists. Do not invent WASAPI/ASRC metrics yet.

Hard invariants:
- Media time never waits for HCI credits.
- HCI completions never advance FrameId.
- Media does not know raw CIS handles.
- Bridge emits logical ISO work only.
- one authoritative media grid remains intact.

Exit gate:
- StreamClock, LC3, Bridge and synthetic end-to-end tests green;
- runtime tests green;
- full cargo test/check/build green;
- old top-level `media/`, `stream_clock/`, `iso_tx_bridge/` removed.

---

## Round 5 — L1 Control Structural Migration + Type Ownership Audit

This round is move-first. Do not redesign the mega-FSM yet.

Goals:
1. Fill and activate:
   - `plane/l1_control/engine/`
   - `plane/l1_control/model/`
   - `plane/l1_control/discovery/`
   - `plane/l1_control/ascs/`
   - `plane/l1_control/planning/`
   - `plane/l1_control/interface/`
2. Manually migrate the existing tested implementation.
3. Split old `control/boundary.rs` only after a green move checkpoint:
   - command/intents
   - reports
   - engine step
4. Move the logical stream contract to Control interface:
   - AudioSide
   - LeAudioFormat
   - StreamTopology
   - StreamBinding
5. Preserve StreamId minting semantics. Audit ownership; do not change who creates it just to satisfy directory aesthetics.
6. Keep current `ControlMachine` behavior exactly.
7. Remove old `src/control/` only after all tests are green.
8. Perform an explicit type ownership audit for everything remaining in `src/architecture/`.
9. Do not create SharedInfo/Contract/Common.

Special audit of ControlMachine:
Classify fields into:
- durable Control Model state;
- current target/current semantic state candidate;
- temporary operation/transaction state;
- rollback-only state;
- read-model-only projection.
Write the classification into a temporary design note or final round report, but do not perform a large extraction yet.

Exit gate:
- all Control tests green;
- full cargo test/check/build green;
- L1-Control tree fully active;
- clear extraction map for ControlModel/operations.

---

## Round 6 — L0 Condition + Command + Lifecycle Foundation, then Architecture Retirement

This round establishes the V2 Core boundary. Keep semantic Control decomposition bounded.

Goals:
1. Fill `core/lifecycle/`.
   - Move only coarse whole-runtime/session state.
   - Preserve generation fencing and existing session identity semantics.
2. Fill `core/condition/`.
   - one Condition owner;
   - one private Blackboard;
   - ConditionReducer;
   - snapshot API;
   - semantic query API for Command;
   - no public Blackboard reference;
   - no raw telemetry ingestion.
3. Add Plane-owned typed ConditionUpdate values incrementally:
   - Transport
   - HCI
   - Media
   - Control
   - Lifecycle
4. Condition reduces only these coarse updates into Blackboard.
5. Wire producer-local telemetry snapshots into Condition only at bounded/coalesced publication points.
   - state changes: immediate;
   - high-rate numeric telemetry: coalesced;
   - fault/teardown: forced final publish.
6. Fill `core/command/`.
   - Command queries Condition API.
   - Command never reads Blackboard directly.
   - Command issues typed operations to actual owners.
   - owner validates final invariants again.
7. Retire transitional:
   - RuntimeAggregator
   - RuntimeState
   - RuntimeObservation
   once their V2 replacements are proven.
8. Retire `src/architecture/` file-by-file only when every concept has a real semantic owner.
9. Begin only the first bounded ControlModel extraction if time/context allows:
   - introduce current/target semantic state without changing transition ordering;
   - do not attempt full mega-FSM decomposition if it risks losing the gate.

Final software gate:
- full cargo test/check/build green;
- no Shared/Common/Contract bucket;
- Blackboard private;
- Command -> Condition API enforced;
- producer telemetry remains local;
- source root reduced to:
  `main.rs / core / plane / runtime / platform / review`.

Stop point:
This six-round set completes the V2 structural refactor and Core read/write
foundation. Full ControlMachine operation extraction, composed V2 runtime HIL,
WASAPI, and ASRC are subsequent implementation rounds and must not be crammed
into this migration set.
