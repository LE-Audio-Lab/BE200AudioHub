# Control Plane Boundary

This document freezes the product-facing Control Plane boundary. It does not
define Core/Lifecycle policy or concrete mailbox capacities.

## 1. Ownership

Control owns Bluetooth / LE Audio semantics and the authoritative projection of
those semantics into the shared-readable RuntimeState world model.

Control does not own:

- raw USB transfers or endpoints;
- HCI packet framing;
- raw ACL/CIS handles outside HCI Host;
- controller ISO sequence numbers or credits;
- media timing, PCM storage, LC3 encoder internals or ASRC;
- L0 desired-state / retry / recovery policy.

RuntimeState is an observation model. It must never become a container for
mutable subsystem implementation objects.

## 2. Upper boundary: Core/L0 -> Control

The stable input type is `ControlIntent`.

Current intents deliberately cover only semantics already implemented by the
ControlMachine:

- `Start`
- `BeginStreamHandoff { session_id, stream_id }`
- `DiscoveryWindowElapsed`
- `TimerExpired(ControlTimer)`

Lifecycle stop/recovery policy is intentionally not guessed into this boundary.
L0/Core will own that contract when full lifecycle composition is implemented.

Core/L0 interacts with `ControlPlane`, not with fine-grained
`ControlMachine::start/on_event/on_hci_report/...` methods.

## 3. Lower semantic boundary: HCI Host -> Control

The stable lower semantic input is `ControlLowerReport`.

Currently:

```text
ControlLowerReport::Hci(HciReport)
```

Only typed semantic reports cross this boundary. Raw controller packets,
transport callbacks and mutable HCI objects do not.

The opposite direction remains `ControlAction`. The existing thin dispatcher
maps each action into either:

- one reliable typed `HciControl` submission; or
- one Core timer directive.

Control itself performs no blocking wait or HCI/USB I/O.

## 4. Uniform transition result

Every accepted upper intent or lower semantic report produces one
`ControlStep`:

```text
ControlStep
├── actions: Vec<ControlAction>
├── snapshot: ControlSnapshot
└── stream_binding: Option<StreamBinding>
```

`ControlSnapshot` is a coarse public observation. Fine-grained
`ControlPhase` remains an implementation detail of the semantic FSM.

`StreamBinding` appears only when Control has reached the existing
route-acknowledged handoff gate. It remains transport-neutral.

## 5. Runtime observation path

Runtime observation is intentionally separate from semantic Control input.

Plane-local mutable state is projected as typed snapshots:

```text
LifecycleSnapshot
ControlSnapshot
MediaSnapshot
HciHostSnapshot
TransportSnapshot
```

These enter the reducer as `RuntimeObservation`.

This distinction prevents Media/Transport health telemetry from accidentally
becoming LE Audio ControlMachine events.

## 6. RuntimeState aggregation

`RuntimeStateReducer` is the single writer of `RuntimeState`.

Rules:

1. Each plane owns its own mutable implementation state.
2. No plane receives `&mut RuntimeState`.
3. State-changing cross-plane traffic is typed and reliable.
4. High-frequency counters are aggregated inside the producer plane.
5. High-frequency observations are published as coalesced snapshots rather
   than one message per packet/frame.
6. Repeated identical observations do not mutate RuntimeState and do not
   advance its observation epoch.
7. A changed observation updates only its own projection.
8. Lifecycle generation is mirrored into the top-level RuntimeState generation.
9. Readers receive cloned/read-only value snapshots; they never receive mutable
   access to the reducer-owned world model.

The top-level `RuntimeState::epoch` is an observation-model revision counter.
It is distinct from `Generation`:

- `Generation` fences runtime/session work;
- `epoch` tells observers that the published world model changed.

## 7. Reliable events vs coalesced telemetry

Do not collapse these delivery classes.

### Reliable state-changing traffic

Examples:

- connection/security state transition;
- ASE/CIG/CIS/path transition;
- route bind/unbind acknowledgement;
- fault;
- lifecycle transition.

Semantics:

- bounded;
- producer non-blocking;
- mailbox-full is a fault;
- never silently overwrite or drop.

### Coalesced telemetry / health snapshots

Examples:

- packet counters;
- ISO submitted/completed/in-flight;
- queue depth;
- media ring fill;
- encode/deadline metrics.

Semantics:

- aggregate locally;
- publish latest/coalesced snapshots;
- do not emit one Core message per packet/frame;
- telemetry must not drive media time or protocol semantics.

## 8. Intended Core composition

```text
User/L0 intents ────────────────┐
Core timer expiry ──────────────┤
HCI semantic reports ───────────┤
                                ▼
                         ControlPlane
                                │
                      ControlStep/actions
                                │
                   ┌────────────┴────────────┐
                   ▼                         ▼
               HciControl               Core timer
                   
Lifecycle/Control/Media/HCI/Transport observations
                                │
                                ▼
                      RuntimeStateReducer
                                │
                                ▼
                    RuntimeState snapshot
                                │
                         CLI / egui / logs
```

No UI or telemetry consumer may bypass the reducer and mutate subsystem state.

## 9. Current non-goals

This boundary does not yet implement:

- full L0 startup/shutdown composition;
- Core timer service;
- recovery/backoff policy;
- MediaRt owner;
- WASAPI Capture;
- ASRC;
- UI subscription mechanics.

Those pieces must consume this boundary rather than expanding it with raw
subsystem handles.
