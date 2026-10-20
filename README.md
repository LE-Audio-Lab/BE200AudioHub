# BE200AudioHub

**BE200AudioHub is an experimental Windows user-space Bluetooth LE Audio Host for Intel BE200-class controllers.**

It bypasses the Windows Bluetooth LE Audio Host path for the product data plane and drives a controller directly from Rust:

```text
Windows applications
    ↓
WASAPI Process Loopback
    ↓
bounded PCM relay
    ↓
LC3
    ↓
LE Audio Control / BAP
    ↓
HCI ISO
    ↓
Intel BE200
    ↓
LE CIS
    ↓
Galaxy Buds3 Pro
```

The project started as an attempt to make LE Audio usable on Windows and evolved into a complete Host-side unicast playback implementation with real-hardware HIL, dual-CIS audio, live Windows application capture, explicit lifecycle ownership, and controller-level latency instrumentation.

> **Current status:** real dual-ear live audio works on hardware. The Host-side unicast playback path is functional end-to-end. This is still an experimental project, not a production Windows Bluetooth driver.

---

## Current capabilities

The current implementation has exercised the following path on real Intel BE200 + Galaxy Buds3 Pro hardware:

- Intel OEM-signed BE200 firmware bootstrap and direct HCI access from Windows;
- resetless Host attach;
- LE scanning and dual-member discovery;
- SMP pairing, persistent bond storage, LTK/IRK reuse and controller privacy;
- coordinated-set handling;
- encrypted dual LE ACL links;
- ATT / GATT transactions;
- PACS / ASCS / CAS / CSIS discovery and state handling;
- LC3 codec configuration;
- BAP QoS configuration;
- CIG creation and dual CIS establishment;
- ISO Data Path setup/removal;
- logical stream routing isolated from raw CIS handles;
- 48 kHz / 10 ms / 100-byte-per-ear LC3;
- bounded HCI ISO admission and completion accounting;
- Windows Process Loopback capture;
- live Float32 stereo PCM → LC3 → dual-CIS playback;
- simple egui control surface with Start / Stop / live gain;
- explicit graceful and BE200-specific controller-clean teardown policies;
- real First Sound HIL and sustained live-audio HIL.

The current product path is **downlink playback only**. Headset microphone uplink, telephony, Broadcast Audio/Auracast and general multi-vendor compatibility are outside the current scope.

---

## Current live data path

```text
Windows application render
        ↓
sacrificial physical Windows render endpoint
        ↓
Process Loopback capture
        ↓
48 kHz Float32 stereo
        ↓
bounded SPSC PCM ring
        ↓
Media / StreamClock
        ↓
live gain
        ↓
LC3 L / R
        ↓
EncodedStereoFrame
        ↓
IsoTxBridge
        ↓
logical HCI ISO submit lane
        ↓
HCI Host ISO framing / sequence / credits
        ↓
BE200
        ↓
CIG / dual CIS
        ↓
Galaxy Buds3 Pro
```

The Windows Bluetooth audio endpoint is not used for this path.

---

## Low-latency experiment

The current `ExtremeLowLatency` profile keeps the Buds3 Pro-required presentation delay while reducing controller transport latency:

```text
SDU interval              10 ms
PHY                       2M
RTN                       2
Max Transport Latency     10 ms
Presentation Delay        40 ms
```

Real controller-selected CIS timing:

| Profile | RTN | NSE | Controller C→P transport |
|---|---:|---:|---:|
| experimental RTN0 | 0 | 1 | 1.458 ms |
| experimental RTN1 | 1 | 2 | 3.066 ms |
| current RTN2 | 2 | 3 | 4.674 ms |

RTN0 proved too fragile for practical RF conditions. RTN2 is the current low-latency/robustness experiment point.

Galaxy Buds3 Pro reports, for the current codec configuration:

```text
supported presentation delay = 40000..40000 us
preferred presentation delay = 40000..40000 us
```

So the peer currently fixes Presentation Delay at **40 ms**.

### Host-side latency observations

The Windows Process Loopback endpoint currently allocates:

```text
480 frames @ 48 kHz = 10.000 ms
```

A temporary QPC instrumentation run measured a 10 ms packet span with roughly **0.4–1.9 ms** callback tail age in the sampled run.

With the current 10 ms PCM target cushion, the working nominal timing model is approximately:

```text
Process Loopback sample-phase + delivery   ~6 ms
PCM target cushion                         10 ms
LC3 10 ms frame sample-phase               ~5 ms
CIS transport                              4.674 ms
Presentation Delay                         40 ms
------------------------------------------------
nominal scheduled/Host timing model       ~65 ms
```

This is **not yet a physical acoustic E2E measurement**. A future measurement will reuse the project's earlier physical latency-probe methodology.

ASRC-based clock discipline and a smaller steady-state PCM cushion are planned follow-up work; they are not claimed as complete here.

---

## Architecture

The current root implementation is organized by ownership boundary rather than protocol call sequence.

```text
L0 Core / LifeCycle
    |
    +----------------------+-----------------------+
    |                      |                       |
    v                      v                       v
L1 Control             L1 Media               L1 HCI Host
BAP / ASCS             PCM / LC3              HCI / ACL / ATT
PACS / CSIS            StreamClock            CIG / CIS / ISO
semantic state         IsoTxBridge            credits / raw handles
    |                      |                       |
    +----------------------+-----------------------+
                           |
                           v
                    L2 Transport
                           |
                           v
                 L3 Windows / BE200
```

Major source areas:

```text
src/
├─ core/
│  ├─ command/
│  ├─ condition/
│  └─ lifecycle/
├─ plane/
│  ├─ l1_control/
│  ├─ l1_media/
│  ├─ l1_hci/
│  └─ l2_transport/
├─ platform/
│  └─ windows/
├─ runtime/
├─ gui.rs
└─ main.rs
```

### Frozen engineering invariants

The current implementation treats these as non-regression contracts:

1. **Media time never waits for HCI credits.**
2. Number Of Completed Packets releases capacity; it never advances media time.
3. HCI owns raw CIS handles, ISO sequence numbers, fragmentation and controller credits.
4. Media never observes raw CIS handles.
5. Logical routes are session-fenced.
6. Route acknowledgement gates the transport-neutral stream binding.
7. Left/right audio is submitted as one logical interval group.
8. Backpressure drops stale/current audio instead of shifting media time.
9. Queues are bounded.
10. Mutable subsystem state has a single owner.
11. Reliable state-changing traffic is never silently dropped.
12. L2 Transport moves/reassembles packets but does not own BAP/CIS semantics.
13. L3 owns raw Windows/device I/O.
14. Normal teardown and failure rollback are distinct policies.

The architectural principle that drove the media rewrite is:

> **Do not let transport backpressure become media time.**

---

## Teardown policy and known BE200 defect

The architecture remains protocol-graceful by default.

Normal graceful shutdown is:

```text
stop Media
→ drain / stop Bridge submissions
→ unbind route
→ ASCS Release
→ ASE RELEASING
→ Remove ISO Data Path
→ Disconnect CIS
→ Remove CIG
→ Disconnect ACL
→ clear controller privacy/FAL
```

A long-session investigation found a separate repeatable defect on the current BE200 userspace-Host path: after roughly tens of seconds, both LE ACLs can remain logically alive while **controller → Host HCI ACL RX stops**, even though HCI Event traffic and Host → Controller ACL TX remain alive.

The project deliberately does **not** claim that Intel firmware, BE200 silicon, USB DMA, the peer or the custom Host has been proven as the internal root cause.

For current BE200 daily use an explicit workaround exists:

```text
--ungraceful-teardown
```

It skips peer-facing ASCS Release during full-session shutdown and retires controller-owned ISO/CIS/CIG/ACL resources directly. It is a hardware-path workaround, not the Bluetooth architectural default.

See [BE200 Teardown Policy](./docs/design/BE200_TEARDOWN_POLICY.md) and the archived investigation under `deprecated/graceful-teardown-bug/`.

---

## GUI

The current GUI is intentionally small:

- Start
- Stop
- live volume/gain slider

The gain value is read by the live Media path before LC3 encoding, without restarting the LE Audio session.

Example launch:

```powershell
.\target\debug\be200hub.exe gui `
  --sfi <path-to-intel-oem-signed.sfi> `
  --ddc <path-to-intel.ddc> `
  --bond-root <bond-store-directory> `
  --name "Galaxy Buds3 Pro (...)" `
  --connect-seconds 12 `
  --ungraceful-teardown `
  --extreme-low-latency
```

Intel proprietary firmware files and private bond/key material are **not distributed by this repository**.

---

## Build and test

Rust workspace:

```powershell
cargo test --workspace --locked
cargo check --workspace --locked
cargo build --workspace --locked
```

Current software gate at the mainline merge checkpoint:

```text
416 tests passed
cargo check PASS
cargo build PASS
```

Software tests are not treated as substitutes for real-hardware HIL. HIL claims in the documentation are made only from explicit runs on the target hardware.

---

## Real-hardware milestones

| Milestone | Status |
|---|---|
| BE200 firmware bootstrap / HCI / scanning | HIL PASS |
| dual-member pairing / bond / privacy | HIL PASS |
| dual encrypted ACL + PACS/ASCS/CAS | HIL PASS |
| codec / QoS / CIG / dual CIS | HIL PASS |
| ISO Data Path lifecycle | HIL PASS |
| valid LC3 dual-ear First Sound | HIL PASS |
| V2 semantic Control + logical HCI ISO path | HIL PASS |
| Windows Process Loopback → LC3 → BE200 → Buds3 Pro | HIL PASS |
| sustained live data plane | HIL PASS |
| graceful long-session teardown | known BE200-path defect remains |
| ASRC + reduced PCM cushion | planned |
| physical acoustic E2E for current custom Host | planned |

---

## Documentation

Start here:

1. [Documentation index](./docs/README.md)
2. [Base Design Principles](./docs/design/BASE_DESIGN_PRINCIPLES.md)
3. [Architecture](./docs/design/ARCHITECTURE.md)
4. [Data Flow](./docs/design/DATA_FLOW.md)
5. [Live Audio Relay Migration](./docs/design/LIVE_AUDIO_RELAY_MIGRATION.md)
6. [BE200 Teardown Policy](./docs/design/BE200_TEARDOWN_POLICY.md)
7. [Engineering Timeline](./docs/Timeline/README.md)
8. [Bug / Fix Register](./docs/Timeline/BUG_FIX_REGISTER.md)
9. [Function-level Reference Index](./docs/reference-index/README.md)

Historical Experiment/R5 source and evidence remain frozen under `deprecated/r5/`.

The Round9/10 graceful-teardown forensic implementation is archived under `deprecated/graceful-teardown-bug/`.

---

## Scope and non-goals

BE200AudioHub is currently:

- Windows-focused;
- Intel BE200-class controller focused;
- Galaxy Buds3 Pro focused for product HIL;
- unicast sink/playback focused;
- a userspace Host experiment and engineering platform.

It is not currently:

- a general replacement Windows Bluetooth driver;
- a multi-vendor compatibility layer;
- a headset microphone / telephony implementation;
- a Broadcast Audio / Auracast implementation;
- a controller firmware project;
- an earbud firmware project.

---

## License

BE200AudioHub source code is licensed under the **Apache License, Version 2.0**.

See [LICENSE](./LICENSE) and [NOTICE](./NOTICE). Third-party dependencies and distribution-specific license boundaries remain governed by their respective licenses.
