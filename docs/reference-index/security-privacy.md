# Security / Bond / Privacy Reference Index

## BE200AudioHub contract

Security is split conceptually:

- HCI Host owns controller/encryption/SMP transport state;
- Control owns semantic policy: which peer/set is trusted and when to pair/reuse;
- persistence is cross-cutting and only written through explicit commit boundaries.

The product path should prefer stored security material and avoid unnecessary re-pairing.

## SMP manager primary oracle — Bumble-RS

Project: Bumble-RS

File: `bumble-smp/src/manager.rs`

### `PairingManager::register_connection()`

Reference for associating SMP state with an LE connection.

### `PairingManager::pair()`

Reference for initiating pairing as an explicit operation.

### `PairingManager::receive()`

Reference for inbound SMP PDU dispatch.

### `PairingManager::mark_encrypted()`

Reference for feeding controller encryption state back into SMP lifecycle.

### `PairingManager::poll_outbound()`

Reference for pull-based outbound SMP PDU production rather than direct lower-layer side effects.

### `PairingManager::poll_security_request()`

Reference for peer security-request notification.

### Key accessors

- `pairing_keys()`
- `encryption_key()`
- `store_bond()`

Use as separation precedent between provisional protocol state and durable bond persistence.

### `PairingManager::disconnect()`

Reference for clearing per-connection SMP state.

## SMP session FSM oracle

Project: Bumble-RS

File: `bumble-smp/src/session.rs`

### `LegacyPairingSession::poll_outbound()`

Reference for deterministic state-machine-driven outbound packets.

### State/key accessors

- `state()`
- `failure()`
- `pairing_keys()`
- `mark_encrypted()`

### Transition handlers

Inspect:

- `on_pairing_request`
- `on_pairing_response`
- `on_pairing_confirm`
- `on_pairing_random`

Use for event-sequence replay tests.

Do not couple pairing progress to thread sleeps or USB timing.

## Privacy primary oracle

Project: Bumble-RS

File: `bumble-smp/src/privacy.rs`

### `AddressResolver::can_resolve_to()`

Reference for deciding whether an address class is resolvable.

### `AddressResolver::resolve()`

Reference for resolving an RPA against stored IRKs.

### `resolvable_private_address()`

### `generate_resolvable_private_address()`

### `verify_resolvable_private_address()`

These are ideal golden-vector/property-test targets.

## Experiment/HIL references

Project: BE200AudioHub, immutable `deprecated/r5/crates/bumble-adapter/src/`

### `bond.rs`

Inspect when implementing persistent bond representation and coordinated-set summaries.

Relevant exported types:

- `BondStore`
- `BondMemberSummary`
- `BondSetSummary`
- `ControllerIdentity`

### `commit.rs`

Explicit persistence boundary.

Exported functions:

- `commit_primary_bond`
- `commit_verified_set_member`

Architectural precedent:

- provisional pairing/security state is not durable until semantic verification succeeds.

### `privacy.rs`

Exported `BondedPeerIdentity`.

Use for previously verified BE200 FAL/privacy assumptions.

### `stored_security.rs`

Use as HIL evidence for stored-LTK encryption recovery without starting new SMP.

### `security.rs`

Exported `LeBondManager`, `ProvisionalBond`.

Use for the experiment’s successful bond flow, not its orchestration/threading.

### `csis.rs`

Function `rsi_matches_sirk()`.

Useful deterministic oracle for RSI/SIRK matching.

### `set_member_verifier.rs`

Exported `SetMemberVerification`.

Use as evidence that second-ear membership was verified before persistence.

## Security boundary tests

### Privacy

- IRK resolves matching RPA;
- nonmatching IRK rejects;
- address types that cannot be RPA are rejected early;
- generation of RPA roundtrips through resolver;
- multiple IRKs return correct identity.

### SMP

- full successful event sequence;
- repeated/unexpected PDU;
- confirm/random mismatch;
- disconnect mid-pair;
- controller encryption success/failure;
- no persistent store write before commit condition.

### Stored security

Replay:

```text
resolve peer identity
→ establish ACL
→ request stored-key encryption
→ Encryption Change success
→ encrypted ATT permitted
```

Also test failure paths:

- missing LTK;
- wrong key/encryption failure;
- stale connection;
- peer identity mismatch.

### Coordinated-set persistence

Property:

> a peer discovered through RSI/address resolution does not become a committed coordinated-set member until CSIS verification proves it.

## HIL boundary

HIL validates:

- Samsung key distribution quirks;
- controller FAL/privacy behavior;
- stored-LTK encryption against real earbuds;
- actual CSIS/RSI data.

Crypto/parser/state-machine behavior remains Cargo-test constrained.
