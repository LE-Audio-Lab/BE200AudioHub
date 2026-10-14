# Documentation

Active documentation is organized by responsibility.

- **design/** — current architecture contracts and design principles. These documents describe the Review/Refactor target and are authoritative for new implementation work.
- **reference/** — external implementation references and the rules for adopting ideas from AOSP, Zephyr, Bumble-RS, PipeWire, BTstack, Linux/BlueZ, nRF Audio and Espressif.
- **history/** — navigation for the sealed Experiment/R5 material. Historical source and Round documents remain byte-for-byte under `deprecated/r5/` and are not evolved in place.

## Read first

1. [Base Design Principles](./design/BASE_DESIGN_PRINCIPLES.md)
2. [Architecture](./design/ARCHITECTURE.md)
3. [Execution Model](./design/EXECUTION_MODEL.md)
4. [Review-1 Runtime + Async Transport](./design/REVIEW1_RUNTIME_TRANSPORT.md)
5. [Data Flow](./design/DATA_FLOW.md)
6. [Reference implementations](./reference/README.md)

## Archive rule

`deprecated/r5/` is an immutable experiment snapshot. Historical documents are indexed from `history/`, but are not moved or rewritten just to fit the new documentation taxonomy.
