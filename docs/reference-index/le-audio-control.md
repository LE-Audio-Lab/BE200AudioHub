# LE Audio Control Plane Reference Index

## BE200AudioHub contract

Control owns LE Audio **semantics**, not raw HCI transport.

Control responsibilities include:

- coordinated-set/device model;
- PACS/ASCS/CAS/CSIS semantic discovery;
- codec selection/planning;
- QoS planning;
- ASE state transitions;
- semantic CIG/CIS lifecycle;
- semantic ISO Data Path lifecycle;
- stream prepare/start/teardown transaction;
- construction of transport-neutral `StreamBinding`.

Control must not:

- own USB/libusb;
- mutate Bumble Device directly;
- own HCI ISO sequence/credits;
- pace media from controller completions.

## Primary semantic oracle — AOSP LE Audio state machine

Project: AOSP Bluetooth

File: `system/bta/le_audio/state_machine.cc`

Read these anchors in lifecycle order.

### Codec / QoS / ASE configuration

- `PrepareAndSendCodecConfigToTheGroup()`
- `PrepareAndSendCodecConfigure()`
- `PrepareAndSendQoSToTheGroup()`
- `PrepareAndSendConfigQos()`

Use for:

- ordering of codec configuration before QoS;
- group/device ASE transaction structure;
- separating plan construction from HCI CIS creation.

### Enable / stream start

- `PrepareAndSendEnableToTheGroup()`
- `PrepareAndSendEnable()`
- `PrepareAndSendReceiverStartReady()`
- `ProcessGroupEnable()`

Use for:

- ASE enable semantics;
- receiver-ready gating;
- group transition only when member/ASE preconditions are satisfied.

### Disable / release

- `PrepareAndSendDisableToTheGroup()`
- `PrepareAndSendDisable()`
- `PrepareAndSendReleaseToTheGroup()`
- `PrepareAndSendRelease()`

Use for deterministic teardown state-machine ordering.

### CIG/CIS

- `CigCreate()`
- `CisCreateForDevice()`
- `CisCreate()`

Use for:

- semantic CIG creation after QoS is known;
- assigning/creating CIS for group members;
- separating logical stream topology from lower HCI primitive execution.

### ISO Data Path

- `PrepareDataPath()`
- `ProcessHciNotifSetupIsoDataPath()`
- `ProcessHciNotifRemoveIsoDataPath()`
- `RemoveDataPathByCisHandle()`

Use for:

- setup only after CIS exists;
- asynchronous callback-driven confirmation;
- teardown data path before final CIS/CIG release when required.

### Disconnect / CIG cleanup

- `ProcessHciNotifAclDisconnected()`
- `ReleaseCisIds()`
- `RemoveCigForGroup()`

Use for:

- invalidating group/CIS semantic state after lower connection loss;
- ensuring stale CIS identity is not retained across generations.

## AOSP client orchestration cross-check

File: `system/bta/le_audio/client.cc`

### `ConfigureStream(LeAudioDeviceGroup*, bool up_to_qos_configured)`

Reference for the high-level configuration transaction and the “configure only up to QoS” boundary.

### ISO callback anchors

- `IsoCigEventsCb()`
- `IsoCisEventsCb()`
- `IsoSetupIsoDataPathCb()`

Reference for:

- HCI callbacks becoming semantic state-machine events rather than direct application actions.

## AOSP group/CIS identity model

File: `system/bta/le_audio/device_groups.cc`

### `CigConfiguration::GenerateCisIds()`

Reference for generating group-local CIS identities from stream topology.

### `AssignCisIds()`

Reference for assigning logical CIS IDs to ASE/device streams.

### `AssignCisConnHandlesToAses()`

Reference for binding controller connection handles only after HCI creation succeeds.

### `HaveAllCisesDisconnected()`

Reference for teardown completion predicates.

## PACS parser oracle — Bumble-RS

Project: Bumble-RS

File: `bumble-profiles/src/pacs.rs`

### `PacCodecCapabilities::from_bytes()`

Primary fixture parser for PAC codec records/capability LTV payload.

### `PublishedAudioCapabilitiesServiceProxy::discover()`

Reference for GATT discovery → typed PACS proxy construction.

Use parser behavior; keep discovery orchestration behind Control/HCI typed boundary.

## ASCS / ASE FSM oracle — Bumble-RS

Project: Bumble-RS

File: `bumble-profiles/src/ascs.rs`

### Parsers

- `AseOperation::from_bytes()`
- `AseControlResponse::from_bytes()`
- `AseStatus::from_bytes()`

Use as golden parser oracles.

### ASE transitions

Inspect methods:

- `config_codec()`
- `config_qos()`
- `enable()`
- `receiver_start_ready()`
- `disable()`
- `receiver_stop_ready()`
- `update_metadata()`

Use for local ASE FSM invariants and invalid-transition tests.

### `AudioStreamControlService::establish_cis()`

Use as a profile-level cross-check for the handoff from ASE semantics to CIS establishment.

### Proxy anchors

- `discover()`
- `read_ase()`
- `event_from_notification()`

Use for deterministic notification→ASE-state translation.

## BAP LTV parser oracle — Bumble-RS

File: `bumble-profiles/src/bap.rs`

- `CodecSpecificCapabilities::from_bytes()`
- `CodecSpecificConfiguration::from_bytes()`
- `parse_ltv()`

These should back parser/golden tests for LC3 capability/configuration planning.

## BlueZ cross-check — semantic BAP state vs I/O object

Project: BlueZ

File: `profiles/audio/bap.c`

### `bap_state()`

Reference for reacting to BAP state changes and initiating the appropriate I/O/QoS action.

Important lesson:

- BAP semantic state is distinct from the ISO socket/fd object.

### `setup_create_io()`

Reference for choosing/creating stream I/O only after semantic stream type/QoS is known.

### `iso_connect_cb()`

On socket connection, binds fd into BAP stream via `bt_bap_stream_set_io`.

Use this as a boundary precedent, not as a socket implementation to copy.

Notable source comment:

- STREAMING state and ISO connect callback order can race; code explicitly tolerates the nondeterminism.

BE200AudioHub should model equivalent “semantic ready vs lower-path ready” conditions explicitly.

## Deterministic test obligations

### PACS / codec planning

- parse all LC3 capability LTV fields;
- unsupported codec rejected;
- incompatible sample rate/frame duration rejected;
- left/right plan remains group-consistent.

### ASCS / ASE

- valid state transition sequence;
- every invalid transition;
- operation response errors;
- notification reorder/duplicate;
- one member failure prevents false group-ready state.

### CIG/CIS plan

Given devices/ASE/QoS:

- stable CIS IDs;
- left/right binding deterministic;
- handles only installed after successful HCI callback;
- partial CIS failure yields explicit rollback/recovery action.

### Data Path

- setup request only after CIS established;
- path-ready only after callback success;
- remove path is idempotent;
- late callback from stale generation ignored.

### Teardown

Replay:

```text
stop semantic production
→ disable/release ASE as policy requires
→ remove ISO Data Path
→ disconnect CIS
→ remove CIG
→ invalidate bindings
```

Exact implementation order may vary by controller/profile requirement, but every transition must be explicit and testable.

## HIL boundary

HIL validates Samsung-specific PACS/ASCS/CSIS values, notification quirks and controller command behavior. The semantic FSM itself should be synthetic-test complete before hardware use.
