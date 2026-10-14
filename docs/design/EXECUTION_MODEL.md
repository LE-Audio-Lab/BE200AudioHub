# Execution Model, Mailboxes and Blocking Rules

Status: **Review contract — frozen unless HIL or profiling disproves an assumption**

This document refines the base architecture without changing its layers. It follows the AOSP/GD design principle that a subsystem depends on a **serial execution context / handler**, not permanently on a particular OS thread. Cross-context interactions use typed messages and bounded queues rather than shared mutable subsystem state.

The initial runtime maps five execution domains onto five physical threads. That mapping is a deployment choice, not a module-level dependency.

---

## 1. Core principles

### 1.1 Single-owner mutable state

Every mutable subsystem object has exactly one execution-domain owner.

No object is concurrently mutated from multiple contexts through `Arc<Mutex<T>>` merely for convenience.

Immutable configuration snapshots, atomics used for cancellation/health flags, bounded SPSC rings and immutable messages may cross context boundaries.

### 1.2 Run-to-completion messages

A context processes one logical message/event to a bounded completion point, mutates only its owned state, emits messages/data to other contexts, and returns to its event loop.

Long operations are represented as state machines driven by later events/timers. They are not implemented as sleeps or synchronous cross-context waits.

### 1.3 Data passing over data sharing

Subsystem communication uses typed messages or explicit realtime data rings.

`RuntimeState` remains Control-owned and single-writer. Other planes publish reports; they do not mutate the model directly.

### 1.4 All queues are bounded

Unbounded queues are prohibited in the production runtime.

A queue capacity is eventually derived from expected producer rate, worst-case servicing latency and explicit safety margin. When a bounded queue cannot accept data, the resulting action is part of the queue contract: fault, drop-oldest/discontinuity, stale-drop/miss, or telemetry coalescing.

### 1.5 No silent loss of state-changing traffic

HCI Events, lifecycle commands, state-changing reports and command replies are reliable bounded traffic.

If their mailbox exhausts, the subsystem reports/escalates a fault. It does not silently discard the oldest event.

---

## 2. Execution domains and ownership

### CoreContext / CoreThread

Owns:

- L0 LifeCycle / Orchestration;
- Control Plane;
- the mutable `RuntimeState` reducer;
- desired-state and generation bookkeeping;
- session registry/config snapshot;
- recovery/backoff policy.

Core operations are short state transitions and orchestration decisions.

Core must not perform USB, WASAPI, GATT/HCI event waits, sleeps, or filesystem/network operations while the runtime is active. Blocking persistence work is dispatched outside the core execution domains.

### HciContext / HciHostThread

Owns:

- Bumble/Bluetooth Host mutable state;
- HCI command transaction state;
- controller/ACL connection state;
- ATT/L2CAP transport state;
- CIG/CIS controller primitives;
- per-CIS ISO Host queues;
- ISO transport sequence numbers;
- fragmentation/framing state;
- controller ISO capacity and in-flight accounting.

Only HciContext may mutate the Host/Device state.

HciContext never waits for USB, sleeps for completion, or synchronously waits for Core/Media. HCI command transactions are event-driven state machines with requester-owned timeout events.

Inbound controller events must not be starved by outbound ISO draining. Exact fairness/budgeting can be tuned later, but an outbound loop may not monopolize the context.

### MediaRtContext / MediaRtThread

Owns the target media domain:

- authoritative session `StreamClock`;
- clock estimator/discipline state;
- ASRC state;
- channel mapping;
- LC3 encoder state;
- bounded encoded frame store;
- first-stage `IsoTxBridge` interval-group scheduler.

During Running, the realtime hot path:

- executes against absolute StreamClock deadlines;
- performs bounded work only;
- uses preallocated/bounded storage;
- never waits for HCI credit, USB, GATT, logging, a general mutex or source I/O;
- never performs synchronous request/reply;
- never grows an unbounded backlog.

The only intentional wait on this domain is the scheduler waiting until an absolute media deadline or cancellation/stop signal.

Expensive preparation/allocation is allowed before entering Running, not in the steady-state RT path.

### CaptureContext / WASAPI CaptureThread

Owns:

- WASAPI/COM capture/session interfaces;
- Process Loopback source lifecycle;
- Windows source-clock observations;
- source packet/discontinuity handling;
- the producer side of the PCM SPSC ring.

CaptureContext may wait only on the WASAPI source event and cancellation/stop event.

It never waits for Media or Bluetooth state. It drains available source packets into the bounded PCM ring and returns to the source wait.

On PCM-ring overflow it does not block. It applies the source-ring latency policy (drop oldest source-domain data, mark discontinuity, update telemetry).

### TransportContext / UsbReactorThread

Owns:

- L3 platform/backend object;
- libusb/WinUSB device/endpoint handles;
- pending async transfers;
- transfer pools;
- endpoint packet reassembly;
- transport cancellation;
- transport health/fault state.

Transport may block only inside the OS/libusb completion reactor. Independent USB RX endpoints are concurrently pending.

Completion callbacks are bounded: collect bytes/status, update reassembly/transfer state, forward a complete HCI transport packet, resubmit/replenish I/O, return.

Transport never invokes Bumble Host logic and never interprets LE Audio semantics.

---

## 3. Initial physical-thread mapping

The initial deployment is:

| Execution context | Physical thread |
| --- | --- |
| CoreContext | CoreThread |
| HciContext | HciHostThread |
| MediaRtContext | MediaRtThread |
| CaptureContext | WASAPI CaptureThread |
| TransportContext | UsbReactorThread |

This mapping is intentionally reversible. A later profile may colocate two serial contexts or split one policy component onto another worker without changing ownership contracts.

---

## 4. Message scope and stale-message rejection

Every session-scoped cross-context control/report message carries:

- `Generation`;
- optional `SessionId`;
- optional `StreamId`.

A receiver validates scope before mutating state.

A message from an older generation/session is ignored as stale and counted for diagnostics. It is not allowed to revive old state after reconnect/restart.

Generation invalidation is the primary protection against delayed callbacks from prior sessions.

---

## 5. Request/reply rules

There are **no synchronous cross-context calls**.

A logical request contains a request/correlation ID and receives a later reply message/oneshot result.

Allowed model:

```text
Core
  -> HciCommand(RequestId, ...)
  ...
Hci
  -> HciReply(RequestId, Result)
  -> Core mailbox
```

Not allowed:

```text
CoreThread:
  send HCI command
  blocking_wait(reply)
```

Timeout ownership belongs to the requester. A timeout is represented by a timer event/state transition, never `sleep()` on the context thread.

The same rule applies to Core↔Media, Core↔Transport and other control paths.

---

## 6. Mailbox and data-path classes

### 6.1 Core intents and reports

Examples:

- User/L0 desired-state intents;
- HCI reports;
- Media health/state reports;
- Transport health/state reports.

Semantics:

- bounded;
- reliable for state-changing traffic;
- producer non-blocking;
- mailbox-full is a fault, not silent loss.

High-frequency counters are aggregated inside the producer plane and reported as snapshots instead of one message per packet/frame.

### 6.2 Core -> HCI control mailbox

Carries typed Host/control requests such as connect, encryption, ATT operation, CIS setup/teardown and stream binding.

Semantics:

- reliable bounded;
- asynchronous request/reply;
- no requester blocking;
- lifecycle/teardown control must not be trapped behind an unbounded ISO data backlog.

### 6.3 Core -> Media control mailbox

Carries prepare/prime/start/stop/reconfigure commands.

Semantics:

- reliable bounded;
- heavy setup may run before Media enters Running;
- while Running, control handling occurs only at defined safe points and may not introduce unbounded RT work.

### 6.4 Core -> Capture control mailbox

Carries WASAPI prepare/start/stop/recreate commands.

Semantics:

- reliable bounded;
- capture source event waiting remains independent of control-message delivery.

### 6.5 Core -> Transport lifecycle mailbox

Carries attach/start/stop/reset/cancel lifecycle commands for the backend/transport.

Semantics:

- reliable bounded;
- transport shutdown/cancel is independent of ordinary data queues.

### 6.6 Capture -> Media PCM SPSC ring

This is a realtime clock-domain buffer, not a reliable event log.

Semantics:

- bounded SPSC;
- producer never blocks;
- overflow drops the oldest source-domain PCM according to block/frame granularity and marks a discontinuity;
- Media/clock discipline observes the discontinuity and may reset/settle the ASRC estimator;
- latency is bounded rather than allowed to grow indefinitely.

### 6.7 Media frame store

MediaRt and the initial IsoTxBridge execute in the same context; therefore the encoded frame store is an owned bounded data structure rather than a cross-thread mailbox.

Frames are keyed by `FrameId`.

Stale target-domain frames are dropped and counted as misses; they are not played late simply to preserve delivery.

### 6.8 IsoTxBridge -> HCI submission lane

Carries codec-agnostic encoded stereo/group SDUs already selected for a StreamClock interval.

Semantics:

- one producer (MediaRt/Bridge), one consumer (HciContext);
- bounded and non-blocking;
- no waiting for controller credit;
- if the lane cannot accept the interval submission, Bridge records a miss/backpressure fault according to policy rather than blocking the RT context;
- persistent saturation escalates health/recovery.

### 6.9 HCI -> Transport TX lanes

HCI Host is the only Bluetooth producer; Transport is the physical I/O consumer.

Traffic classes remain logically separable (Command/control, ACL, ISO) so sustained ISO load cannot indefinitely starve teardown/control traffic.

Semantics:

- bounded reliable packet submission;
- HCI Host has already applied Bluetooth-controller credit rules before queueing controller-bound data;
- Transport applies only OS/USB I/O backpressure;
- Transport-queue exhaustion is a transport fault, not permission for HCI Host to reinterpret media time.

The exact Rust representation may be one discriminated queue or multiple physical lanes; the semantic separation must be preserved.

### 6.10 Transport -> HCI RX lanes

Carries complete controller-originated HCI packets (Event/ACL/ISO as supported).

Semantics:

- bounded reliable;
- no HCI Event is silently dropped;
- overflow means Host state can no longer be trusted and escalates to controller/session recovery;
- Transport performs packet-boundary reassembly only; HCI semantic decoding remains in HciContext.

### 6.11 Telemetry

High-rate telemetry uses coalesced/latest snapshots or local counters.

Examples:

- ring fill;
- ASRC ppm/ratio;
- HCI queue depth;
- ISO credits/in-flight;
- USB throughput/jitter;
- underrun/miss counters.

Telemetry must never occupy the reliable state-transition queue at packet rate.

---

## 7. Blocking rules summary

| Context | Allowed blocking | Forbidden |
| --- | --- | --- |
| Core | none | USB, WASAPI, filesystem during active session, sleep, sync request/reply |
| HCI Host | none | USB wait, credit wait, command-complete wait loop, sleep |
| Media RT | absolute deadline/cancel wait only | source I/O, HCI/USB/GATT wait, mutex, blocking allocation/logging |
| WASAPI Capture | WASAPI source event + cancel only | waiting for Media/HCI/Core |
| USB Reactor | OS/libusb completion reactor only | waiting for HCI interpretation or session state |

A bounded OS API call used to submit work is distinct from waiting for another execution context to make progress.

---

## 8. Priority and fairness requirements

Exact scheduler implementation remains open, but these invariants are frozen:

1. Controller HCI Event servicing must not be starved by outbound ISO work.
2. Lifecycle stop/reset/teardown control must remain deliverable under data load.
3. ISO draining is budgeted/bounded per event-loop turn; it may not busy-loop until an arbitrarily large backlog is empty.
4. Independent Transport endpoints remain concurrently pending; no round-robin read timeout may create priority inversion.
5. MediaRt deadlines take precedence over nonessential telemetry/logging.
6. High-rate metrics are sampled/coalesced outside the critical packet/frame path.

---

## 9. Startup and shutdown ownership

### Startup direction

```text
L0/Core
  -> Transport attach/start
  -> HCI Host ready
  -> Control prepares Bluetooth stream
  -> Capture/Media prepare
  -> StreamClock established
  -> IsoTxBridge/HCI binding ready
  -> Media prime
  -> Media start
```

The exact ordering of Capture prepare versus Bluetooth preparation can be optimized later, but audio production does not enter Running until the target stream is ready.

### Shutdown direction

```text
L0/Core
  -> invalidate/stop Media production
  -> stop Bridge interval submission
  -> drain/cancel bounded ISO work as policy requires
  -> Control/HCI stream teardown
  -> stop/release Capture
  -> stop HCI/Transport when session/controller lifecycle requires it
```

A new generation invalidates delayed callbacks from the old session immediately.

---

## 10. Error and overflow semantics

A full reliable mailbox is a correctness fault.

A full realtime/source buffer is a latency event handled by that buffer's explicit policy.

A stale media frame is not equivalent to a lost HCI Event:

- stale media may be dropped/fallback-substituted;
- HCI Event loss invalidates Host state and requires escalation.

This distinction is mandatory throughout implementation and testing.

---

## 11. Allocation and logging rules

### Media RT

Steady-state MediaRt work uses preallocated/bounded storage. No formatting/log output, filesystem access or general allocator dependency belongs in the per-interval hot path.

RT diagnostics are counters or fixed-size trace records drained elsewhere.

### HCI Host / Transport

Control-path allocation is permitted. Steady-state high-rate ISO/USB paths use bounded packet/transfer pools where practical; unbounded allocation growth is prohibited.

Transport callbacks do not perform verbose formatting or Host parsing.

---

## 12. AOSP-derived rationale

The model intentionally follows the AOSP Bluetooth architecture style rather than copying Android class names:

- mutable subsystem state is serialized onto a handler/execution context;
- hardware/HAL I/O is isolated from Host state-machine execution;
- queue/message passing is preferred over shared mutable objects;
- realtime LE Audio media work executes on a dedicated worker with periodic scheduling;
- the execution context abstraction is separate from the physical thread deployment.

The Windows adaptation adds a separate event-driven WASAPI CaptureContext and an explicit USB ReactorContext because those platform I/O domains must not inject jitter into MediaRt or HciContext.

---

## 13. Still implementation-defined

This contract does not select:

- Tokio, async-std, crossbeam, ringbuf, flume or any specific channel crate;
- IOCP versus libusb async implementation details;
- exact mailbox capacities;
- exact HCI lane arbitration weights;
- exact MMCSS class/priority;
- exact packet/transfer pool sizes;
- exact persistence blocking executor;
- final physical-thread colocations after profiling.

Any implementation choice must preserve the delivery, ownership and blocking semantics above.
