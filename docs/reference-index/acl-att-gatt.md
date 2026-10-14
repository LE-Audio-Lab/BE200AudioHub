# ACL / L2CAP / ATT / GATT Reference Index

## BE200AudioHub contract

This domain belongs inside HCI Host substrate, below Control semantic LE Audio logic.

Desired boundary:

```text
Control typed request
→ HCI Host connection / ATT / GATT substrate
→ ACL packets
→ Transport
```

Control may request semantic operations, but it does not parse raw ACL/L2CAP/ATT bytes.

## ATT primary oracle — Bumble-RS

Project: Bumble-RS

File: `bumble-att/src/lib.rs`

### `AttPdu::from_bytes()`

Primary parser oracle for ATT PDU fixtures.

Test with:

- valid fixed and variable-length PDUs;
- truncated header;
- invalid opcode;
- invalid attribute payload lengths.

### `parse_handles()`

Reference for handle-list decoding.

### `parse_length_value_tuples()`

Reference for ATT response entries encoded as fixed-length tuples.

Use these helpers as parser/golden-test precedent rather than writing ad-hoc byte offsets in Control.

## GATT client primary oracle

Project: Bumble-RS

File: `bumble-gatt/src/client.rs`

### `GattClient::exchange_mtu()`

Reference for ATT transaction setup and response-driven MTU update.

### `GattClient::discover_services()`

Reference for primary service discovery iteration/pagination.

### `GattClient::discover_service_by_uuid()`

Reference for targeted service lookup.

### `GattClient::discover_characteristics()`

Reference for characteristic discovery within service handle range.

### `GattClient::discover_descriptors()`

Reference for descriptor discovery.

### `GattClient::discover_attributes()`

Reference for generic attribute iteration.

### `GattClient::on_notification()`

Reference for unsolicited Handle Value Notification routing.

### `GattClient::on_indication()`

Reference for indication routing/confirmation semantics.

### Parser helpers

- `parse_group_entries()`
- `parse_type_entries()`
- `parse_information_entries()`

Use as deterministic parser oracles for service/characteristic/descriptor fixtures.

## GATT server cross-check

File: `bumble-gatt/src/lib.rs`

### `GattServer::on_request()` / `on_request_with_context()`

Reference for request dispatch separated from attribute access policy.

### `GattServer::check_read_access()`

### `GattServer::check_write_access()`

Useful for understanding ATT error/security boundaries when interpreting Buds responses; BE200AudioHub is primarily a client.

## L2CAP primary oracle

Project: Bumble-RS

File: `bumble-l2cap/src/lib.rs`

### `L2capPdu::from_bytes()`

Primary framing parser oracle.

### `ControlFrame::from_bytes()`

Signaling-channel parser reference.

Tests:

- length/header correctness;
- CID routing;
- truncated signaling commands;
- unsupported/unknown command behavior.

## LE Credit-Based L2CAP

File: `bumble-l2cap/src/le_credit.rs`

### `LeCreditBasedChannel::receive_pdu()`

Reference for inbound SDU reassembly and credit consumption.

### `LeCreditBasedChannel::poll_outbound_pdu()`

Reference for outbound segmented PDU production.

### `LeCreditBasedChannel::poll_credit_grant()`

Reference for replenishment policy boundary.

### `LeCreditBasedChannel::disconnect()`

Reference for channel local-state teardown.

### `LeCreditChannelManager::connect()`

Reference for asynchronous channel establishment.

### `LeCreditChannelManager::process_pdu()`

Reference for signaling response/event-driven transitions.

### `LeCreditChannelManager::send()`

Reference for bounded data submission into channel state.

Even if initial B3P path does not need LE CoC for every service, keep the model indexed because future LE Audio profiles/controllers may.

## Controller ACL scheduling cross-check — Zephyr

Project: Zephyr

File: `subsys/bluetooth/host/conn.c`

### `send_buf()`

Reference for lower-driver send plus pending TX accounting.

### `get_conn_ready()`

Reference for fairness among active connections.

File: `subsys/bluetooth/host/hci_core.c`

### `hci_num_completed_packets()`

Reference for releasing ACL controller packet capacity per connection handle.

## Experiment/HIL evidence

Project: BE200AudioHub

Immutable path:

- `deprecated/r5/crates/bumble-adapter/src/acl_session.rs`
- `deprecated/r5/crates/bumble-adapter/src/gatt_session.rs`
- `deprecated/r5/crates/bumble-adapter/src/connection_manager.rs`

Use these files for:

- BE200/Buds-specific event order;
- previously proven LE ACL establishment;
- encrypted ATT/GATT access quirks.

Do not move their old blocking orchestration back into Review.

## Deterministic test obligations

### ACL

- connection-handle routing;
- controller packet-credit accounting;
- completion cannot underflow/over-release;
- disconnect invalidates per-handle pending state;
- stale connection events ignored after generation/session change.

### L2CAP

- complete header/parser fixtures;
- CID routing;
- fragmented/coalesced ACL packet input;
- signaling request/response transitions;
- channel disconnect ordering.

### ATT

- request/response opcode correlation;
- ATT Error Response mapping;
- MTU negotiation;
- malformed/truncated PDU rejection;
- unsolicited notification/indication independent of active request.

### GATT

Use recorded/synthetic attribute database fixtures to test:

- services;
- characteristics;
- descriptors;
- pagination;
- duplicate UUIDs;
- missing expected service;
- notification routing.

HIL is for validating the Buds3 Pro attribute database and timing, not ordinary parser logic.
