# R2 pairing-layer refactor

Status: Phase 1 — control-plane extraction

## Why this refactor exists

The previous `r2 bond-onboard` path combined five distinct responsibilities in one procedure:

1. Samsung/Swift Pair primary discovery.
2. CSIS set-member discovery from RSI.
3. LE address/privacy/connection policy.
4. SMP execution and peer-key persistence.
5. Authoritative set-membership verification by SIRK.

That architecture caused hardware observations to become ad-hoc policy. Examples include requiring a second member to use an RPA, treating public-address advertisements as non-connectable identities, requiring a second Swift Pair anchor, and rotating the local coordinator identity. Those rules are not owned by a CSIS coordinator in AOSP or Zephyr.

## Upstream control-plane model

The new layer follows the common behavior found in AOSP Fluoride and Zephyr:

```text
primary discovery
    -> one generic LE bond
    -> discover CSIS / obtain set identity and size
    -> scan for RSI-matching expected member
    -> stop discovery
    -> one generic LE bond for that exact scanner address
    -> pairing-time SIRK verification
       -> reject and discard provisional bond on mismatch
       -> atomically commit peer bond + set membership on match
    -> resume member search until set size is reached
```

CSIS discovers candidates. It does not own HCI initiating policy, address privacy, or SMP mechanics.

## New crate boundary

`crates/pairing-orchestrator` is transport agnostic. It contains no Bumble, HCI, SMP, GATT, Intel, Windows, or Samsung types.

It owns:

- pairing states and events;
- one-in-flight-bond serialization;
- expected-set-member lifecycle;
- provisional-vs-committed bond semantics;
- retry/recovery actions;
- completion based on CSIS set size.

It emits actions for lower layers to execute:

- start/stop discovery;
- begin a generic LE bond;
- discover primary CSIS;
- search remaining set members;
- verify the provisional member SIRK;
- commit or discard a provisional bond.

## Required lower-layer contracts

The next phases will implement three narrow adapters.

### `LeConnectionManager`

Input: scanner-supplied address and address type.

Responsibilities:

- device/address record normalization;
- controller resolving-list and filter-accept-list policy;
- direct connection lifecycle and cancellation;
- own-address/privacy policy;
- no CSIS-specific address guessing.

### `LeBondManager`

Input: one address selected by the orchestrator.

Responsibilities:

- exactly one active LE bond;
- ACL + fixed SMP channel lifecycle;
- SMP state and security callbacks;
- provisional peer keys until the transaction commits;
- no coordinated-set discovery logic.

### `SetMemberVerifier`

Input: provisional encrypted connection, expected set handle, and peer identity.

Responsibilities:

- discover/read CSIS;
- decrypt SIRK when required using the negotiated peer LTK;
- compare with the expected set;
- return verified rank/size metadata or a rejection;
- never commit peer keys directly.

## Invariants

1. At most one primary/member bond is active.
2. RSI match creates only an `ExpectedSetMember`; it is not final trust.
3. The scanner address is passed unchanged to the generic bond manager.
4. Public-vs-RPA policy belongs to the connection manager, not CSIS.
5. A second-member bond remains provisional until SIRK verification succeeds.
6. SIRK rejection discards provisional keys and resumes set-member discovery.
7. Existing committed members are never automatically forgotten or re-paired.
8. Discovery is stopped while a bond or verification transaction is active.
9. Set completion is derived from CSIS set size, not a hard-coded TWS count.

## Migration phases

### Phase 1 — complete

- introduce `pairing-orchestrator` pure state machine;
- encode serialization, verification gate, commit/discard, and recovery;
- document legacy experimental policy under `deprecated/`;
- retain the existing HIL path temporarily so the workspace remains testable.

### Phase 2

- add `LeConnectionManager` over the current Bumble/BE200 transport;
- first HIL: RSI-matching scanner address -> generic direct connection only;
- no SMP and no bond-store mutation.

### Phase 3

- add `LeBondManager` with staged peer keys;
- connect SMP completion to orchestrator events;
- keep the current production bond store read-only during initial HIL.

### Phase 4

- add pairing-time `SetMemberVerifier`;
- atomically commit staged peer keys and CSIS metadata only after SIRK match;
- switch `r2 bond-onboard` to the new path.

### Phase 5

- remove the legacy orchestration from the active build graph;
- keep only low-level, reusable primitives in `bumble-adapter`;
- move the final superseded implementation snapshot to `deprecated/r2-pairing-experiments/`.
