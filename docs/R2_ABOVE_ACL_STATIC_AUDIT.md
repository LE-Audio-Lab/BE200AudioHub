# R2 above-ACL static audit and route consolidation

Date: 2026-10-13

## Scope

This audit covers code executed after an LE ACL can be established:

- LE connection lifecycle ownership;
- SMP Secure Connections;
- provisional and stored peer security;
- encrypted ATT/GATT and CSIS;
- peer-key/set-metadata persistence;
- onboarding and read-only verification orchestration;
- CLI entry points that expose those paths.

Firmware boot, USB framing, HCI initialization, scanning, ISO, BAP, and audio routing are outside this audit except where they define an input boundary.

## Findings

### 1. Multiple successful and experimental routes coexisted

The active tree contained:

- legacy `bond-onboard` and `bond-verify` orchestration in a 60 KB diagnostics file;
- an isolated Filter Accept List probe;
- `pairing-v2` onboarding and verification;
- four Bond Manager generations;
- explicit-address and local-RPA connection experiments;
- an unused transport-agnostic pairing state-machine crate.

Only one route had complete HIL proof:

```text
scan/discover
-> Filter Accept List ACL
-> provisional SMP
-> encrypted CSIS gate
-> atomic commit
-> stored-IRK/LTK reconnect verification
```

All other routes increased policy surface without adding a required product capability.

### 2. Layer ownership was blurred

The legacy code mixed:

- discovery policy;
- ACL initiating policy;
- SMP execution;
- identity correction;
- CSIS trust;
- persistent writes.

The refactored route assigns one owner to each transition.

### 3. Persistent writes had duplicate implementations

Primary and later-member commit code duplicated key snapshots, writes, metadata updates, and rollback. They are now one transaction implementation with two precondition gates.

### 4. ACL support code retained obsolete initiating paths

The old `connect.rs` included explicit-peer and local-RPA initiation that were disproven by HIL for the second Buds3 Pro member. Active ACL support now contains only session lifetime (`linger`, `disconnect`) while `LeConnectionManager` exclusively owns connection creation.

### 5. Stored-LTK recovery was buried in legacy pairing code

Read-only encryption restoration is now isolated from SMP. The verification route cannot accidentally start a Pairing Request or persist new keys.

### 6. Versioned source filenames encoded experiment history into the product graph

`bond_manager`, `bond_manager_v2`, `bond_manager_v3`, and `bond_manager_v4` are historical iterations, not separate product concepts. Only the HIL-proven implementation remains active under the stable `security` module name.

## Canonical active route

### Onboarding

```text
pair onboard
  -> discover primary Swift Pair transport
  -> LeConnectionManager::direct_connect_filter_accept_list
  -> LeBondManager::pair_provisional
  -> encrypted CSIS read
  -> commit_primary_bond
  -> discover RSI-matching remaining member
  -> LeConnectionManager::direct_connect_filter_accept_list
  -> LeBondManager::pair_provisional
  -> SetMemberVerifier
  -> commit_verified_set_member
  -> complete when CSIS Set Size is reached
```

### Verification

```text
pair verify
  -> resolve current address from stored IRK
  -> LeConnectionManager::direct_connect_filter_accept_list
  -> resume_bonded_encryption
  -> encrypted CSIS read
  -> exact SIRK / Set Size / Rank comparison
  -> disconnect
  -> assert persistent files unchanged
```

## Active module boundaries

### Binary crate

```text
src/r2_attach.rs        attach/scan/forensics only
src/pairing/onboard.rs  write-capable onboarding entry
src/pairing/verify.rs   read-only stored-bond verification entry
src/pairing/mod.rs      public pairing command surface
src/main.rs             CLI routing only
```

### Bumble adapter

```text
connection_manager.rs   sole ACL creation owner
acl_session.rs          established-ACL lifetime and disconnect
security.rs             provisional SMP transaction
stored_security.rs      stored-LTK encryption restoration; no SMP
csis.rs                 CSIS read/decrypt and RSI primitives
set_member_verifier.rs  authoritative member gate
commit.rs               sole peer-key/set-metadata write boundary
bond.rs                 persistent store and identity resolution
scan.rs                 discovery observations
init.rs / usb_fixed.rs  controller/transport substrate
```

## Public entry points

The supported above-ACL CLI surface is intentionally three commands:

```text
r2 pair reset --confirm
r2 pair onboard --seconds <1..30> --name <DEVICE_NAME>
r2 pair verify --seconds <1..30> --name <DEVICE_NAME>
```

Experimental `bond-*`, `pairing-v2-*`, direct-connect probes, and second-member probes are removed from the active CLI.

## Trust and mutation boundaries

1. RSI selects a candidate but does not establish trust.
2. SMP output remains provisional.
3. CSIS SIRK/Set Size validation is the trust gate.
4. `commit.rs` is the only onboarding persistence boundary.
5. Stored verification never invokes SMP and fingerprints persistent files before/after.
6. `LeConnectionManager` is the only ACL creation path above HCI.
7. Existing committed bonds are not automatically deleted or re-paired.

## Deprecated material

Historical source remains recoverable through Git history and the blob manifest in `deprecated/r2-pairing-experiments/`. It is intentionally absent from the workspace and active module graph.
