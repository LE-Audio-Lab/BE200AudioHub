# R2 stack-integration bootstrap

This branch preserves the R1 hardware-proven code and introduces the product-facing stack boundary.

## Frozen facts

- R1.5b Windows OEM Intel firmware bring-up remains canonical and is not rewritten here.
- R1.6 operational HCI capability/scan path remains canonical and is not rewritten here.
- MT7925 remains the normal Windows Bluetooth controller.
- BE200 interface 0 remains the owned WinUSB controller path.
- No new code in this branch is allowed to silently power-cycle the BE200 or reload firmware.
- Bumble-RS is pinned as an upstream implementation oracle at `bbac2a6803b8cab0920ab725a23aa408fc4fed85`.
- LE-Audio-Relay reuse is anchored at `3f75ae1a487bf92aaaba8dfe37fac36ab6576f59`.

## New module boundary

```text
R1 verified substrate
  firmware / WinUSB / operational controller
                  |
                  v
           be200-runtime
                  |
                  v
       bumble-rs Host substrate
                  |
          +-------+--------+
          |                |
          v                v
      bap-client       device-policy
          |                |
          +-------+--------+
                  v
           iso-scheduler
                  |
                  v
            audio-bridge
                  |
                  v
        LE-Audio-Relay audio source
```

## Rule

We do not copy protocol-stack code merely because it is copyable. Prefer a pinned dependency or a very small adapter so upstream ownership remains visible.

Code that is not yet safe to connect to hardware is represented as compileable interfaces plus pseudocode and oracle source ranges. The next phase replaces those placeholders one edge at a time with HIL validation.
