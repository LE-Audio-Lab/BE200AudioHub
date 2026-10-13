# Deprecated R2 pairing experiments

This directory records the pre-refactor R2.4 pairing control plane. It is not a design source for the replacement implementation.

The superseded orchestration was developed incrementally during Buds3 Pro HIL and mixed product policy with low-level Bluetooth mechanisms. Git history remains the canonical source snapshot for the exact code; this manifest names the active symbols that are being retired from the build graph during the pairing-layer refactor.

## Superseded orchestration

- `src/r2_attach.rs::bond_onboard`
- `src/r2_attach.rs::bond_verify`
- `src/r2_attach.rs::bond_reseed_host`
- `src/r2_attach.rs::connect_with_coordinator_privacy`
- second-member transport rules embedded directly in scan-result filtering
- Samsung Swift Pair selection embedded directly in the set-member loop

## Superseded adapter policy

- `crates/bumble-adapter/src/pairing.rs::pair_and_persist`
  - manually drives the SC pairing session from the product orchestration;
  - mixes provisional key creation, identity validation, persistent commit, and address-resolver refresh.
- `crates/bumble-adapter/src/connect.rs::connect_scanned_observation_with_local_rpa`
  - generates a local RPA as a CSIS-specific connection policy;
  - couples host privacy policy to coordinated-set discovery.
- `crates/bumble-adapter/src/bond.rs` local-identity reseed experiment
  - introduced to test a specific second-member ACL hypothesis;
  - must not define the new generic bond manager contract.

## Incorrect or unproven policy that must not migrate

1. A second set member must advertise with an RPA.
2. A public-address RSI match is identity-only and cannot be a bond transport.
3. A second Swift Pair beacon is required for the second member.
4. CSIS should choose the controller own-address/privacy mode.
5. A second-member ACL timeout proves that the coordinator local IRK is wrong.
6. Peer keys may be permanently committed before authoritative SIRK verification.

## Still-valid reusable work

The following are not deprecated:

- BE200 firmware/WinUSB bring-up;
- no-reset Bumble attach;
- product-local USB HCI Event/ACL framing;
- passive/forensic extended advertising scan;
- generic LE ACL primitives;
- SMP/ATT/GATT crypto and transport primitives;
- persistent peer-key storage format, subject to transaction refactoring;
- CSIS discovery, SIRK read/decrypt, and RSI cryptography;
- verbose HIL logging.

## Replacement

The replacement control plane lives in `crates/pairing-orchestrator` and is documented in `docs/R2_PAIRING_LAYER_REFACTOR.md`.

Cutover rule: legacy symbols stay callable only until `LeConnectionManager`, `LeBondManager`, and `SetMemberVerifier` have passed their individual HIL gates. They are then removed from the active build graph rather than patched further.
