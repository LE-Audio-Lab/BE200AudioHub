# BE200AudioHub — Review / Refactor

The Experiment phase (R1 through R5.3) is sealed. Its complete final source tree is preserved under `deprecated/r5/`, corresponding to experiment commit `34f46809105567259f9614f648eadd0451c46b72`.

The repository root is the Review/Refactor baseline.

## Architecture

- **L0 — LifeCycle / Orchestration**
- **L1 — Control Plane**
- **L1 — Media Plane**
- **L1 — HCI Host Plane**
- **Bridge — IsoTxBridge**
- **Cross-cutting — authoritative StreamClock**
- **L2 — Transport Plane**
- **L3 — Platform / Device Backend**

The initial runtime mapping uses five execution domains/threads: Core, HCI Host, Media RT, WASAPI Capture and USB Reactor. Modules depend on execution contexts rather than permanent physical-thread identities.

## Documentation

Start here:

1. [Documentation index](./docs/README.md)
2. [Base Design Principles](./docs/design/BASE_DESIGN_PRINCIPLES.md)
3. [Architecture](./docs/design/ARCHITECTURE.md)
4. [Data Flow](./docs/design/DATA_FLOW.md)

Historical Experiment/R5 material remains immutable under `deprecated/r5/`.
