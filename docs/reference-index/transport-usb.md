# Async USB / Transport Reference Index

## BE200AudioHub contract

Target modules:

- `src/platform/backend.rs`
- `src/platform/libusb_async.rs`
- `src/platform/windows_be200.rs`
- `src/transport/packet.rs`
- `src/transport/reassembly.rs`
- `src/transport/reactor.rs`
- `src/transport/sink.rs`
- `src/transport/worker.rs`

Layer boundary:

- L3 owns USB/libusb device, endpoint and transfer mechanics.
- L2 owns native HCI packet classes, boundaries/reassembly, cancellation and transport health.
- neither L2 nor L3 owns Bluetooth semantic state.

Critical invariant:

> interrupt-IN Event and bulk-IN ACL receive operations remain independently pending. Never recreate the Experiment/R5 synchronous round-robin read loop.

## Primary oracle — libusb asynchronous transfer model

Project: libusb

File: `libusb/io.c`

### `libusb_alloc_transfer()`

Reference for heap-stable transfer objects.

### `libusb_fill_control_transfer()`

Reference for asynchronous HCI Command control transfers. The setup packet and payload occupy one transfer buffer.

### `libusb_fill_bulk_transfer()`

Reference for ACL/ISO bulk endpoint submission.

### `libusb_fill_interrupt_transfer()`

Reference for controller Event interrupt-IN submission.

### `libusb_submit_transfer()`

Core contract:

- submission is asynchronous;
- returns before completion;
- completion arrives through callback from the event-handling context.

Adopt:

- one submitted operation per independent RX endpoint;
- transfer state remains alive until terminal callback.

### `libusb_cancel_transfer()`

Core contract:

- cancellation is asynchronous;
- transfer is still active until cancellation completion callback.

Adopt:

- cancel does not mean free;
- owner reclaims only after callback has made terminal state observable.

### `libusb_free_transfer()`

Important rule:

- active transfer must not be freed.

This is the ownership oracle for BE200AudioHub callback/cancel races.

### event handling section / `libusb_handle_events*()`

Read the event-handling model around `libusb_handle_events`.

Adopt:

- a dedicated TransportContext may block in the libusb event reactor;
- callbacks execute on the event-handling thread;
- callback work must remain bounded.

### resubmission guidance

The async API documentation explicitly supports re-submitting completed transfers and calls out persistent transfer use for interrupt endpoints.

Direct precedent for BE200AudioHub persistent Event/ACL RX slots.

## Rust boundary — rusb

Project: rusb

### File: `src/context.rs`

#### `UsbContext::handle_events()`

Safe event-loop wrapper around libusb event handling.

Use for:

- reactor event pumping and cancellation completion.

### File: `src/device_handle.rs`

#### `DeviceHandle::as_raw()`

Intended escape hatch for advanced unsafe endpoint I/O.

Adopt:

- use rusb for ordinary device/interface lifetime;
- use `rusb::ffi` only where async transfers require raw libusb APIs.

#### `DeviceHandle::set_auto_detach_kernel_driver()`

Reference for backend attach behavior.

#### `DeviceHandle::claim_interface()`

Reference for interface ownership.

#### `DeviceHandle::set_alternate_setting()`

Reference for selected interface/alternate configuration.

Caveat:

- do not manipulate interface ownership behind rusb in a way that invalidates its bookkeeping.

## Hardware evidence — BE200AudioHub Experiment/R5

Project: this repository

File: `deprecated/r5/crates/bumble-adapter/src/usb_fixed.rs`

Anchors to inspect when changing the BE200 backend:

- `open_be200_split_transport()`
- the PacketSource `read_packet()` implementation
- command/control OUT path
- ACL/ISO bulk-OUT path
- USB descriptor/endpoint selection.

Use for:

- HIL-proven BE200 endpoint mapping and Intel/controller quirks.

Do **not** copy:

- synchronous Event/ACL polling;
- timeout-based endpoint arbitration.

The old loop is regression evidence for what Review transport removes.

## Current BE200 endpoint contract

Validated mapping carried from Experiment into Review:

| Traffic | USB operation |
|---|---|
| HCI Command | class control OUT |
| HCI Event | interrupt-IN |
| ACL RX | bulk-IN |
| ACL TX | bulk-OUT |
| ISO TX | bulk-OUT |

L2 sees native USB HCI packet bytes without H4 type.

## Reassembly contract

Reference source is the Bluetooth USB transport framing itself; implementation is deterministic.

Tests must cover:

- Event header split across transfers;
- Event payload split across transfers;
- ACL header split;
- ACL payload split;
- multiple complete packets in one completion;
- declared size exceeding configured maximum;
- endpoint-local buffers never cross-contaminate.

## Callback contract

The callback should do no Host semantics.

Allowed:

- read terminal libusb status;
- read actual length;
- publish/mark completion;
- return.

Forbidden:

- call Bumble Device;
- run ATT/GATT/BAP;
- wait for mailbox capacity;
- free an active transfer;
- change lifecycle policy.

## Transfer reuse contract

BE200AudioHub Review policy:

- persistent reusable RX slot per active receive endpoint;
- bounded reusable TX transfer pool;
- no unbounded pool growth.

Use libusb resubmission semantics as the upstream basis.

## Worker fairness contract

One UsbReactor turn must not indefinitely drain Host TX before servicing backend completions.

Deterministic tests should inject:

- sustained TX load;
- interleaved Event/ACL completions;
- cancellation during TX pressure;
- detach during pending reads;
- reliable sink overflow.

## Test obligations

A transport change is not complete until software tests cover:

1. independent Event + ACL pending reads;
2. completion arrival order independent of endpoint class;
3. immediate endpoint re-arm;
4. partial read preservation;
5. exact HCI packet boundary output;
6. full controller sink => fault;
7. disconnected controller sink => fault;
8. detach => explicit Detached;
9. transfer failure => explicit Faulted;
10. cancellation drains/reclaims active transfers;
11. callback duplicate terminal notification cannot double-reclaim;
12. persistent RX reuse;
13. bounded TX reuse;
14. TX pressure cannot starve cancellation.

HIL is only required afterwards to validate Windows/libusb/BE200 external behavior.
