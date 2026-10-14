# R5.2-B — BE200 Dual CIS Establishment HIL

Status: implementation and Windows `cargo-check`/`cargo-build` complete.
**Not HIL-verified until the explicit test below passes.**

## Scope / Safety

Build on the R5.2-A HIL baseline. This test **really enables two Galaxy
Buds3 Pro Sink ASEs and requests two LE Connected Isochronous Streams** on
the Intel BE200. Both links are intentionally short-lived.

Allowed transient writes:
- ASCS: Config Codec, Config QoS, Enable, Disable (if needed), Release
- HCI: LE Set CIG Parameters, LE Create CIS, Disconnect each established CIS,
  LE Remove CIG
- existing bonded ACL connections/encryption, GATT CCCDs and privacy lists.

Not allowed:
- SMP pairing, key/bond/local-identity mutation
- ASCS Receiver Start Ready: only the **audio sinks** are selected. A sink
  server can transition itself to Streaming when ready.
- ISO Data Path setup, ISO data packets, audio capture or LC3 transmission.
- Retaining or reusing ACL/CIS handles after cleanup.

## State/command sequence

1. **Before any scan/ACL**: check CIS (Host Support) feature bit 32 from
   the BE200 Local LE Features snapshot. If clear, send pinned Bumble-RS
   `HCI_LE_Set_Host_Feature (0x2074, bit_number=32, bit_value=1)`, require
   successful Command Complete, re-read Local LE Features and verify bit32
   is now set. This Host feature is controller-local, not a bond mutation.
   Bluetooth requires this before any ACL exists. It survives during the
   controller session and may need re-enabling after a firmware cold boot.
   **R5.2-B recovery**: issue HCI LE Remove CIG(1) once while no ACL exists,
   accepting only Status=0 (removed an inactive leftover CIG with matching
   CIG ID) or Status=0x02 (no CIG to remove). Any other status, particularly
   0x0C (CIG still active), aborts safely before connecting the earbuds.
2. Resolve both bonded peers, two encrypted ACLs and PACS/ASCS/CAS discovery.
3. Prepare two Sink ASEs and install Control Point + ASE CCCDs.
3. Both ASCS Config Codec: expect matched CP Success, Codec Configured
   notification and independent readback.
4. HCI LE Set CIG Parameters: BE200 returns one CIG with 2 ordered allocated
   CIS Handles (not yet established).
5. Both ASCS Config QoS: same three-way verification, exact QoS field match.
6. Both ASCS Enable with upstream Bumble-RS `Metadata` containing Streaming
   Audio Context = Media (0x0004): CP Success, Enabling notification and
   readback. The returned CIG/CIS IDs and Media Context must match.
7. One **HCI LE Create CIS** Command using both (CIS Handle, ACL Handle)
   pairs. Command Status SUCCESS **alone is not success**.
8. Receive **both** LE CIS Established asynchronous events (Status 0), map
   back to the expected left/right CIS IDs and verify
   `Device::cis_link(handle)` for both. Print controller-reported PHY,
   transport latency, NSE, BN, FT, Max PDU and ISO Interval.
9. Read the active Sink ASE states: Enabling or autonomous Streaming both
   accepted, with matching CIG/CIS.
10. Teardown even on any earlier failure:
    - HCI Disconnect for any established CIS and confirm they are gone.
    - If needed, ASCS Disable transitions Sink ASE back to QoS Configured.
    - ASCS Release each attempted ASE back to Idle.
    - HCI Remove CIG; verify returned CIG ID.
    - Existing R3 dual ACL disconnect, privacy cleanup and bond fingerprint.

Any stage/cleanup uncertainty is a failure. Preserve the log and **do not
repeat immediately** when teardown reports UNCONFIRMED.

## Offline tests

```powershell
cargo test -p bumble-adapter -p bap-client -p be200audiohub
```

ToolHub currently allows cargo-check and cargo-build only; a successful
build does **not** prove `#[cfg(test)]` code has compiled and run.

## Explicit HIL command

Run with **both earbuds ready** and the BE200 attached to the existing
WinUSB stack. This command goes beyond R5.2-A and truly creates LE CIS
radio links; it requires a **new, distinct confirmation flag**.

```powershell
.\target\debug\be200hub.exe r5 establish-cis `
    --seconds 8 `
    --name "Galaxy Buds3 Pro (2F23)" `
    --confirm-cis-create `
    2>&1 | Tee-Object ".\r5-establish-cis-1.log"
```

Expected success markers:

```text
[R5.2/HOST] pre-ACL CIS Host Support bit32=...
[R5.2/HOST] CIS Host Support bit32 READBACK PASS
[R5.2/HOST] READY: bit32_enabled=true ...
[R5.2/RECOVERY] CIG=1 absent before ACL PASS
  (or: removed leftover inactive CIG=1 before ACL PASS)
[R4.2/CONFIG] left and right ... PASS
[R5.2/CIG] HCI Set CIG PASS
[R5.2/QOS] left ... QoSConfigured PASS
[R5.2/QOS] right ... QoSConfigured PASS
[R5.2/ENABLE] left ... Enabling PASS
[R5.2/ENABLE] right ... Enabling PASS
[R5.2/CIS] LE Create CIS Command Status: SUCCESS
[R5.2/CIS] CIS=1 ... Established event PASS
[R5.2/CIS] CIS=2 ... Established event PASS
[R5.2-B/CIS] DUAL CIS ESTABLISHED: 2/2 ...
[R5.2/STATE] left ... PASS
[R5.2/STATE] right ... PASS
[R5.2/CIS] CIS=2 ... disconnected PASS
[R5.2/CIS] CIS=1 ... disconnected PASS
[R4.2/RELEASE] ... Idle PASS
[R5.2/CIG] Remove CIG=1 PASS
Persistent-state fingerprint: UNCHANGED
R5.2-B DUAL CIS PASS
```

During cleanup, `[R5.2/DISABLE]` appears only if the earbud remains
Enabling/Streaming after its CIS disconnects.

## Diagnostic branches

- **Historical 0x0C Command Disallowed in first R5.2-B HIL**: Both Codec,
  CIG, Config QoS, Enable and rollback passed, but HCI LE Create CIS returned
  0x0C before asynchronous establishment. Comparison with the Bluetooth Core
  spec and Linux `net/bluetooth/hci_sync.c` found our
  `initialize_existing_operational` did not configure the Controller's
  CIS (Host Support) feature bit32, unlike Linux's
  `hci_le_set_host_feature_sync`. The new pre-ACL gate tests this hypothesis.
  If the feature readback is already **true** and Create CIS still returns
  0x0C, investigate other command-disallowed conditions rather than assuming
  the fix worked. No conclusion until real HIL passes.
- **HCI Create CIS Command Status nonzero**: this is an explicit controller
  rejection. Preserve HCI status, do not assume either CIS established.
- **Command Status 0, one or both LE CIS Established events missing**:
  asynchronous link procedure failed/timed out; cleanup may be uncertain.
- **CIS Established status nonzero**: preserve controller status and which
  allocated CIS handle failed. The other CIS may already be established.
- **ASE readback differs after establishment**: link succeeded at HCI level,
  but ASCS server state is not proven; teardown still required.
- **Historical second R5.2-B HIL: 2/2 CIS Established PASS, teardown
  UNCONFIRMED**: Both earbuds autonomously reached Streaming (0x04);
  controller reported 2M PHY, 63,210us C->P transport latency,
  NSE=2/BN=1/FT=7/MaxPDU=100/ISO Interval=8.
  Our CIS Disconnect incorrectly used command Reason=0x16, which the Core
  HCI_Disconnect command *does not permit*. 0x16 is the **local Disconnection
  Complete event Reason**, not a legal command Reason. BE200 returned
  Invalid HCI Command Parameters (0x12) for both CIS. Because the CIS
  remained active, both Release attempts timed out awaiting final Idle
  notifications and Remove CIG returned Command Disallowed (0x0C).
  Both ACLs subsequently disconnected with implicit CIS disconnection events,
  but that did not prove CIG release. The next R5.2-B run uses HCI Disconnect
  command Reason=0x13 (permitted by Core 7.1.6); local event Reason remains
  0x16. A pre-ACL cleanup gate reconciles any inactive CIG 1 leftover from
  the failed run.
- **CIS Disconnect/Remove CIG fails**: do not claim PASS or start another HIL
  attempt until controller resources and both ASE states are inspected.
  The pre-ACL recovery gate must prove that CIG 1 was absent or removed
  before any new CIS creation.
- **Spontaneous early remote streaming/disconnect**: expected possibility
  when no ISO data packets are sent. Log both HCI and ASE observations.

## Next milestone

References: [Linux Host Feature bit32 setup](https://codebrowser.dev/linux/linux/net/bluetooth/hci_sync.c.html) and [Bluetooth Core HCI spec](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-60/out/en/host-controller-interface/host-controller-interface-functional-specification.html).

R5.3 builds only after 2/2 CIS Established plus cleanup HIL passes:
prepare ISO Data Path in the BE200 controller, implement LC3 encode and
transmit two independent audio SDU streams, validate left/right mapping and
first audible output.
