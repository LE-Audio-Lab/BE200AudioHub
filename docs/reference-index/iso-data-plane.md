# ISO Data Plane Reference Index

## BE200AudioHub contract

The HCI Host owns:

- complete ISO SDU submission API;
- per-CIS HCI ISO sequence;
- HCI ISO header/PB flags;
- fragmentation;
- per-CIS queue/in-flight accounting;
- controller ISO packet capacity;
- Number Of Completed Packets accounting.

Media and IsoTxBridge submit opaque encoded SDUs. They never own HCI sequence numbers or controller credits.

Critical invariant:

> controller completion releases transport capacity only. It does not generate FrameId, advance StreamClock, or tell Media to encode the next frame.

## Primary oracle — Bumble-RS ISO Host path

Project: Bumble-RS

File: `bumble-host/src/lib.rs`

### `Device::send_iso_sdu()`

Read for the full Host-side SDU boundary.

Observed responsibilities:

- choose/advance ISO packet sequence per ISO handle;
- use controller ISO packet length;
- fragment a complete SDU;
- assign PB flags;
- enqueue or send fragments;
- invoke ISO queue flush.

Adopt:

- caller supplies one complete SDU;
- Host owns transport sequencing/fragmentation.

### `Device::flush_iso_queue()`

Read for:

- pull packets from bounded controller-credit-aware queue with `poll_ready()`;
- send through `HostTransport::send_iso_packet()`;
- restore capacity if lower transport refuses the send.

Use as primary queue/credit behavior together with `DataPacketQueue`.

### Number Of Completed Packets event branch

In the Device event handling section, inspect `NumberOfCompletedPackets` handling.

Observed behavior:

- release per-handle ACL/ISO capacity through queue `on_packets_completed`;
- then flush queues.

This is the main Bumble precedent for completion accounting.

## Queue accounting oracle — Bumble-RS

File: `bumble-host/src/data_queue.rs`

Type: `DataPacketQueue<T>`

### `DataPacketQueue::new(max_in_flight)`

Defines bounded controller capacity.

### `enqueue()`

Adds pending data without changing media time.

Review-2D caveat:

- the controller `max_in_flight` window is bounded;
- the pinned implementation's waiting `VecDeque` is not capacity-bounded;
- `Device::send_iso_sdu()` can therefore accept more fragments while controller credits are exhausted.

BE200AudioHub must not use that internal waiting queue as its realtime backlog. HCI performs complete interval-group admission before calling `send_iso_sdu()`. Normal groups may not move Host pending packets beyond the controller window. A legal group whose own fragmentation exceeds that window may be admitted only from a fully drained state, so at most one such logical group can occupy Host waiting state.

### `poll_ready()`

Only emits data while in-flight count is below configured maximum.

### `on_packets_completed()`

Per-handle completion accounting.

Important tests/reference behavior:

- unknown connection/handle is an error;
- completing more packets than outstanding is an error;
- capacity is returned exactly by completion count.

### `flush(handle)`

Reference for connection teardown cleanup.

## Production cross-check — Zephyr

Project: Zephyr

File: `subsys/bluetooth/host/iso.c`

### `bt_iso_chan_send()`

Complete SDU API:

- validates stream state/SDU size;
- writes ISO SDU sequence/length metadata;
- queues into connection ISO TX path.

### `bt_iso_chan_send_ts()`

Same concept with timestamp.

### `conn_iso_send()`

Queues SDU into `conn->iso.txq` and signals connection data-ready.

### `iso_data_pull()`

Fragments queued SDU while retaining it until the final fragment is sent.

Adopt:

- fragmentation lives below producer;
- queued SDU lifetime spans all fragments.

File: `subsys/bluetooth/host/conn.c`

### `send_iso()`

Maps fragment flags to HCI ISO PB flags, builds HCI ISO header and sends to lower transport.

### `send_buf()`

Acquires controller buffer semaphore/capacity before driver send and restores it on send failure.

File: `subsys/bluetooth/host/hci_core.c`

### `hci_num_completed_packets()`

Releases packet semaphore/capacity as completed packets arrive.

This is the strongest cross-check for Bumble queue accounting.

## Linux cross-check — user SDU / kernel transport sequence

Project: Linux Bluetooth, standalone source snapshot

File: `net/bluetooth/iso.c`

### `iso_sock_sendmsg()`

Observed path:

- userspace supplies one ISO SDU;
- kernel allocates skb/fragments based on HCI MTU;
- passes the SDU to `iso_send_frame()`.

### `iso_send_frame()`

Observed responsibilities:

- verifies SDU size against QoS;
- pushes HCI ISO SDU header;
- owns/increments `conn->tx_sn`;
- calls `hci_send_iso()`.

Use to reinforce the boundary:

> application/media owns SDU payload; Host/kernel owns transport sequence/header.

Limitation:

- this index has only standalone `iso.c`; do not infer the full Linux controller credit path from it.

## Fragmentation cross-check — BTstack

Project: BTstack

File: `src/hci.c`

### `hci_send_iso_packet_fragments()`

Reference for:

- fragmentation at controller maximum ISO packet length;
- PB flag assignment;
- timestamp only on first fragment.

### `hci_send_iso_packet_buffer()`

Useful secondary reference for packet construction.

Caveat:

- source contains a TODO around controller-space checking in this path; do not use it as sole production credit oracle.

### `hci_handle_number_of_completed_packets()`

Reference for decrementing per-connection/ISO outstanding packet counts.

## Negative reference — BTstack lite LE Audio demo

File: `example/le_audio_unicast_gateway_lite.c`

### `handle_iso_can_send_now_event()`

Demo behavior schedules the next LC3/media production from can-send readiness.

Do **not** copy this scheduling relationship.

Reason:

- it recreates the Experiment/R5 mistake where transport capacity becomes a source/media clock.

## SDU/fragmentation test matrix

For every ISO Host implementation test:

1. SDU fits one HCI packet;
2. exactly fills max packet size;
3. requires two fragments;
4. requires many fragments;
5. timestamp present/absent policy;
6. correct first/continuation/last/complete PB flags;
7. sequence increments once per SDU according to chosen HCI semantics;
8. two CIS handles maintain independent sequence/queue state;
9. send failure restores capacity;
10. completion on one handle cannot release another handle;
11. over-completion faults;
12. disconnect flushes only affected handle;
13. queue full never advances StreamClock;
14. a fast producer cannot move an unbounded realtime backlog into Bumble's waiting queue;
15. group admission is decided before any member is enqueued, so local backpressure cannot create a one-ear partial interval.

## Credit tests

Use a fake controller capacity:

```text
capacity=N
enqueue M>N packets
poll_ready => only N
complete k
poll_ready => exactly k additional
```

Fuzz/property test arbitrary enqueue/completion sequences with invariant:

`0 <= in_flight <= capacity`

and:

`submitted - completed = in_flight`

for each handle.

## HIL boundary

HIL validates:

- BE200 reported ISO packet length/count;
- actual Number Of Completed Packets event timing;
- Intel transport/controller quirks.

All queue math, sequence, fragmentation and ownership should already be Cargo-test constrained.
