# HCI Host Reference Index

## BE200AudioHub contract

Target responsibility:

- one HciContext owns all mutable controller-facing Host state;
- Transport delivers complete native HCI packets;
- HCI Host owns H4 adaptation, command transactions, ACL/ISO Host queues, transport sequence, controller credits and connection primitives;
- Control never owns raw HCI state;
- controller completions release transport capacity only and never advance Media time.

Expected target area:

- `src/hci_host/`
- future typed HCI command/report API.

## Primary oracle — Bumble-RS transport-neutral Device

Project: Bumble-RS

File: `bumble-host/src/lib.rs`

### `trait HostTransport`

Functions:

- `handle_command`
- `send_acl_packet`
- `send_synchronous_data`
- `send_iso_packet`
- `drain_host_events`

This is the preferred adapter boundary.

Adopt:

- Bumble Device depends on a narrow transport interface;
- external adapter owns packet delivery;
- deterministic fake transports can drive the same Device state machine.

BE200AudioHub mapping:

`QueueHostTransport`-style adapter should translate between:

- L2 native HCI packet classes;
- Bumble H4 packet representation;
- bounded Transport TX.

### `Device::poll()`

Read for:

- serialized consumption of HostTransport events;
- Device state update performed by the caller’s execution context.

This is the key reason to use Bumble `Device` rather than `ExternalHost`.

Adopt:

- HciHostThread drives `Device::poll()`;
- no hidden reader thread owns Device state.

### Device command/capability completion chain

When implementing resetless operational attach, inspect the command-complete branches in `Device` for:

- Read Local Supported Commands;
- Local Version;
- LE Local Supported Features;
- LE Buffer Size;
- ISO Buffer Size;
- advertising capabilities;
- event mask setup.

Adopt:

- reuse Bumble’s event-driven completion chain where it does not require the forbidden ExternalHost orchestration;
- expose completed information as transport-neutral snapshots to Control.

## Negative reference — Bumble `ExternalHost`

Project: Bumble-RS

File: `bumble-transport/src/host.rs`

### `ExternalHost::new()`

Observed behavior:

- creates its own blocking reader thread.

Do not use as HciContext owner.

### `ExternalHost::send_command()`

Observed behavior:

- waits synchronously with `recv_timeout` for command completion/status.

Do not copy.

Reason:

- violates no-synchronous-cross-context-wait contract;
- introduces ownership outside HciHostThread;
- makes timeout policy live in the wrong layer.

Use the underlying Device/HCI types instead.

## Controller queue/fairness cross-check — Zephyr

Project: Zephyr

File: `subsys/bluetooth/host/conn.c`

### `bt_conn_data_ready()`

Marks a connection ready and kicks TX processing.

Reference for:

- producers signal readiness instead of directly monopolizing controller send path.

### `get_conn_ready()`

Read for:

- iterating ready connections;
- respecting available controller packet/TX contexts;
- rotating a connection back when it still has data.

Adopt concept:

- bounded fairness between controller-facing flows.

### `send_buf()`

Read for:

- acquire controller packet capacity before driver send;
- create/publish pending TX context before the actual send;
- restore capacity on lower-layer send failure.

This is a strong ordering invariant for Host queue accounting.

## Packet completion cross-check — Zephyr

File: `subsys/bluetooth/host/hci_core.c`

### `hci_num_completed_packets()`

Read for:

- per-handle completion;
- pending context removal;
- semaphore/capacity release;
- deferred upper-layer completion notification.

Core invariant:

> completion releases controller transport capacity. It is not a media tick.

## HCI packet boundary

Bumble `HciPacket::from_bytes()` expects H4 packet type.

BE200 USB L2 packets are native endpoint payloads without H4 type.

Therefore exactly one HCI Host adapter boundary owns:

- native Event → prepend Event H4 type;
- native ACL → prepend ACL H4 type;
- native ISO → prepend ISO H4 type;
- Bumble outbound Command/ACL/ISO → strip H4 before Transport.

Do not move H4 knowledge into L2.

## Command transaction model

Preferred shape:

```text
typed HCI request
→ send Command
→ return to event loop
→ Command Status/Complete observed later
→ correlate opcode/request
→ transition transaction state
→ typed report/reply
```

No `sleep`, no `recv_timeout` from inside the owning HciContext.

Timeout is a requester-owned timer event.

## Deterministic test obligations

1. native Event/ACL/ISO ↔ H4 conversion;
2. malformed H4/native packet rejection;
3. command complete correlation;
4. command status correlation;
5. duplicate/unexpected completion handling;
6. bounded Transport TX full => HCI fault/backpressure signal;
7. Device state is mutated from exactly one HciContext;
8. command transaction does not synchronously wait;
9. controller Event load does not starve stop/reset/control lane;
10. outbound data load does not starve Event service;
11. controller completion releases exactly the accounted capacity;
12. lower transport send failure restores capacity exactly once.

## HIL boundary

Cargo tests can constrain Host state/queue behavior. HIL is needed only to confirm:

- exact BE200 supported command/capability responses;
- Intel firmware quirks;
- actual command completion ordering;
- controller buffer counts and timing.
