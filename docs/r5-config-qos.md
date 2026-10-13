# R5.2-A — BE200 CIG allocation + dual ASCS Config QoS (HIL)

## Scope

This **intentionally mutates transient BE200 controller and Buds3 Pro ASE
state**. Only run on an existing healthy R2 bond and dual-ACL R3/R4.2/R5.1
setup. It does **not** start SMP, re-pair, touch persistent bonds, enable the
audio streams, create CIS, install ISO Data Paths or transmit ISO packets.

Implementation spans:
- `bumble-adapter/src/cig_control.rs`: pinned Bumble HCI command dispatch,
  strict typed allocation parsed from LE Set CIG Parameters Command Complete
  `ReturnParameters::Raw`, and LE Remove CIG with status/CIG-ID readback.
- `bap-client/src/codec_config.rs`: already verified R4.2
  `prepare_sink`, `configure_sink`, `perform_operation`, notification gates
  and `release_if_needed` reused for a persistent live Codec Configured phase.
- `bap-client/src/qos_plan.rs`: separate live planner, so a *released*
  R5.1 snapshot cannot falsely be treated as active Codec Configured ASEs.
- `src/audio_set.rs`: existing encrypted dual ACL / fingerprint lifecycle.
- `src/main.rs`: guarded CLI with a dedicated write-confirmation flag.

## Operation, in order

1. Bootstrap BE200 Operational, resolve both IRKs, create two encrypted ACLs.
2. Discover PACS/ASCS/CAS, select left/right mono Sink ASEs by Audio Location.
3. Check controller CIS capability and ISO buffer size; no CIG mutation yet.
4. Install CP/ASE CCCDs and validate Idle on **both** earbuds.
5. Issue Config Codec on each; require CP Success, Codec Configured notification
   and independent GATT Readback. Do not Release yet.
6. Build the live QoS plan (CIG 1; left CIS 1; right CIS 2). Send **HCI LE
   Set CIG Parameters** once. Check status, CIG ID, CIS count, exactly two
   distinct 12-bit connection handles, and their order-to-CIS-ID mapping.
   These CIS handles are **allocated, not established**.
7. Issue ASCS Config QoS on each; require CP Success, QoS Configured
   notification and independent readback. Compare every returned QoS field
   against the corresponding planned field.
8. In reverse order, Release all attempted ASEs and prove Idle.
9. Remove the unestablished CIG with HCI LE Remove CIG and verify returned
   CIG ID. Disconnect both ACLs, clear controller Resolving List, and compare
   pairing/set/local-identity SHA-256 fingerprints.

**Every failure path** includes a best-effort reverse ASE cleanup and, if
the CIG could exist, HCI Remove CIG. An unknown CIG response can still mean
resource allocation; an explicit nonzero LE Set CIG status instead guarantees
that command did not change the CIG. Unconfirmed cleanup is reported as an
error and must never be treated as HIL PASS.

## Test

First run offline tests (ToolHub currently only allows cargo-check/build):

```powershell
cargo test -p bumble-adapter -p bap-client -p be200audiohub
```

Then, with both earbuds ready and BE200 in its existing Windows WinUSB
configuration, execute exactly:

```powershell
.\target\debug\be200hub.exe r5 config-qos `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)" `
    --confirm-cig-qos-write `
    2>&1 | Tee-Object ".\r5-config-qos-1.log"
```

Success requires both ears and *all* cleanup gates:

```text
[R5/ISO] controller capability snapshot: ...
[R5.2/TXN] both ASEs prepared ...
[R4.2/CONFIG] left ... CodecConfigured PASS
[R4.2/CONFIG] right ... CodecConfigured PASS
[R5.2/CIG] HCI Set CIG PASS: CIG=1 CIS_count=2
[R5.2/CIG] allocation: CIS ID=1 handle=0x.... (not established)
[R5.2/CIG] allocation: CIS ID=2 handle=0x.... (not established)
[R5.2/QOS] left ... QoSConfigured PASS
[R5.2/QOS] right ... QoSConfigured PASS
[R4.2/RELEASE] right ... Idle PASS
[R4.2/RELEASE] left ... Idle PASS
[R5.2/CIG] Remove CIG=1 PASS
Persistent-state fingerprint: UNCHANGED
R5.2-A CONFIG QOS PASS
```

If Set CIG is rejected, **do not immediately repeat with alternative RTN,
change/re-pair bonds or assume hardware damage**. Preserve the HCI status and
logs; R5.1's 13 RTN and 95 ms latency remain candidates, not guaranteed to be
schedulable by the BE200. Follow-up negotiation can select another legal
policy without changing R2 bonds.

R5.2-B begins separately at ASCS Enable and HCI Create CIS. **Never reuse**
CIS or ACL connection handles printed by this R5.2-A cleanup probe.
