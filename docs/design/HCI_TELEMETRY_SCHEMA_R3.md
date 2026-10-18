# Round 3 — HCI ISO / Interface / Telemetry Schema

Status: **Round 3 implementation handoff**

This note records the actual HCI ISO, interface, and producer-local telemetry
shape after the V2 Round 3 migration. It is intentionally narrower than the
future Core/Condition observability design.

## 1. Active L1-HCI layout

```text
plane/l1_hci/
├── acl/
├── gatt/
├── host/
│   ├── engine.rs
│   ├── link.rs
│   ├── stack.rs
│   └── worker.rs
├── interface/
│   ├── command.rs
│   ├── report.rs
│   ├── iso_tx.rs
│   └── mailbox.rs
├── iso/
│   ├── cig_cis.rs
│   ├── lifecycle.rs
│   ├── path.rs
│   ├── credit.rs
│   ├── tx.rs
│   ├── feature.rs
│   └── route.rs
└── telemetry/
    ├── accumulator.rs
    ├── observation.rs
    └── trace.rs
```

The old flat `src/hci_host/` ownership tree is gone. There is one physical
owner for each ISO/interface identity and API type.

## 2. Frozen ISO invariants

Round 3 preserves the following proven behavior:

- logical routes remain session/stream fenced;
- route bind/unbind keeps explicit ACK semantics;
- logical stream identity resolves to `CisRef` and only HCI resolves
  `CisRef` to raw CIS connection handles;
- controller ISO credit is global across CIS handles;
- group admission remains atomic;
- raw and logical ISO groups share one bounded data lane;
- drain blocks matching handles atomically;
- stale logical groups drop as a whole rather than becoming partial stereo;
- teardown may retire outstanding packets from local accounting but retirement
  is never controller completion evidence.

## 3. Authoritative credit state

`IsoCreditAudit` is protocol/accounting truth.

It owns:

```text
limit
total_in_flight
in_flight_per_handle
total_submitted
total_completed
submitted_per_handle
completed_per_handle
```

Mutations are strict:

- `submitted(handle)` fails when the controller pool is unavailable/full;
- `completed(handle, count)` faults on unknown or excessive completion;
- `flush(handle)` removes outstanding local accounting on teardown;
- `flush_all()` retires all outstanding accounting on global flush.

`flush` and `flush_all` do **not** increase completed counters.

The explicit diagnostic command:

```text
HciCommand::ReadIsoTxAudit
    ->
HciReport::IsoTxAudit(IsoCreditSnapshot)
```

returns this authoritative accounting snapshot. It is not the producer
telemetry stream.

## 4. Producer-local telemetry

`HciTelemetry` lives inside the HCI Engine and mirrors only successful
authoritative mutations.

Current aggregate schema:

```text
HciIsoTelemetrySnapshot {
    controller_limit
    submitted_packets
    completed_packets
    retired_packets
    in_flight_packets
    peak_in_flight_packets
    trace
}
```

Meaning:

- `controller_limit`: most recently observed controller ISO capacity;
- `submitted_packets`: packets accepted into controller accounting;
- `completed_packets`: packets explicitly released by Number Of Completed
  Packets;
- `retired_packets`: outstanding packets removed only because teardown/flush
  invalidated the local accounting state;
- `in_flight_packets`: latest mirrored authoritative in-flight gauge;
- `peak_in_flight_packets`: high-water mark for the HCI Engine lifetime;
- `trace`: bounded raw trace ring statistics.

Telemetry never decides whether a submit/completion/teardown is legal.
`IsoCreditAudit` mutates first; telemetry mirrors the resulting state only
after the mutation succeeds.

## 5. Mutation-site wiring

### Controller capacity configuration

```text
host initialization succeeds
    ->
IsoCreditAudit::configure(limit)
    ->
HciTelemetry::observe_iso_configured(limit, in_flight)
```

### Successful ISO submission

```text
QueueHostTransport journals IsoSubmitted(handle)
    ->
HciEngine validates live CIS
    ->
IsoCreditAudit::submitted(handle)
    ->
HciTelemetry::observe_iso_submitted(handle, in_flight)
```

A rejected transport enqueue does not produce `IsoSubmitted` and therefore
does not increment authoritative or telemetry submission counters.

### Number Of Completed Packets

```text
HCI event
    ->
QueueHostTransport::HciObservation::CompletedPackets
    ->
HciEngine::process_data_credit_observation
    ->
IsoCreditAudit::completed(handle, count)
    ->
HciTelemetry::observe_iso_completed(handle, count, in_flight)
```

Telemetry is updated only after the strict credit mutation succeeds.

### CIS/Host teardown

```text
DeviceEvent::Disconnected(CIS handle)
    ->
IsoCreditAudit::flush(handle)
    ->
HciTelemetry::observe_iso_retired(Some(handle), retired, in_flight)
```

Global Bumble `DeviceEvent::Flush` uses `flush_all()` and records a
`Retired { cis_handle: None, ... }` observation.

## 6. Raw trace schema

Raw diagnostics are local to HCI:

```text
HciIsoTraceRecord::Configured
HciIsoTraceRecord::Submitted
HciIsoTraceRecord::Completed
HciIsoTraceRecord::Retired
```

Default capacity:

```text
256 records
```

Implementation uses the generic runtime `TelemetryRing<T>` with the default
recent-history policy:

```text
fixed capacity
overwrite oldest
accept newest
count overwrite loss
never block
no default filesystem persistence
```

Raw trace records are diagnostic evidence only and are not protocol truth.

## 7. Dirty/coalescing contract

The accumulator has a local dirty bit.

Every meaningful aggregate mutation sets dirty.

`take_iso_telemetry_dirty()`:

- returns whether aggregate telemetry changed since the previous take;
- clears only the publication bookkeeping bit;
- does not reset counters or gauges.

Round 3 intentionally does **not** attach this dirty signal to Core Condition,
a reliable HCI report, or a new cross-thread telemetry mailbox.

Reason:

> producer-local aggregation is now proven, but the final bounded publication
> cadence and Plane ConditionUpdate boundary belong to the later Condition
> integration round.

This prevents high-rate telemetry from being accidentally promoted into the
reliable semantic lane.

## 8. Reliable semantic lane separation

Reliable HCI reports continue to carry state-changing facts such as:

- attach/capability state;
- privacy/connect/security state;
- CIG/CIS/path state;
- route ACK/state;
- drain state;
- GATT reports.

Producer telemetry is not added as a lossy variant of `HciReport`.

`IsoTxAudit` is the exception only in the sense that it is an explicit
request/reply diagnostic read of authoritative credit state. It is not a
periodic telemetry publication and is not driven by packet rate.

## 9. Number Of Completed Packets audit

The completion path was re-audited in Round 3.

Observed dependency chain:

```text
Number Of Completed Packets
    ->
HCI observation journal
    ->
ACL or ISO credit audit
    ->
producer-local telemetry mirror
```

No production dependency from this path reaches:

- `FrameId`;
- `LocalStreamClock`;
- `IntervalGroupScheduler`;
- Media source timing;
- LC3 frame production.

Controller completions release transport/controller capacity only.

They do not advance media time.

## 10. Raw CIS handle boundary audit

Production Control was checked before its `#[cfg(test)]` sections.

No production Control path reads:

- raw CIS `connection_handle`;
- `IsoTxAudit`;
- Number Of Completed Packets;
- ISO credit counters.

Control uses semantic CIS references and route/state reports.

Raw handle APIs still exist inside HCI and Review/HIL where controller evidence
requires them. Their existence is not treated as permission for product Control
or Media to depend on raw handles.

## 11. Regression evidence

Round 3 added producer-local telemetry tests covering:

- aggregate vs raw trace separation;
- fixed-capacity overwrite-oldest trace behavior;
- dirty coalescing without counter reset;
- submitted/completed/retired distinction;
- HCI Engine submit/completion integration;
- teardown retirement of an outstanding ISO packet without incrementing
  completion.

The teardown regression explicitly proves:

```text
pending ISO packet
+ CIS disconnect
=
retired +1
completed +0
in_flight -> 0
```

## 12. Deferred to Condition integration

Not implemented in Round 3:

- Core Condition publication;
- a periodic/coalesced cross-thread telemetry lane;
- Blackboard projection;
- UI/CLI telemetry snapshot API;
- fault-triggered Core publication.

When Condition integration is implemented, it should consume a coarse HCI
ConditionUpdate / telemetry summary at a bounded cadence. It must not consume
raw trace records or individual packet completions.

## 13. Round 3 conclusion

Round 3 leaves HCI with:

```text
authoritative local ISO accounting
        +
producer-local bounded raw history
        +
producer-local O(1) aggregate telemetry
        +
explicit reliable semantic/control interfaces
```

The data-plane invariant remains:

> Number Of Completed Packets releases controller capacity only; it never
> advances or disciplines media time.
