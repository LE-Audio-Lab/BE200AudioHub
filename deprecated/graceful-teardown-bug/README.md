# Graceful Teardown Bug — Deprecated Snapshot

This directory preserves the complete tracked source tree immediately before
Round10 removed the Round9 graceful-teardown investigation scaffolding.

## Snapshot

- Branch: `refactor/v2-architecture`
- Pre-cleanup source commit: `941092e317561f3bc13c024600bd87efcc87671c`
- Archive commit: `b864cf195d75f6514fce70c70e5c0bc52bef23a6`
- Snapshot: `BE200AudioHub-graceful-teardown-bug-941092e31756.zip`
- Pre-investigation code baseline: `a11efdffadd0bf319d53ab09f362116d11945030`
- Round10 cleanup commits:
  - `ca12d763170a01218a99b2bf79bfde517981b83a`
  - `8176e635b0ab4d51debf8aa1e3c8d1d5ab5c4e4a`
- Status: deprecated reference only.

The ZIP was generated with `git archive HEAD` at the pre-cleanup source commit,
so it is the exact tracked repository state containing the full Round9
investigation implementation.

## Investigation scope

The archived tree contains the code added after the first long-session ASCS
Release timeout to determine why the peer-facing control path stopped responding.

The investigation falsified, among others:

- incorrect Release ordering / serial-vs-group Release;
- ASCS Control Point Write Request semantics;
- missing periodic ATT traffic;
- stale Controller-to-Host HCI flow-control state;
- Host ACL TX / NCP / USB OUT starvation;
- USB ACL-IN cancel or missed re-arm;
- a single-ear or single-handle failure;
- CIS/ISO as a necessary trigger;
- one outstanding USB ACL Bulk-IN read as the trigger.

The final unresolved boundary was a controller-global Host-facing HCI ACL RX
cessation: both LE ACL data bearers stopped producing HCI ACL data toward the
Host while HCI events, connection handles, RSSI, LL control and Host-to-Controller
ACL traffic remained operational.

## Deprecated code index

The following code families were removed from active source in Round10. Their
complete implementation remains recoverable from the snapshot and git history.

| Investigation component | Main commits | Round10 disposition |
| --- | --- | --- |
| Release gate snapshots / timeout-state diagnostics | `41742199`, `2672f75a`, `30a8589c` | Removed |
| Teardown ACL queue / RX-boundary / packet-lineage audits | `d1fb532c`, `084fa605`, `85947255`, `dd7c3e45`, `79750f68`, `104cd8fa` | Removed |
| Release-timeout ASE readback probe | `8b15f561` | Removed |
| Group Release A/B mode | `d85a1d73`, `b99bfbe0` | Removed |
| Controller-to-Host flow-control sanitize A/B | `17b4b969`, `342c1b6c`, `a5da8322` | Removed |
| Runtime ATT liveness / periodic keepalive probes | `70d3c5e2`, `6f8f769b` | Removed |
| USB ACL-IN lifecycle forensic telemetry | `a34c0ffe`, `139e7c87` | Removed |
| Controller RSSI and peer LL Remote Features probes | `f6d5273e`, `ebfaaaaa`, `5196ef0e`, `919dcb89` | Removed |
| Opposite-ear failure-scope probe | `4078f0b2` | Removed |
| ACL negotiated-runtime snapshot | `d8f6d8a6` | Removed |
| Final-model Control profiles and single-CIS/low-QoS branches | `e48257d4` through `bf73b1f2` | Removed |
| CIS runtime snapshot | `5752f387` | Removed |
| Final-model HIL runner, synthetic pressure, checkpoint and abort harness | `1db74aa4` through `3e27c024` | Removed |
| ACL Bulk-IN queue-depth generalization and depth-two A/B | `404e4738`, `941092e3` | Removed; active transport returned to depth 1 |
| Cross-layer `TransportTxAudit` / forensic ACL snapshot plumbing | Round9 diagnostic series above | Removed entirely |

## Active-code carryover after cleanup — architecture decision deferred

Round10 cleanup intentionally left only a very small code delta. The following describes the active implementation at the end of this conversation; it is **not** a finalized architecture decision. The next chat must explicitly decide the teardown contract before design docs are updated.\n\nTwo behaviors remain in active code:

1. ASCS Control Point operations use ATT Write Command / Write Without Response.
   Generic ASCS Release support remains available for failure rollback.
2. Product Stop is controller-clean and protocol-ungraceful by default. It does
   not depend on peer ASCS Release completion.

The current active-code teardown order is:

```text
route unbind
→ Remove ISO Data Path
→ Disconnect CIS
→ Remove CIG
→ Disconnect ACL
→ clear controller privacy/FAL
→ Control Idle
```

Failure rollback remains separate and may still perform best-effort peer ASCS
Release after path/CIS retirement.

## Post-cleanup code delta

After Round10 cleanup, comparing active `src/` against the pre-investigation
baseline `a11efdff` leaves only three intentional code differences:

- `src/plane/l1_control/ascs/operation.rs` — ASCS WNR semantics;
- `src/plane/l1_control/engine/machine/rollback_flow.rs` — default ungraceful product Stop;
- `src/plane/l1_control/engine/machine/tests.rs` — regressions locking those decisions.

All other Round9 investigation runtime code was deprecated rather than retained
as dormant product complexity.


## Handoff note

Round10's final architecture decision is intentionally **not** recorded here. Read
`docs/Timeline/Round10.md` and make that decision explicitly in the next conversation.
