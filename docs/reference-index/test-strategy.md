# Deterministic Test Strategy / Oracle Map

This file is the cross-domain checklist for code that should be maintained primarily by `cargo test` once architecture/semantics are human-approved.

## Rule

A module is not “done” because it resembles upstream code.

It is done when:

1. the BE200AudioHub contract is explicit;
2. one or more upstream function-level references support the design;
3. deterministic tests encode the contract;
4. external assumptions are isolated behind fakeable adapters;
5. only the remaining real-world assumptions are left for HIL.

## Oracle matrix

| BE200AudioHub area | Primary source anchors | Test mechanism | HIL required for |
|---|---|---|---|
| Runtime context | AOSP `InitAudioSinkThread`, LE Relay `ReconcileLoopAsync` | deterministic thread/mailbox tests | scheduler performance only |
| Generation/cancel | LE Relay `RequestRestart`, `WatchCommandsAsync` | event replay | OS callbacks arriving late |
| Async USB | libusb `submit/cancel/free_transfer` | TestBackend + callback fixtures | Windows/libusb/BE200 behavior |
| HCI Host adapter | Bumble `HostTransport`, `Device::poll` | packet replay | controller quirks |
| ACL credits | Zephyr `hci_num_completed_packets`, Bumble queues | model/property tests | actual buffer counts/timing |
| ATT/GATT | Bumble parser/discovery functions | recorded/synthetic ATT DB | Buds attribute database |
| SMP/privacy | Bumble SMP/privacy functions | golden vectors/FSM replay | Samsung key distribution |
| PACS/ASCS | Bumble profile parsers/FSM | byte fixtures/state replay | Buds notifications/caps |
| CIG/CIS semantics | AOSP state machine | fake HCI callbacks | BE200 command behavior |
| ISO fragmentation | Bumble/Zephyr/BTstack/Linux | property tests | controller packet limits |
| StreamClock | AOSP periodic worker + nRF timing | fake clock | timer jitter/clock source |
| IsoTxBridge | PipeWire/Espressif | fake absolute clock | controller TX-sync quality |
| WASAPI capture | LE Relay ProcessLoopbackSource | fake source adapter | Windows audio stack |
| LC3 wrapper | liblc3 | golden PCM/decode | acoustic acceptance |
| Lifecycle/recovery | LE Relay supervisor + AOSP teardown | scripted subsystem world | which faults happen in practice |

## Test layers

### Layer A — pure parser / algorithm

No threads, no time, no I/O.

Examples:

- HCI/ATT/L2CAP/PACS/ASCS bytes;
- privacy/hash/address resolution;
- codec/QoS planner;
- LC3 frame sizing;
- channel mapping.

Prefer table-driven tests plus malformed-input fuzz/property tests.

### Layer B — deterministic state machine

Inject events explicitly.

Examples:

- SMP;
- ASE;
- command transaction;
- CIG/CIS lifecycle;
- lifecycle reducer;
- recovery escalation.

Each test should assert:

- state before;
- event;
- emitted actions/reports;
- state after.

Never assert only final state when intermediate emitted actions are correctness-relevant.

### Layer C — fake backend / fake clock integration

Examples:

- TransportReactor + TestBackend;
- HCI Host + packet sink;
- Bridge + fake StreamClock;
- Media + fake PCM producer;
- Lifecycle + fake subsystem handles.

Use this layer to verify cross-module ownership without hardware.

### Layer D — soak/property

Examples:

- random enqueue/completion sequences;
- random generation changes;
- random FrameId arrival jitter;
- long ppm drift simulation;
- randomized packet fragmentation.

Useful invariants:

- bounded memory;
- no negative credits;
- no stale-generation mutation;
- monotonic target timeline;
- no duplicate completion;
- no double-free/reclaim;
- no unbounded latency accumulation.

### Layer E — HIL

Hardware integration should answer only questions software fixtures cannot:

- does WinUSB/libusb deliver completions as assumed?
- what does BE200 firmware actually report/do?
- what quirks does Buds3 Pro expose?
- what is real WASAPI/QPC jitter?
- does controller TX sync behave well enough?
- is acoustic playback clean?

When HIL discovers a bug:

1. capture the event/packet/timestamp sequence;
2. reduce it into a software regression fixture;
3. fix under `cargo test`;
4. rerun HIL only to validate the external assumption/fix.

## Domain-specific mandatory suites

## Runtime

Must cover:

- FIFO;
- bounded Full/Disconnected;
- stale generation;
- cancellation;
- idle stop;
- panic/fault;
- request/reply correlation;
- duplicate reply.

Primary index: [runtime-execution.md](./runtime-execution.md).

## Transport

Must cover:

- concurrent Event/ACL pending reads;
- partial/multiple packet reassembly;
- overflow fault;
- detach/cancel;
- callback ownership;
- transfer reuse;
- TX pressure fairness.

Primary index: [transport-usb.md](./transport-usb.md).

## HCI Host

Must cover:

- H4/native conversion;
- command status/complete correlation;
- queue capacity;
- per-handle completions;
- control lane not starved by data.

Primary index: [hci-host.md](./hci-host.md).

## ATT/GATT/security

Must cover:

- parser edge cases;
- request correlation;
- notification/indication;
- discovery pagination;
- encryption/security transition;
- persistence commit boundaries.

Indexes:

- [acl-att-gatt.md](./acl-att-gatt.md)
- [security-privacy.md](./security-privacy.md)

## LE Audio Control

Must cover:

- PACS LTV;
- codec/QoS plan;
- ASE valid/invalid transitions;
- CIG/CIS binding;
- data-path callback ordering;
- partial failure rollback;
- teardown.

Primary index: [le-audio-control.md](./le-audio-control.md).

## ISO Host

Must cover:

- PB flags;
- fragment boundaries;
- sequence;
- multiple CIS;
- credit release;
- over-completion;
- disconnect flush.

Primary index: [iso-data-plane.md](./iso-data-plane.md).

## Media / Bridge

Must cover:

- one authoritative interval grid;
- FrameId monotonicity;
- ppm drift;
- deadline jitter;
- stale/drop/fallback;
- atomic L/R interval behavior;
- controller-anchor rebase.

Indexes:

- [media-clock.md](./media-clock.md)
- [bridge-scheduler.md](./bridge-scheduler.md)
- [codec-lc3.md](./codec-lc3.md)

## Windows audio / lifecycle

Must cover with fake adapters:

- callback nonblocking overflow;
- source discontinuity;
- unexpected stop;
- generation invalidation;
- startup/stop failures;
- recovery/backoff.

Indexes:

- [windows-audio.md](./windows-audio.md)
- [lifecycle-recovery.md](./lifecycle-recovery.md)

## Fixture policy

Store reusable fixtures close to the owning test module or under a future dedicated test-fixture directory.

Good fixture sources:

- Experiment/R5 packet captures/log-reduced byte sequences;
- known-good upstream examples;
- spec-derived byte vectors;
- real HIL failures reduced to minimum reproducer.

Do not bake wall-clock sleeps into deterministic tests. Use fake clocks/timers or bounded synchronization.

## Failure policy for tests

A correctness test must not “retry until green”.

If a deterministic test is flaky, treat that as an architecture/test synchronization defect.

Hardware/profiling tests may use statistical thresholds, but must record the distribution and acceptance rule.

## Reference review trigger

Re-open upstream source and update this index when:

- a new Plane/module is introduced;
- an upstream dependency revision changes;
- HIL disproves an assumed behavior;
- a test cannot express a required invariant without reaching through module ownership;
- a workaround starts requiring shared mutable cross-context state.

Those are signs the current mapping may need architectural review rather than another patch.
