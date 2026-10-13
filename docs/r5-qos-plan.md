# R5.1 — BE200 dual-mono QoS/CIG planning

Scope: **pure planning + validated serialization**. No HCI Set CIG,
ASCS Config QoS, HCI Create CIS, ISO Data Path or ISO audio packets.

## Inputs

1. R4.1 `CodecConfigurationPlan`: LC3 48 kHz, 10 ms, 100 octets/frame,
   one mono Sink ASE on each side, channel assignment from PACS Sink Audio
   Location (not from CSIS rank).
2. R4.2 `CodecConfiguredSetSnapshot`, 2/2 members, Control Point Response,
   ASE notification, state readback and Release-to-Idle verified. This is a
   transient snapshot; no bond or controller stream config is persisted.
3. `AttachedController::iso_controller_capabilities()`: read-only
   `ExternalControllerInfo` cached during operational attach. Fail closed
   if CIS Central, requisite CIG/CIS and ISO Data Path HCI commands or ISO
   data buffer capacity are missing.

## Selection and mapping

- CIG ID = 1; left CIS ID = 1; right CIS ID = 2.
- Two downlink CIS entries: Central->Peripheral Max SDU = 100, PHY = 2M
  only if advertised by controller and preferred by *both* earbuds.
  Otherwise use 1M. Per-CIS downlink RTN uses that member's preference.
- Peripheral->Central Max SDU = 0, RTN = 0, valid 1M PHY. The HCI
  reverse-direction SDU interval and transport latency stay nonzero.
- SDU interval = codec frame duration (one frame per SDU), 10,000 us.
  Both ears use identical framing and latency bounds.
- Transport latency = minimum of both reported maximum preferences,
  constrained to 5..4000 ms. The first observed B3P pair reports 95 ms.
- Presentation delay is selected from the intersection of the two **hard**
  supported delay ranges. Use their preferred intersection when available;
  disagreement in *preferences* does not invalidate a valid hard overlap.
  The first observed B3P pair reports fixed 40,000 us.
- Reuses pinned Bumble-RS `CigParameters`/`CisParameters`,
  `ConfigQosParameters`, `AseOperation::to_bytes()` and
  `Command::LeSetCigParameters::to_bytes()`.
- Check upstream ASCS encode/decode roundtrip and full HCI wire header.
  Any invalid or unsupported input is a failure, not a default.

Controller support for **two concurrent CIS at the chosen RTN and latency**
cannot be established by the feature bitmap alone. R5.2 Set CIG Parameters
return status is the authoritative runtime validation. This dry-run does not
claim that a 13-RTN / 95-ms combination has been accepted by hardware.

## Windows commands

Offline tests:

```powershell
cargo test -p bumble-adapter -p bap-client -p be200audiohub
```

R5.1 live plan. This **intentionally writes only the already HIL-verified**
R4.2 ASCS Config Codec and Release commands in order to capture transient QoS
preferences; it releases both ASEs back to Idle and disconnects the ACLs.
The --confirm-ascs-write flag is required. No QoS or HCI CIG writes occur.

```powershell
.\target\debug\be200hub.exe r5 qos-plan `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)" `
    --confirm-ascs-write `
    2>&1 | Tee-Object ".\r5-qos-plan-1.log"
```

Expected gates:

```text
[R5.1/ISO] controller capability snapshot: ...
[R4.2/PREP] left ...
[R4.2/PREP] right ...
[R4.2/CONFIG] left ... PASS
[R4.2/CONFIG] right ... PASS
[R4.2/RELEASE] right ... Idle PASS
[R4.2/RELEASE] left ... Idle PASS
Persistent-state fingerprint: UNCHANGED
StereoQosPlan: ...
HCI LE Set CIG Parameters wire (NOT SENT): ...
R5.1 QOS PLAN PASS
```

If an ISO capability gate or planning rule fails, preserve the full log; do
not re-pair or clear bonds. Never reuse the printed ACL connection handles
after program exit; they are only reference mappings from the dry run.

## Implementation boundaries

- `crates/bumble-adapter/src/iso_capabilities.rs` — read-only capability
  snapshot and ISO HCI buffer-size gate.
- `crates/bap-client/src/qos_plan.rs` — pure decision, validations,
  upstream serialization and synthetic regression fixtures.
- `src/audio_set.rs` — R4.2 capture/Release → R5.1 pure plan.
- `src/main.rs` — CLI arity/consent regression tests.

## R5.2 entry

Only after offline tests and real R5.1 HIL pass:
split R4.2 into a persistent `Codec Configured` session, execute HCI Set
CIG, ASCS Config QoS, confirm QoS Configured, then finally Enable and
Create CIS. Preserve the existing rollback and bond fingerprint gates.
