# R2 stack-integration bootstrap

This branch preserves the R1 hardware-proven code and introduces the product-facing stack boundary.

## Frozen facts

- R1.5b Windows OEM Intel firmware bring-up remains canonical and is not rewritten here.
- R1.6 operational HCI capability/scan path remains canonical and is not rewritten here.
- MT7925 remains the normal Windows Bluetooth controller.
- BE200 interface 0 remains the owned WinUSB controller path.
- No new code in this branch is allowed to silently power-cycle the BE200 or reload firmware.
- Bumble-RS is pinned as an upstream implementation oracle at `bbac2a6803b8cab0920ab725a23aa408fc4fed85`.
- LE-Audio-Relay behavior is anchored at `3f75ae1a487bf92aaaba8dfe37fac36ab6576f59`, but its .NET runtime is **not** part of the product build.

## Product architecture

The final product backend is Rust-only.

```text
Windows application audio
        |
        v
audio-capture-win32
  Rust / Win32 process loopback
        |
        v
audio-bridge
  48 kHz Float32 stereo -> stream channels
        |
        v
iso-scheduler
  LC3 quantum / pacing / drift / backpressure
        |
        +-----------------------+
                                |
R1 verified substrate           |
 firmware / WinUSB              |
        |                       |
        v                       v
  be200-runtime ----------> bumble-adapter
                                |
                                v
                            bap-client
                                |
                                v
                       ACL / GATT / SMP /
                       PACS / ASCS / CIG /
                       CIS / HCI ISO
                                |
                                v
                              BE200
```

`device-policy` supplies peer/topology/QoS decisions. `recovery-supervisor` owns bounded cross-layer recovery.

## Reference-only sources

`reference/le-audio-relay` contains a frozen source snapshot of the already validated C# Process Loopback, ring, cushion, drift and supervision logic. It is an oracle for the Rust port and is intentionally outside every build graph.

First sound does **not** depend on Windows capture at all:

```text
fixed WAV / sine -> LC3 -> ISO -> BE200 -> peer
```

Only after that path is green do we implement and connect `audio-capture-win32`.

## Rule

We do not copy protocol-stack code merely because it is copyable. Prefer a pinned dependency or a very small adapter so upstream ownership remains visible.

Code that is not yet safe to connect to hardware is represented as compileable interfaces plus pseudocode and oracle source ranges. The next phase replaces those placeholders one edge at a time with HIL validation.
