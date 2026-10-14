# Source Inventory and Trust Boundaries

This file records the source snapshots used by the reference index and the limits of what each snapshot can prove.

## Bumble-RS

Repository: `chaitanyarahalkar/bumble-rs`

Pinned revision already validated by Experiment/R5:

`bbac2a6803b8cab0920ab725a23aa408fc4fed85`

Local reference snapshot: `bumble-rs-main.zip`.

Strong uses:

- HCI packet model and `Device` state machine;
- `HostTransport` adapter boundary;
- ACL/ISO packet queues;
- ATT/GATT/L2CAP/SMP/privacy parsers and state machines;
- PACS/ASCS/BAP profile parsers.

Explicit exclusion:

- `bumble-transport/src/host.rs::ExternalHost` is not a product orchestration reference because it creates a blocking reader thread and uses synchronous command waits.

## AOSP Bluetooth

Local snapshot: `AOSP-Bluetooth.zip`.

The supplied tree is sparse. It contains the LE Audio client/state-machine/media pieces used in this index, but not every lower-level GD/HCI implementation.

Strong uses:

- LE Audio semantic state machine;
- media worker/tick lifecycle;
- ASRC placement;
- stream prepare/start/stop gating;
- CIG/CIS / ISO Data Path semantic ordering.

Do not invent missing `IsoManager::SendIsoData` internals from this snapshot.

## Zephyr

Local snapshot: `zephyr-main.zip`.

Strong uses:

- Host-side ISO SDU queueing;
- fragmentation;
- controller packet capacity;
- Number Of Completed Packets handling;
- connection fairness.

Use Zephyr Host implementation as the main cross-check for Bumble-RS transport accounting.

## BTstack

Local snapshot: `btstack-master.zip`.

Strong uses:

- ISO fragmentation and completion accounting cross-check.

Negative reference:

- `example/le_audio_unicast_gateway_lite.c` schedules next media work from can-send readiness. That demo pattern must not become BE200AudioHub’s media clock.

## libusb / rusb

Local snapshots:

- `libusb-master.zip`
- `rusb-master.zip`

Strong uses:

- asynchronous USB transfer ownership;
- event handling;
- cancellation;
- safe device/interface lifecycle plus raw FFI escape hatch.

These sources define L3 mechanics only, not Bluetooth semantics.

## PipeWire

Local snapshot: `pipewire-master.zip`.

Strong use:

- grouped ISO pacing with one timer;
- absolute interval scheduling;
- controller queue-fill correction;
- silence fallback;
- group stream alignment.

Primary reference for the initial IsoTxBridge Interval Group Scheduler.

## BlueZ

Local snapshot: `bluez-master.zip`.

Strong use:

- BAP semantic state separated from ISO socket I/O;
- stream state / fd binding boundary.

## nRF Audio

Local snapshot: `sdk-nrf-main.zip`.

Strong use:

- controller TX timestamp anchoring;
- stream-group send;
- small drift correction versus discrete skip/drop;
- avoiding deep latency buffers.

## Espressif ESP-GMF Bluetooth audio

Local snapshot: `esp-gmf-main.zip`.

Strong use:

- LE TX group timer;
- shared sequence/grid;
- startup prime;
- shadow members;
- controller TX sync and grid skip.

## Linux Bluetooth ISO

Source available as standalone `net/bluetooth/iso.c` in the shared addition pack.

Strong use:

- userspace submits an SDU;
- kernel owns ISO SDU sequence/header;
- packetization boundary.

Limitation:

- same-version surrounding HCI/core source is not bundled, so this standalone file is **not** used as the primary oracle for controller-credit accounting.

## liblc3

Local snapshot: `liblc3-main.zip`.

Strong use:

- frame/sample sizing;
- encoder/decoder state sizing;
- encode/decode API boundary;
- prepare-time codec allocation.

## LE-Audio-Relay

Repository: `STanJK/LE-Audio-Relay`.

Main tree inspected during index construction:

`3d208477a03de9ceb15710f8adb90b02d0c687b7`

Strong use:

- Windows Process Loopback setup;
- SPSC PCM boundary;
- endpoint/power lifecycle observation;
- generation invalidation;
- worker supervision/backoff.

Do not reuse its destination render/keepalive behavior as the BE200AudioHub media destination architecture.

## BE200AudioHub Experiment/R5

Immutable source: `deprecated/r5/`.

Use it for:

- verified BE200 USB/HCI quirks;
- exact Buds3 Pro protocol discoveries;
- HIL-proven command sequences;
- regression fixtures.

Do not treat its synchronous USB/service loop or credit-coupled media timing as architectural precedent.
