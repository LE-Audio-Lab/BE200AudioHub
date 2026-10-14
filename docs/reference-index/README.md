# Reference Index

This directory is the implementation-facing reference library for BE200AudioHub Review/Refactor work.

It is intentionally more precise than `docs/reference/README.md`: entries are indexed to **upstream project → source file → function/type**, with an explicit statement of what BE200AudioHub should adopt, what it must not copy, and which deterministic tests should constrain the resulting implementation.

## How to use this library

For a new module:

1. Start from the matching domain document below.
2. Read the **BE200AudioHub contract** first. Architecture remains authoritative; upstream code is evidence, not authority.
3. Follow the listed upstream anchors in the stated read order.
4. Implement only the responsibilities assigned to the target module.
5. Convert the listed invariants into deterministic tests before or alongside implementation.
6. Use a second upstream implementation where the index marks a behavior as cross-checked.
7. Escalate to HIL only when the remaining uncertainty is an external-world assumption: Windows, libusb/WinUSB, BE200 firmware/controller behavior, Buds behavior, WASAPI scheduling, real clock drift, or acoustic output.

## Index

| Domain | Index | Primary references |
|---|---|---|
| Source snapshots and trust boundaries | [source-inventory.md](./source-inventory.md) | all |
| Execution contexts, ownership, mailbox semantics | [runtime-execution.md](./runtime-execution.md) | AOSP, LE-Audio-Relay |
| Async USB and transport boundary | [transport-usb.md](./transport-usb.md) | libusb, rusb, Review-1 |
| HCI Host ownership and controller flow control | [hci-host.md](./hci-host.md) | Bumble-RS, Zephyr, BTstack |
| ACL / L2CAP / ATT / GATT | [acl-att-gatt.md](./acl-att-gatt.md) | Bumble-RS, Zephyr |
| SMP / keys / privacy | [security-privacy.md](./security-privacy.md) | Bumble-RS |
| PACS / ASCS / ASE / CIG / CIS semantic control | [le-audio-control.md](./le-audio-control.md) | AOSP, Bumble-RS, BlueZ |
| ISO SDU / sequence / fragmentation / completions | [iso-data-plane.md](./iso-data-plane.md) | Bumble-RS, Zephyr, Linux, BTstack |
| Media worker / clock / ASRC placement | [media-clock.md](./media-clock.md) | AOSP, nRF Audio |
| Interval-group Bridge scheduler | [bridge-scheduler.md](./bridge-scheduler.md) | PipeWire, Espressif, nRF Audio |
| Windows Process Loopback capture | [windows-audio.md](./windows-audio.md) | LE-Audio-Relay |
| Lifecycle / generation / recovery | [lifecycle-recovery.md](./lifecycle-recovery.md) | LE-Audio-Relay, AOSP |
| LC3 wrapper and codec boundary | [codec-lc3.md](./codec-lc3.md) | liblc3, AOSP |
| Test oracle map | [test-strategy.md](./test-strategy.md) | all |

## Authority order

When references disagree, use this order:

1. **BE200AudioHub frozen architecture contracts** in `docs/design/`.
2. Bluetooth / LE Audio specification semantics as represented by multiple mature stacks.
3. Production upstream behavior (AOSP, Zephyr, PipeWire, BlueZ, Linux).
4. Bumble-RS where its substrate has already been HIL-validated in Experiment/R5.
5. Demonstration/sample code only as a narrow implementation example.

Do not change Plane ownership because an upstream project chose a different process/thread layout.

## Reference classes

- **Primary oracle** — suitable to shape module behavior directly.
- **Cross-check oracle** — confirms a boundary/invariant implemented differently elsewhere.
- **Negative reference** — useful specifically because a pattern must *not* be copied.
- **Platform precedent** — shows how to integrate with Windows/USB/audio, but does not define Bluetooth semantics.

## What “cargo-test constrained” means here

A domain is considered deterministic-test constrained when external I/O can be replaced with fixtures, fake clocks, fake backends, or event replays while preserving the architectural contract.

Typical examples:

- packet parser and framing;
- command/event transaction state machines;
- credit accounting;
- fragmentation;
- ATT/GATT discovery;
- privacy algorithms;
- PACS/ASCS parsers;
- ASE/CIG/CIS planning;
- StreamClock math;
- group scheduler decisions;
- backlog/drop/fallback policy;
- ASRC estimator/control;
- lifecycle reducer and recovery escalation.

HIL remains necessary for assumptions about real Windows scheduling, USB completion behavior, controller firmware quirks, RF, earbuds, and acoustics.

## Maintenance rule

Whenever a Review implementation chooses an upstream precedent:

- add or update the relevant function-level anchor here;
- record the BE200AudioHub mapping;
- record any intentional deviation;
- add the corresponding deterministic test class to [test-strategy.md](./test-strategy.md).

This directory is an index, not a source mirror. Do not copy large upstream code blocks into documentation.
