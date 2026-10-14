# BE200AudioHub — Base Design Principles

Status: **Review / Refactor architecture baseline**

This document records the architectural decisions frozen after the Experiment phase. It is the default design contract for new implementation work. It intentionally separates **architectural ownership and execution semantics** from concrete Rust libraries, queue crates, executors, thread APIs and OS synchronization primitives.

The Experiment/R5 implementation remains archived under `deprecated/r5/`.

---

## 1. Design objective

The Experiment phase proved that a Windows-hosted Rust stack can drive BE200 and Galaxy Buds3 Pro through the complete LE Audio path:

- controller bootstrap and HCI;
- bond/privacy/security recovery;
- PACS / ASCS / CAS / CSIS;
- codec and QoS configuration;
- CIG / dual CIS lifecycle;
- ISO Data Path setup/removal;
- LC3 dual-mono SDUs;
- real acoustic First Sound.

The remaining sustained-playback defects were traced to the experimental software architecture: synchronous USB endpoint polling, mixed clock domains and transport servicing latency. Review therefore treats the protocol path as proven and focuses on a clean production architecture.

The principal rule is:

> **Do not let transport backpressure become media time.**

---

## 2. Vertical architecture

### L0 — LifeCycle / Orchestration

L0 owns the service/session lifecycle, not Bluetooth packet semantics.

Responsibilities:

- desired state;
- generation/session identity;
- ordered startup and teardown;
- retry/backoff;
- recovery escalation;
- process/worker supervision;
- cancellation and stale-generation rejection.

Session state is an L0-owned concept, not a separate architectural layer.

L0 may decide to restart Media, rebuild a stream, restart the controller, or restart the worker. It does not interpret raw HCI status or manipulate ASE/CIS objects directly.

---

### L1 — Control Plane

Control owns high-level Bluetooth and LE Audio semantics:

- device/set identity;
- bond, privacy and security policy;
- PACS / ASCS / CAS / CSIS;
- codec selection and QoS planning;
- ASE state;
- semantic CIG/CIS lifecycle;
- ISO Data Path orchestration;
- stream readiness.

Control also owns the authoritative **RuntimeState reducer**.

#### RuntimeState rule

RuntimeState is:

> **shared-readable, single-writer**

Other planes do not receive `&mut RuntimeState` and do not mutate shared global state through a mutex. They emit typed reports. Control reduces those reports into the current world model.

RuntimeState is an observation model, not a bag containing the actual internal objects of Media, HCI or Transport.

High-frequency telemetry is aggregated within its owning plane before being published to Control.

---

### L1 — Media Plane

Media owns Windows audio ingestion and the entire source/media clock domain.

Target pipeline:

```text
WASAPI Process Loopback
        |
        v
PCM source SPSC ring
        |
        v
Clock discipline / ASRC
        |
        v
exact target-domain PCM frame
        |
        v
channel mapping
        |
        v
LC3 L / LC3 R
        |
        v
EncodedStereoFrame(FrameId)
```

The Windows-side source path inherits proven LE Audio Relay practices:

- Process Loopback;
- event-driven WASAPI capture;
- canonical 48 kHz float stereo internal representation;
- process-tree exclusion;
- endpoint/device invalidation handling;
- bounded SPSC buffering;
- capture discontinuity/overflow telemetry.

The target-domain side follows AOSP LE Audio principles:

- a dedicated periodic media execution context;
- one fixed LE media interval;
- ASRC between source and target clock domains;
- codec work after clock-domain adaptation;
- media start gated until the Bluetooth stream is ready.

Media does **not** know:

- CIS handles;
- CIG IDs;
- HCI controller credits;
- USB endpoints;
- Number Of Completed Packets.

---

### L1 — HCI Host Plane

HCI Host is the sole owner of controller-facing Bluetooth Host state.

Responsibilities include:

- HCI commands/events;
- ACL/ATT transport state;
- controller connection state;
- CIS primitives;
- ISO transport sequence numbers;
- ISO framing and fragmentation;
- per-CIS Host queues;
- controller ISO capacity and in-flight accounting;
- Number Of Completed Packets handling.

Bumble/Bumble-RS Host state is only accessed from this execution domain.

A controller completion:

> **releases transport capacity only**

It never causes the Media Plane to advance its clock or generate its next audio frame.

---

## 3. IsoTxBridge

The Media-to-HCI boundary is an explicit architectural component:

`IsoTxBridge`

It is intentionally not forced into either the Media or HCI ownership hierarchy. This makes scheduling policy replaceable without changing either Plane.

### First implementation — Interval Group Scheduler

The initial policy follows the direction established by PipeWire and Espressif while using Zephyr/Bumble-style Host queues beneath it.

Responsibilities:

- consume encoded frame groups;
- map FrameId to one StreamClock interval;
- submit left/right as one logical stereo group;
- maintain bounded buffering;
- reject/drop stale frames rather than accumulating unbounded latency;
- use Media-provided pre-encoded fallback SDUs for a missed interval where policy requires it;
- never block on HCI credits or USB.

The Bridge is codec-agnostic. It may carry fallback bytes supplied by Media, but it does not encode LC3 itself.

### Replaceable policy

The Bridge contract must permit a later controller-anchored scheduler without changing Media/HCI boundaries.

Current path:

`IntervalGroupScheduler`

Future candidate:

`ControllerAnchoredScheduler`

---

## 4. Authoritative StreamClock

Each active ISO stream group has one authoritative session-scoped media timeline.

There is **one 10 ms grid**, not separate Media and Bridge clocks.

For a frame N:

```text
StreamClock
  Frame N -> authoritative target time Tn

Media:
  produces Frame N before Tn

IsoTxBridge:
  commits Frame N to HCI before Tn

Controller:
  transmits on the corresponding ISO grid
```

Media and IsoTxBridge consume the same FrameId-to-time mapping and may use different lead/deadline offsets on that one timeline.

### ASRC is not the clock

ASRC is an actuator mapping the WASAPI source clock into the StreamClock domain.

Conceptually:

```text
WASAPI clock
     |
     v
clock estimator / discipline
     |
     v
ASRC ratio
     |
     v
StreamClock-domain PCM
```

Initial authority may be Windows local monotonic/QPC time.

The architecture must allow the same StreamClock abstraction to become controller-anchored later using ISO TX synchronization/anchor information.

The logical `FrameId` semantics must survive that upgrade unchanged.

---

## 5. L2 — Transport Plane

Transport owns reliable asynchronous HCI packet movement, not Bluetooth semantics.

Responsibilities:

- concurrent RX/TX transport;
- complete-packet reassembly;
- endpoint/transfer cancellation;
- OS/USB I/O backpressure;
- transport health/fault reporting.

Transport may distinguish Command/Event/ACL/ISO HCI packet classes. It does not understand ASE, CIG, CIS meaning or controller-credit policy.

The key boundary is:

> **Transport guarantees a complete HCI transport packet. HCI Host decides what it means.**

Independent RX endpoints must never be serviced by synchronous round-robin polling.

Bluetooth-controller backpressure and OS/USB I/O backpressure are separate mechanisms and remain separate.

---

## 6. L3 — Platform / Device Backend

L3 contains OS/hardware-specific raw I/O and BE200 bootstrap behavior:

- Windows/libusb/WinUSB primitives;
- device discovery/open/claim;
- endpoint discovery;
- raw async transfer primitives;
- Intel controller image/bootstrap behavior;
- BE200-specific hardware quirks;
- deterministic test backend.

L3 does not interpret LE Audio semantics or HCI Host state.

---

## 7. Execution-context model

The architecture follows the AOSP Handler principle:

> Modules depend on a serial execution context, not directly on a particular OS thread.

A module should not contain architectural assumptions such as “I own pthread #3”. Runtime composition binds execution contexts to physical threads.

### Initial deployment

The first implementation uses five physical threads / execution domains:

#### CoreThread / CoreContext

Owns:

- L0 LifeCycle;
- Control Plane;
- RuntimeState reducer.

These are serialized state transitions and low-rate orchestration tasks. They share one execution context so normal state reduction does not require cross-thread locking.

#### HciHostThread / HciContext

Owns:

- Bumble Device / Host state;
- ACL/ATT;
- CIS/ISO;
- controller credits and Host queues.

This domain is isolated because HCI event service latency directly affects ISO stability.

#### MediaRtThread / MediaRtContext

Owns:

- StreamClock scheduling;
- target-domain media deadlines;
- ASRC;
- channel mapping;
- LC3;
- first-stage IsoTxBridge interval scheduling.

Media and Bridge use the same authoritative timeline. The Bridge is architecturally independent but initially executes on the Media RT context.

The RT hot path performs only bounded, non-blocking work. It never waits for HCI credit, USB, GATT or general logging.

#### WASAPI CaptureThread / CaptureContext

Owns event-driven Process Loopback ingestion on the Windows audio clock and writes PCM into the source SPSC ring.

It is separated from MediaRt so WASAPI source I/O jitter cannot become StreamClock jitter.

#### UsbReactorThread / TransportContext

Owns asynchronous USB/WinUSB/libusb completion servicing and packet reassembly.

Independent endpoint operations are concurrently pending. The reactor forwards complete packets to HciContext and never calls Bumble Host logic directly.

### Deployment flexibility

The separation above freezes execution **domains**, not an eternal physical-thread count. If later profiling proves that two serial contexts can safely share a thread, runtime composition may remap them without changing module ownership.

---

## 8. Message passing and queue semantics

Cross-context design follows:

> **prefer data passing over data sharing**

Concrete Rust queue libraries are not yet selected.

The semantic queue classes are:

- **Capture -> Media:** bounded SPSC audio buffer. Source latency is bounded; old source PCM is less valuable than blocking Media RT.
- **Media / Bridge frame store:** bounded target-domain encoded frames keyed by FrameId.
- **Bridge -> HCI:** bounded non-blocking submission mailbox. No credit wait on the RT path.
- **HCI <-> Transport:** reliable bounded packet transport. HCI Event loss is not silently tolerated.
- **Plane -> Control reports:** reliable low-rate typed mailbox; high-frequency telemetry is aggregated first.
- **Control -> HCI:** asynchronous command/request-reply interface.

Queue capacity, exact overflow policy and Rust implementation crate remain Review decisions, not base architecture.

---

## 9. Media/HCI data contract

Media produces transport-agnostic encoded frame groups.

Conceptually:

```text
EncodedStereoFrame
  frame_id
  left payload
  right payload
```

Media-owned FrameId identifies the target media interval.

HCI Host separately owns transport sequence numbers and HCI ISO packetization.

Do not equate Media FrameId with HCI ISO packet sequence.

Control establishes the semantic stream binding between logical Left/Right and the relevant CIS transport resources. Media never receives raw CIS handles.

---

## 10. Reference-derived TX principles

The following are considered established reference precedents rather than local invention:

- **AOSP:** independent realtime media interval; ASRC belongs before LE codec output.
- **Zephyr:** complete SDUs enter per-CIS Host queues; controller credits gate queue drain, not source time.
- **Bumble-RS:** Host owns ISO packetization/sequence and flushes queued traffic on completion events.
- **PipeWire:** grouped ISO interval scheduling, bounded fill and valid fallback media for missed intervals.
- **Linux/BlueZ:** Control binds a stream to a transport handle; normal media packets bypass the BAP state machine.
- **nRF Audio:** latency is bounded aggressively; large timing errors are corrected discretely rather than hidden behind indefinite backlog.
- **Espressif:** per-stream queues feed a group-paced transmitter that can later align to controller timing.
- **BTstack:** Host credit/readiness accounting is useful precedent; its lite-demo credit-driven source generation is not adopted as the Windows media-clock architecture.

---

## 11. Frozen invariants

1. Media time never waits for HCI credits.
2. Number Of Completed Packets only releases controller capacity.
3. RuntimeState is shared-readable and Control-single-writer.
4. Plane internals are owned by one serial execution context.
5. Cross-context communication prefers typed messages over shared mutable objects.
6. WASAPI source clock and LE StreamClock are distinct clock domains.
7. ASRC maps source time into target time; it is not the clock authority.
8. Media and IsoTxBridge reference one StreamClock grid.
9. Media produces encoded transport-agnostic frames.
10. HCI Host owns transport sequence, fragmentation, HCI ISO framing and controller credits.
11. IsoTxBridge policy is replaceable.
12. Backlog is bounded; stale media is not allowed to accumulate latency without limit.
13. Transport does not synchronously round-robin independent RX endpoints.
14. HCI Host and USB transport are separate execution domains.
15. Media RT never blocks on USB, HCI credit, GATT, logging or general-purpose locks.
16. The Experiment/R5 archive is historical evidence, not a source of implicit new architecture requirements.

---

## 12. Explicitly unresolved

The following are intentionally **not** frozen by this document:

- exact Rust executor/runtime;
- exact mailbox/channel crate;
- concrete queue capacities;
- exact Media/Bridge lead times;
- exact fallback-silence vs drop thresholds;
- exact controller-anchor acquisition/filtering algorithm;
- AOSP-derived ASRC algorithm vs another replaceable DSP implementation;
- exact Windows realtime/MMCSS policy;
- final mapping of execution contexts to physical threads after profiling;
- detailed HCI Host internal actor/module decomposition;
- asynchronous USB backend API choice.

These must be decided in later Review steps against reference implementations and HIL measurements.

---

## 13. Review implementation order

The architecture suggests the following broad dependency order:

1. freeze execution-context and mailbox contracts;
2. implement asynchronous L3/L2 transport;
3. establish single-owner HCI Host context;
4. implement StreamClock and IntervalGroup IsoTxBridge;
5. rebuild synthetic Media path;
6. validate clean synthetic First Sound;
7. port Relay WASAPI source path;
8. add clock discipline / ASRC;
9. integrate lifecycle/recovery;
10. profile, then reconsider physical-thread placement.

This order is guidance, not a new Round numbering scheme.
