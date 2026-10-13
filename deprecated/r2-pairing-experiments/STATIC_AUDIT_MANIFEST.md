# Above-ACL static-audit archive manifest

Date: 2026-10-13

The active graph was reduced to one HIL-proven route. Removed source remains recoverable from these immutable blobs and repository history.

| Removed active path | Blob SHA | Reason |
|---|---|---|
| `src/r2_attach.rs` legacy snapshot | `e20ce4891d6198ba304fca97cf14c63630e46345` | mixed diagnostics, explicit ACL experiments, legacy pairing and verification |
| `src/r2_pairing_refactor.rs` | `b493f0630923f76be72396019d512e83a1f323e4` | one-off FAL proof; result integrated into canonical connection manager |
| `crates/bumble-adapter/src/bond_manager.rs` | `22ee0c93bcbc49c992dc5ab1659ed531eb91b242` | superseded security transaction v1 |
| `crates/bumble-adapter/src/bond_manager_v2.rs` | `3912bb19d5695739d796940b06bdc4532a611641` | superseded encryption-order diagnostic generation |
| `crates/bumble-adapter/src/bond_manager_v3.rs` | `0ad076acedc3f30faeb416f9ecddba18d5f8c339` | superseded endpoint-order generation |
| `crates/bumble-adapter/src/bond_manager_v4.rs` | `71d612faa9d54e984ccb90d585e429d88a22e9c7` | HIL-proven implementation moved unchanged to stable `security.rs` |
| `crates/bumble-adapter/src/connect.rs` | `b42d295e4f03a7b35d66af62c2115c3b1d0c09b6` | explicit-peer/local-RPA experiments removed; session lifetime extracted |
| `crates/bumble-adapter/src/pairing.rs` | `215e49ffb768177a4883c6f91175716e6a68a321` | immediate-persist pairing removed; stored-LTK recovery extracted |
| `crates/bumble-adapter/src/bond_commit.rs` | `766b08e54cdc12bd66c9b6064e8a5817b6b71672` | merged into sole `commit.rs` write boundary |
| `crates/bumble-adapter/src/primary_commit.rs` | `a626cdccc80ae2d571c3cc84abdfa12259445690` | merged into sole `commit.rs` write boundary |
| `crates/pairing-orchestrator/src/machine.rs` | `4aba4f55f16002c2b2ae6b4e9cdfe2e00b69cd65` | unused parallel state machine; synchronous proven route is the product source of truth |
| `crates/pairing-orchestrator/src/model.rs` | `2d6b4eaf43a8e0a29044faf09535bda06b01f478` | unused model layer |

The former `r2_pairing_v2.rs` and `r2_pairing_verify.rs` blobs were not discarded; they were moved unchanged to `src/pairing/onboard.rs` and `src/pairing/verify.rs`.
