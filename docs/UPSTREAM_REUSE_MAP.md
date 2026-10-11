# Upstream reuse/oracle map

## Direct reuse candidates

### bumble-rs @ bbac2a6803b8cab0920ab725a23aa408fc4fed85

Primary Rust host substrate.

Relevant source anchors from the audited snapshot:

- `bumble-transport/src/usb.rs` around line 567:
  - HCI ACL and HCI ISO TX share USB Bulk OUT.
- `bumble-host/src/lib.rs` around 2750-2990:
  - Host power-on/reset/capability initialization.
  - **Do not copy the unconditional HCI Reset path into the first BE200 attach experiment.**
- `bumble-host/src/lib.rs` around 5117-5200:
  - LE Set CIG Parameters / LE Create CIS.
- `bumble-host/src/lib.rs` around 5350-5525:
  - Setup/Remove ISO Data Path and `send_iso_sdu`.
- `bumble-profiles/src/pacs.rs` around 245-288:
  - `PublishedAudioCapabilitiesServiceProxy`.
- `bumble-profiles/src/ascs.rs` around 1056-1127:
  - `AudioStreamControlServiceProxy`, discovery and control-point writes.
- `bumble-codecs`:
  - first-sound LC3 path.

Preferred integration: pinned upstream dependency / adapter, not source copy.

## Existing project code to preserve

### BE200AudioHub main @ 04a089e6a3d9476350e856ea322953e3f934a087

Keep as validated hardware substrate:

- `src/boot.rs`
- `src/firmware.rs`
- `src/intel.rs`
- `src/usb.rs`
- `src/hci_wire.rs`
- `src/hci_capabilities.rs`
- `src/ble_scan.rs`

These are R1 evidence-bearing code. R2 wraps them; it does not rewrite them.

## LE-Audio-Relay as a reference oracle

Canonical snapshot:

`STanJK/LE-Audio-Relay @ 3f75ae1a487bf92aaaba8dfe37fac36ab6576f59`

Frozen under:

`reference/le-audio-relay/`

Reference behavior to port into Rust:

- `Routing/ProcessLoopbackSource.cs`
- `Routing/AudioFormatPolicy.cs`
- `Timing/SpscPcmRing.cs`
- `Timing/PcmRelayBoundary.cs`
- `Timing/ProvisionalPositiveDriftGuard.cs`
- `Supervision/WorkerProtocol.cs`
- `Supervision/SupervisorState.cs`
- `Telemetry/RouteTelemetry.cs`

The snapshot is not compiled. There is no .NET sidecar in the target product architecture.

Rust target:

- `crates/audio-capture-win32`: Win32/WASAPI process loopback.
- `crates/audio-bridge`: PCM normalization/channel mapping.
- `crates/iso-scheduler`: ring/cushion/drift/pacing semantics.

## Oracles, not product dependencies

### Zephyr BAP Unicast Client

`samples/bluetooth/audio/bap_unicast_client/src/main.c`

Audited happy-path anchors:

- ~649 `bt_bap_stream_config`
- ~740 `bt_bap_unicast_group_create`
- ~766 `bt_bap_stream_qos`
- ~789 `bt_bap_stream_enable`
- ~855 `bt_bap_stream_start`
- `stream_tx.c` ~107 `bt_bap_stream_send`

Use this sequence to define our BAP client coordinator.

### AOSP Fluoride

`system/bta/le_audio/state_machine.cc`

Audited failure/recovery anchors:

- ~216 StartStream
- ~227 CIG creation gate
- ~549 CIG create notification
- ~571 recovering after CIG failure
- ~678 Setup ISO Data Path result
- ~1008 CIS established
- ~1194 CIS disconnected
- ~1575 CIG creation implementation
- ~1821 ReleaseDataPath

Use AOSP for recovery policy, never as a source-level dependency.

### qarnet/le-audio-receiver

Use `scripts/bap_central*.py` for ownership, pairing/reconnect, transport acquisition, 10 ms writer lifecycle, and ordered teardown patterns.

### Linux btusb

Use `drivers/bluetooth/btusb.c` only as a transport-behavior oracle. GPL source is not copied into the product.
