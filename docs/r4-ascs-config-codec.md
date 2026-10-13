# R4.2 — ASCS Config Codec smoke transaction

## Safety and prerequisites

R1–R3 dual-member bond, IRK/RPA resolution, encrypted dual ACL, PACS/ASCS
discovery and R4.1 LC3 codec planning must pass first.

This command **writes ASCS Control Point** on the selected Sink ASE(s). It
does not initiate SMP pairing, re-bond, update the bond store, configure QoS,
create CIG/CIS, or stream ISO. It intentionally reuses existing stored keys.

Both members must present a supported, available Media context, an LC3
48 kHz/10 ms/100-octet-per-frame mono Sink PAC, and a left/right Idle Sink ASE.
The planner chooses the lowest Idle ASE ID on each side, independent of CSIS rank.

## Runs

From the Windows workspace (BE200 under WinUSB), verify the offline tests:

```powershell
cargo test -p bap-client
cargo build -p be200audiohub --bin be200hub
```

Single ear first (smoke test):

```powershell
.\target\debug\be200hub.exe r4 config-codec `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)" `
    --side left `
    --confirm-ascs-write `
    2>&1 | Tee-Object ".\r4-config-codec-left-1.log"
```

Then test both ears:

```powershell
.\target\debug\be200hub.exe r4 config-codec `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)" `
    --side both `
    --confirm-ascs-write `
    2>&1 | Tee-Object ".\r4-config-codec-both-1.log"
```

## Implementation

- `bumble-adapter/src/gatt_session.rs`: synchronously capture pending
  unsolicited ATT PDUs during requests and continue pumping them after the
  temporary GATT transport is dropped. Each event carries an ACL handle.
  ATT indications receive Handle Value Confirmation.
- `bap-client/src/codec_config.rs`: negotiate ATT MTU to fit the CP payload,
  subscribe ASCS ASE/CP CCCDs, re-read selected ASE as Idle, then use the
  upstream `AudioStreamControlServiceProxy::write_operation`.
- Treat CP Success **and** the matching ASE Codec Configured notification as
  necessary; independently read the ASE state back before claiming success.
  Record the peer's QoS/presentation-delay preferences for the R5 planner.
- On single- or dual-ear completion, issue ASCS Release and require CP Success,
  an Idle notification and an Idle readback on each attempted ASE.
- On an uncertain write failure, inspect and attempt to Release both relevant
  ASEs in reverse order. The log explicitly reports `rollback UNCONFIRMED`
  if cleanup cannot prove Idle; it must not claim success.
- Disconnect both ACLs, clear controller privacy and compare persistent
  bond/set/local-identity fingerprints. No ACL handle is reusable after exit.

## Completion evidence

R4.2 is HIL PASS **only if** all selected ears show:

1. `[R4.2/PREP]` with MTU/CCCD/Idle PASS
2. `[R4.2/CONFIG]` with CP + notify + readback PASS
3. `[R4.2/RELEASE]` with Idle PASS
4. `Persistent-state fingerprint: UNCHANGED`
5. `R4.2 CONFIG CODEC PASS`

Compile success alone is not hardware validation. If ASCS write or rollback
fails, preserve the full log; never reset the bond as a troubleshooting step.
