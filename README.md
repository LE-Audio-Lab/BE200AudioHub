# BE200AudioHub — Review / Refactor

The Experiment phase (R1 through R5.3) is sealed. Its complete final source tree is preserved under `deprecated/r5/`, corresponding to experiment commit `34f46809105567259f9614f648eadd0451c46b72`.

The repository root is now the Review/Refactor baseline.

## Frozen architecture

- **L0 — LifeCycle / Orchestration**
  - desired state, generation, session lifecycle, recovery and ordered startup/teardown
- **L1 — Control Plane**
  - LE Audio semantics and the single-writer RuntimeState model
- **L1 — Media Plane**
  - WASAPI ingress, clock-domain adaptation, ASRC, channel mapping, LC3 and encoded SDU production
- **L1 — HCI Host Plane**
  - controller-facing Bluetooth Host semantics, ACL/ATT, CIS/ISO queues, sequence/framing and controller credits
- **Bridge — IsoTxBridge**
  - explicit Media↔HCI boundary; first implementation follows interval-group scheduling
- **Cross-cutting — StreamClock**
  - one authoritative session-scoped media timeline shared by Media and IsoTxBridge
- **L2 — Transport Plane**
  - asynchronous HCI packet transport and packet reassembly; no Bluetooth session semantics
- **L3 — Platform / Device Backend**
  - Windows/libusb/WinUSB/BE200-specific bootstrap and raw transfer primitives

The execution/thread model is intentionally **not frozen yet**.

See `docs/architecture/ARCHITECTURE.md` and `docs/architecture/DATA_FLOW.md`.
