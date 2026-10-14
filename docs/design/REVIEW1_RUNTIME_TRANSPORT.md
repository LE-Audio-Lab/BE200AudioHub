# Review-1 — Runtime Kernel + Async Transport

Status: **sealed for Review implementation**

This milestone implements the first two Review blocks defined by the architecture plan:

1. Runtime Kernel / execution substrate.
2. L3/L2 asynchronous BE200 transport.

The goal was to remove the synchronous endpoint polling and mixed ownership model that caused transport servicing jitter during the Experiment/R5 audio path.

## 1. Runtime Kernel

Implemented under `src/runtime/`:

- `GenerationGate` — session generation fencing and stale-message rejection.
- `CancellationToken` — shared idempotent cancellation.
- bounded nonblocking mailbox primitives.
- serial execution context with single-owner mutable state.
- request/reply correlation tracker.
- bounded supervisor fault channel.
- panic-to-fault propagation.
- explicit context stop/wakeup behavior.

Frozen runtime properties:

- no unbounded production mailbox;
- producer-side `try_send` never waits;
- stale generations are rejected before state mutation;
- context handlers execute serially against owned state;
- cross-context request/reply is correlation based rather than synchronous blocking;
- shutdown can wake an idle context without requiring the last sender to disappear.

## 2. Transport packet boundary

Implemented under `src/transport/`:

- native HCI controller packet classes;
- Event / ACL / ISO packet reassembly;
- preservation of partial USB reads;
- multiple complete HCI packets in a single transfer;
- explicit impossible-length rejection;
- pluggable `ControllerPacketSink`;
- deterministic `TransportReactor`;
- bounded Host->Transport TX mailbox;
- dedicated UsbReactor worker shell.

The transport reactor treats HCI packet loss as a correctness failure:

- controller packet sink full -> transport fault;
- sink disconnected -> transport fault;
- malformed/reassembly failure -> transport fault;
- device detach -> explicit Detached state;
- backend transfer failure -> explicit Faulted state.

## 3. L3 backend contract

Implemented under `src/platform/`:

### TestBackend

A deterministic backend used for software-only validation of:

- independent pending RX endpoints;
- completion ordering;
- cancellation;
- detach;
- write classification.

### Windows BE200 backend

The Windows backend uses `rusb::ffi` / libusb asynchronous transfers.

Current BE200 mapping:

- HCI Command -> USB class control OUT;
- controller Event -> interrupt-IN;
- ACL RX -> bulk-IN;
- ACL TX and ISO TX -> bulk-OUT.

The backend discovers the Bluetooth HCI interface by USB class/subclass/protocol and validates the required interrupt-IN / bulk-IN / bulk-OUT layout.

## 4. Concurrency model

The Experiment/R5 synchronous read loop has been removed from the Review implementation.

At transport start:

```text
interrupt-IN Event read  ─┐
                          ├─ concurrently pending
bulk-IN ACL read         ─┘
```

Completion order is determined by actual backend arrival order, not by a host round-robin read schedule.

After a read completion:

```text
completion
-> endpoint-local packet reassembly
-> bounded controller packet sink
-> immediately re-arm that endpoint
```

Independent endpoints remain independent.

## 5. Transfer ownership

libusb transfer lifetime is backend-owned.

The callback:

- never frees the transfer;
- never frees callback state;
- only records terminal completion information.

The TransportContext later observes completion and performs reclamation or reuse. This removes the callback/cancel raw-pointer race present in the earlier intermediate implementation.

## 6. Allocation policy

Steady-state high-rate transfer objects are reused:

- one persistent reusable read-transfer slot per RX endpoint;
- bounded reusable write-transfer pools per transport class;
- write pools are capped at 8 idle transfers per class.

This prevents per-HCI-event / per-ISO-packet `libusb_transfer` allocation churn.

## 7. Fairness and shutdown

The UsbReactor worker:

- checks cancellation every loop turn;
- services a bounded Host TX budget per turn;
- then services backend completion;
- does not drain an arbitrarily deep TX backlog before checking controller completions;
- cancels backend I/O on exit.

Tests explicitly verify that cancellation still wins under sustained TX pressure.

## 8. Verified invariants

The software test suite covers, among others:

- FIFO bounded mailbox semantics;
- mailbox full/disconnect behavior;
- generation invalidation;
- context single-owner ordering;
- idle context stop wakeup;
- request/reply out-of-order completion;
- context panic -> supervisor fault;
- Event partial reads;
- ACL partial header/payload reads;
- multi-packet transfers;
- reassembly size rejection;
- concurrent Event + ACL pending reads;
- interleaved Event/ACL completion ordering;
- endpoint re-arm;
- HCI sink overflow faulting;
- transport class mapping;
- detach propagation;
- worker cancellation under TX pressure;
- callback ownership;
- timeout partial-byte preservation;
- duplicate callback protection;
- persistent RX slot policy;
- bounded write-pool policy.

## 9. Validation result

On Windows, current default Review build:

```text
cargo test --workspace
48 passed; 0 failed

cargo check --workspace
PASS

cargo build --workspace
PASS
```

Warnings are predominantly expected `dead_code` from architecture scaffolding that is not yet consumed by the product entry point.

## 10. Deferred from Review-1

The following are deliberately not part of this sealed milestone:

- HCI Host semantic ownership;
- controller capability/attach policy;
- ACL/GATT/security;
- CIG/CIS;
- ISO controller credit accounting;
- Media / StreamClock / IsoTxBridge;
- WASAPI / ASRC.

Early HCI Host work already present in the repository is gated behind the `hci-wip` Cargo feature so it does not redefine the Review-1 acceptance baseline.

## 11. Review-1 exit criteria

Review-1 is considered complete because:

- Runtime ownership/message-passing primitives exist and are tested;
- independent USB RX endpoints are modeled as concurrently pending async transfers;
- synchronous round-robin Event/ACL polling is absent from the active Review transport;
- packet reassembly and transport faults are explicit;
- cancellation/detach are explicit;
- transfer callback ownership is safe and deterministic;
- steady-state transfer objects are bounded/reused;
- Windows build, check, and default workspace tests pass.

Further changes to these components should be driven by HIL evidence, profiling, or a demonstrated contract violation rather than architectural churn.
