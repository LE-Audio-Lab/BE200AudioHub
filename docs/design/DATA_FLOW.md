# Data Flow

## Control / state flow

```text
User Intent
    |
    v
L0 LifeCycle / Orchestration
    |
    v
L1 Control Plane ---- single writer ----> RuntimeState
      ^       ^       ^
      |       |       |
   reports  reports  reports
    Media   HCI Host Transport
```

RuntimeState is an observation/world model. It is not a shared mutable bag of subsystem internals.

## Media / ISO hot path

```text
Windows Audio Clock
        |
        v
WASAPI source
        |
        v
PCM clock-domain buffer
        |
        v
Clock Discipline / ASRC <------ StreamClock
        |
        v
480 target-domain samples / interval
        |
        v
Channel map -> LC3 L/R
        |
        v
EncodedStereoFrame(FrameId)
        |
        v
IsoTxBridge <------------------- StreamClock
 interval group / bounded backlog
 stale drop / fallback SDU
        |
        v
HCI Host
 per-CIS queues
 transport sequence
 fragmentation / HCI ISO framing
 controller credits
        |
        v
L2 Transport
 async packet movement / reassembly
        |
        v
L3 Platform / BE200 backend
        |
        v
BE200 controller -> CIS -> earbuds
```

## Frozen invariants

1. Media time never waits for HCI credits.
2. Number Of Completed Packets releases controller capacity only.
3. Media produces encoded, transport-agnostic frame groups.
4. HCI Host owns transport sequence, fragmentation, framing and controller credits.
5. IsoTxBridge owns interval-group submission policy; it does not understand LC3 payload semantics.
6. One StreamClock grid is authoritative for both Media and IsoTxBridge.
7. Backlog is bounded; stale audio is not allowed to accumulate latency indefinitely.
8. Transport never synchronously round-robin polls mutually independent RX endpoints.
9. RuntimeState is shared-readable and single-writer through Control.
10. Thread/executor/channel choices are not part of this architecture commit.
