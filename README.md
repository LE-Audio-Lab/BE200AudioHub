# BE200AudioHub

**BE200AudioHub is a Windows user-space LE Audio host experiment and refactor project for Intel BE200-class controllers, built around a Rust Bluetooth host path rather than the Windows Bluetooth audio stack.**

The original Experiment phase proved the complete control/data path against Galaxy Buds3 Pro, including real acoustic First Sound. The project has now moved into **Review / Refactor**: the protocol path is considered proven; the current work is rebuilding the software around explicit clock domains, asynchronous transport, bounded queues, and replaceable scheduling policy.

> **Status:** architecture refactor in progress. Not a daily-use LE Audio driver yet.

---

## What has been proven

The sealed Experiment/R5 implementation demonstrated, on real hardware:

- Intel BE200 bootstrap and direct HCI access from Windows;
- persistent bond / privacy / stored-LTK recovery;
- coordinated-set discovery and dual encrypted ACLs;
- PACS / ASCS / CAS / CSIS handling;
- LC3 codec configuration and QoS planning;
- CIG creation and dual CIS establishment;
- ISO Data Path setup/removal;
- 48 kHz / 10 ms / 100-byte dual-mono LC3;
- real left/right acoustic First Sound on Galaxy Buds3 Pro;
- bounded dual-CIS streaming and complete teardown without mutating the saved bond.

The Experiment phase also exposed the architectural limit of the prototype: synchronous USB servicing and mixed clock/flow-control responsibilities can distort sustained playback even when the Bluetooth protocol path itself is correct.

That implementation is frozen under:

`deprecated/r5/`

The archived tree corresponds to Experiment commit:

`34f46809105567259f9614f648eadd0451c46b72`

It is historical evidence, not the current implementation target.

---

## Current design

The Review architecture separates lifecycle, media time, Bluetooth host state and physical transport.

```text
                           User / UI / CLI
                                  |
                                  v
+---------------------------------------------------------------+
| L0  LifeCycle / Orchestration                                 |
| generation | session | recovery | startup | teardown          |
+-----------------------------+---------------------------------+
                              |
               +--------------+--------------+
               |              |              |
               v              v              v
        +-------------+ +-------------+ +------------------+
        | L1 Control  | | L1 Media    | | L1 HCI Host      |
        |             | |             | |                  |
        | BAP / ASCS  | | WASAPI      | | HCI / ACL / ATT  |
        | PACS / CSIS | | ASRC        | | CIS / ISO        |
        | CIG / CIS   | | LC3         | | credits / queues |
        | semantics   | |             | |                  |
        +-------------+ +------+------+  +---------+--------+
                               |                  ^
                               v                  |
                       +---------------+          |
                       | IsoTxBridge   |----------+
                       | interval grid |
                       | stale/fallback|
                       +---------------+

                       +---------------+
                       | StreamClock   |
                       | authority     |
                       +---------------+
                         ^           ^
                         |           |
                       Media       Bridge

                                      |
                                      v
+---------------------------------------------------------------+
| L2  Transport Plane                                           |
| async HCI packet movement | reassembly | transport health     |
+-----------------------------+---------------------------------+
                              |
                              v
+---------------------------------------------------------------+
| L3  Platform / Device Backend                                 |
| Windows | libusb/WinUSB | BE200 bootstrap | raw transfers     |
+-----------------------------+---------------------------------+
                              |
                           Hardware
```

### L0 — LifeCycle / Orchestration

Owns desired state, generation, session lifetime, retry/backoff, recovery escalation and ordered startup/teardown.

Session is an L0-owned state-machine concept rather than a separate architecture layer.

### L1 — Control Plane

Owns high-level LE Audio semantics:

- identity, bond, privacy and security;
- PACS / ASCS / CAS / CSIS;
- codec and QoS planning;
- ASE state;
- semantic CIG/CIS and ISO-path lifecycle.

Control is also the **single writer** of the shared-readable `RuntimeState`.

### L1 — Media Plane

Owns the source/media clock domain:

```text
WASAPI Process Loopback
        |
        v
PCM source ring
        |
        v
Clock discipline / ASRC
        |
        v
target-domain PCM
        |
        v
channel map
        |
        v
LC3 L / R
        |
        v
EncodedStereoFrame(FrameId)
```

The Windows ingress side reuses lessons from LE Audio Relay. The target-domain side follows AOSP-style periodic media scheduling and explicit clock-domain adaptation.

Media never observes CIS handles, controller credits or USB endpoints.

### L1 — HCI Host Plane

Owns controller-facing Bluetooth host state:

- HCI commands/events;
- ACL / ATT transport;
- CIS primitives;
- ISO transport sequence numbers;
- HCI ISO framing / fragmentation;
- per-CIS queues;
- controller capacity and completion accounting.

Controller completion events release capacity; they **never advance media time**.

### IsoTxBridge

An explicit Media ↔ HCI architectural bridge.

The first implementation is an **Interval Group Scheduler**:

- one interval maps to one left/right encoded frame group;
- left/right are submitted as one logical group;
- backlog is bounded;
- stale audio is dropped instead of accumulating latency;
- missed slots may use Media-provided pre-encoded fallback SDUs;
- the bridge never waits on HCI credits or USB.

The scheduling policy is intentionally replaceable. A later controller-anchored scheduler can be introduced without changing the Media/HCI boundary.

### StreamClock

Each active stream group owns one authoritative media timeline.

There is one 10 ms grid shared by Media and IsoTxBridge. ASRC is an actuator that maps the WASAPI source clock into this target clock domain; ASRC itself is not the clock authority.

Initial authority may be local monotonic/QPC time. The same abstraction is designed to become controller-anchored later.

### L2 — Transport Plane

Owns asynchronous HCI packet movement and complete-packet reassembly.

Transport understands packet classes such as Command/Event/ACL/ISO, but not BAP/ASE/CIG/CIS semantics.

Independent RX endpoints must not be synchronously round-robin polled.

### L3 — Platform / Device Backend

Owns Windows/USB/BE200-specific details:

- libusb / WinUSB primitives;
- device and endpoint discovery;
- open/claim/cancel;
- raw asynchronous transfers;
- Intel controller bootstrap;
- BE200-specific hardware quirks;
- deterministic test backend.

---

## Execution model

The runtime follows the AOSP Handler principle:

> **Modules depend on a serial execution context, not permanently on a specific OS thread.**

The first deployment maps those contexts onto five physical threads:

| Execution domain | Initial responsibility |
| --- | --- |
| **Core** | L0 LifeCycle + Control + RuntimeState reducer |
| **HCI Host** | Bumble/host state, ACL/ATT, CIS/ISO, controller credits and host queues |
| **Media RT** | StreamClock deadlines, ASRC, LC3 and first-stage IsoTxBridge scheduling |
| **WASAPI Capture** | event-driven Process Loopback ingestion on the Windows audio clock |
| **USB Reactor** | asynchronous USB completion servicing and HCI packet reassembly |

This is a deployment choice, not a permanent module-level dependency.

---

## Frozen design invariants

The current Review baseline treats these as architectural contracts:

1. Media time never waits for HCI credits.
2. Number Of Completed Packets only releases controller capacity.
3. RuntimeState is shared-readable and Control-single-writer.
4. Plane internals are owned by one serial execution context.
5. Cross-context communication prefers typed messages over shared mutable objects.
6. WASAPI source clock and LE StreamClock are distinct clock domains.
7. ASRC maps source time into target time; it is not the clock authority.
8. Media and IsoTxBridge reference one StreamClock grid.
9. Media produces encoded transport-agnostic frame groups.
10. HCI Host owns transport sequence, fragmentation, HCI ISO framing and controller credits.
11. IsoTxBridge policy is replaceable.
12. Backlog is bounded; stale audio is not allowed to accumulate latency indefinitely.
13. Transport never synchronously round-robin polls independent RX endpoints.
14. HCI Host and USB transport are separate execution domains.
15. Media RT never blocks on USB, HCI credit, GATT, logging or general-purpose locks.

---

## Reference implementations

The refactor deliberately combines established ideas instead of inventing lower-level Bluetooth/media policy from scratch:

- **AOSP Bluetooth / LE Audio** — realtime media worker, ASRC placement, Handler-style execution contexts;
- **Zephyr Bluetooth Host** — per-CIS ISO queues, fragmentation and completion-driven credit release;
- **Bumble / Bumble-RS** — Rust Host substrate, ISO packetization and DataPacketQueue behavior;
- **PipeWire BlueZ5** — grouped ISO scheduling, bounded fill and fallback media;
- **Linux Bluetooth ISO + BlueZ** — userspace SDU / ISO-socket boundary and BAP transport binding;
- **Nordic nRF Audio** — controller-timestamp alignment and bounded-latency correction;
- **Espressif esp_bt_audio** — TX groups, interval timers and startup priming;
- **BTstack** — explicit Host readiness/credit accounting;
- **LE Audio Relay** — Windows Process Loopback, SPSC audio buffering and lifecycle/recovery experience.

A behavior with no strong reference precedent should remain explicitly unresolved until justified by system constraints or HIL evidence.

---

## Repository layout

```text
.
├─ src/
│  ├─ main.rs
│  └─ architecture/
│     ├─ lifecycle.rs
│     ├─ runtime_state.rs
│     ├─ control.rs
│     ├─ media.rs
│     ├─ hci_host.rs
│     ├─ iso_tx_bridge.rs
│     ├─ stream_clock.rs
│     ├─ transport.rs
│     ├─ platform.rs
│     └─ types.rs
│
├─ docs/
│  ├─ design/
│  ├─ reference/
│  └─ history/
│
└─ deprecated/
   └─ r5/        # frozen Experiment/R5 source + docs snapshot
```

---

## Documentation

Start with:

1. [Documentation index](./docs/README.md)
2. [Base Design Principles](./docs/design/BASE_DESIGN_PRINCIPLES.md)
3. [Architecture](./docs/design/ARCHITECTURE.md)
4. [Data Flow](./docs/design/DATA_FLOW.md)
5. [Reference implementations](./docs/reference/README.md)
6. [Experiment history index](./docs/history/README.md)

---

## Current phase

**Review / Refactor**

Immediate design/implementation work is focused on:

1. execution-context mailbox and ownership contracts;
2. asynchronous L3/L2 transport;
3. a single-owner HCI Host execution domain;
4. authoritative StreamClock + interval-group IsoTxBridge;
5. rebuilt synthetic media path;
6. clean synthetic First Sound validation;
7. WASAPI Process Loopback integration;
8. clock discipline / ASRC;
9. lifecycle and recovery integration;
10. profiling before reconsidering physical-thread placement.

The project is intentionally not carrying the Experiment/R5 synchronous architecture forward into the new root implementation.
