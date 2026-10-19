# BE200AudioHub — Round9 / Teardown, Crash Recovery, and ACL Boundary Diagnosis

> **Status**: **ACTIVE / HANDOFF**
>
> **Branch**: `refactor/v2-architecture`
>
> **Hardware**: Intel BE200 + Samsung Galaxy Buds3 Pro
>
> **Start boundary**: Round9 continues from Round8 commit `a11efdff`. The CLI still uses `round8 live-relay`; that name is a test namespace, not the Engineering Round number.

---

## 1. Round9 opening: dirty controller after abnormal Host death

A repeated setup failure appeared after a prior live-relay process had been
interrupted with Ctrl+C:

```text
Discovery(MemberNotVisible { member_index: 0 })
```

Re-boxing/retrying did not fix it. Power-cycling the BE200 immediately restored
normal operation.

The important interpretation is not “hardware dead” but userspace
Host/controller ownership desynchronization:

```text
Host process dies
→ Host semantic state disappears
→ BE200 controller may retain ACL/CIS/CIG/privacy/session state
→ peer may remain in the old session
→ next process does resetless attach against unknown controller truth
```

This is a confirmed product-runtime defect for a userspace Host.

Frozen recovery direction:

```text
previous generation CLEAN
→ resetless attach

previous generation DIRTY / UNKNOWN
→ HCI Reset
→ full Host/controller reinitialization
```

Normal Ctrl+C should request graceful Stop. A durable DIRTY/CLEAN ownership
marker and product supervisor are Wave-1 work. Only a proven successful
teardown may clear DIRTY.

Reference comparison used in the decision:

- Bumble normally resets/reinitializes the controller on a fresh Host power-on.
- AOSP relies on a supervised Bluetooth service/HAL lifecycle and restarts on
  unrecoverable Host/controller desynchronization.
- BlueZ keeps the durable HCI owner in the Linux kernel; `bluetoothd` is not
  equivalent to our sole userspace raw-HCI owner.

---

## 2. Graceful Release model corrected to use RELEASING as the pivot

The first Round9 teardown correction was commit:

```text
96cf90a5  control: pivot graceful release on ASE releasing state
190d9b06  test(control): align teardown helpers with release semantics
```

The previous model still treated Release as one synchronous transaction that
had to reach terminal `IDLE` / cached `CODEC_CONFIGURED` before transport
teardown.

Reference-aligned graceful choreography:

```text
Release
→ ASE = RELEASING
→ Remove ISO Data Path
→ Disconnect CIS
→ terminal IDLE / cached CODEC_CONFIGURED may follow
```

Failure rollback remains separate and fail-closed.

A 1 s normal live-relay run passed complete teardown, but 30 s runs still
failed. Therefore the bug was not simply the terminal-state pivot.

---

## 3. Gate-level teardown diagnostics

Commits:

```text
41742199  diag(control): expose graceful release gate timeout state
2672f75a  fix(diag): attach release gate snapshot to ASE operation
30a8589c  test(control): import release gate phase
```

`AseOperationGateSnapshot` records:

```text
phase
pending_request_id
write_complete
cp_success
notified_state
```

Before the ASCS write-method correction, a 30 s run showed both ears at:

```text
phase=WaitingGates
pending_request_id=Some(...)
write_complete=false
cp_success=false
notified_state=None
```

This proved the timeout happened before CP success or ASE `RELEASING`.

---

## 4. Longevity heartbeat exonerated

The 10%-interval longevity snapshot path performs an active `IsoTxAudit`
query and drains/processes reliable HCI reports while waiting for the audit
reply. It was initially suspected as an observer-side effect.

A controlled A/B disproved that hypothesis:

```text
1 s normal mode   → graceful teardown PASS
30 s longevity    → dual Release-gate timeout
30 s normal mode  → same dual Release-gate timeout
```

Therefore the fault is time/state dependent and not specific to
`--longevity-test`.

---

## 5. ASCS Control Point aligned to Write Without Response

Reference review showed the product ASCS Control Point should not depend on an
ATT Write Response.

Commits:

```text
85300c88  control: use write command for ASCS control point
87c3352c  test(control): align ASCS write-command expectations
```

Current ASCS operation path:

```text
ASCS Control Point
→ GattOperation::WriteCommand
→ local WriteCommand completion
→ require CP SUCCESS notification
→ require expected ASE state notification
→ strict readback only where that operation requires it
```

CCCD subscription remains `WriteRequest`.

After this correction, the authoritative 30 s normal-mode failure became:

```text
Right:
  phase=WaitingGates
  pending_request_id=None
  write_complete=true
  cp_success=false
  notified_state=None

Left:
  phase=WaitingGates
  pending_request_id=None
  write_complete=true
  cp_success=false
  notified_state=None
```

Interpretation:

> The local GATT transaction accepted/completed the Write Command, but neither
> ear produced a Control Point SUCCESS notification nor an ASE-state
> notification before the cleanup deadline.

This still does **not** prove that the packet actually crossed Bumble's ACL
waiting queue into HCI TX.

---

## 6. Current lower boundary: Bumble ACL queue vs actual HCI submit

Latest implementation commits before this documentation correction:

```text
d1fb532c  diag(hci): expose ACL queue state on teardown failure
084fa605  fix(diag): wire ACL teardown audit
```

The teardown failure path now exposes an ACL audit:

```text
bumble_pending
audit_in_flight
audit_limit
waiting_estimate = bumble_pending - audit_in_flight
```

Definitions:

- `bumble_pending`: Bumble LE ACL packets queued or in flight.
- `audit_in_flight`: packets that actually crossed
  `QueueHostTransport::send_acl_packet` and have not yet been retired by HCI
  Number Of Completed Packets.
- `waiting_estimate`: packets still waiting above the actual HCI submit
  boundary.

Next-HIL classification:

```text
waiting_estimate > 0
→ Release ATT/L2CAP packet is stuck in Bumble Host ACL queue
→ inspect Bumble LE ACL DataPacketQueue / credit projection / flush

waiting_estimate == 0 && audit_in_flight > 0
→ packet crossed the HCI submit boundary
→ controller completion is still outstanding

waiting_estimate == 0 && audit_in_flight == 0
→ Host/controller TX has retired
→ investigate receive/peer ASCS/TWS multi-device choreography
```

**No real-hardware output from this ACL teardown audit has been recorded yet.**
Do not infer the result.

---

## 7. Other Round9 observations

### 7.1 Isolated PCM underflow

Some 30 s runs showed isolated 480-frame underflows while:

```text
pcm_drop      = 0
bridge_drop   = 0
iso_host_gap  = 0
```

This is tracked separately as a negative-drift / scheduling issue. It is not
the cause of the current teardown failure.

### 7.2 300 s rerun intentionally deferred

Do not burn another five-minute run until 30 s teardown is clean. The 300 s
data plane has already reached 100%; the unresolved acceptance blocker is
graceful teardown.

---

## 8. Current deterministic gate

At the Round9 handoff point:

```text
cargo test --workspace --locked
410 passed / 0 failed

cargo check --workspace --locked
PASS

cargo build --workspace --locked
PASS
```

Implementation boundary immediately before the documentation correction:

```text
084fa605  fix(diag): wire ACL teardown audit
```

---

## 9. Exact next HIL

Start from a clean BE200/Buds state if the previous attempt ended in failed
teardown. Run normal 30 s live relay; do not use longevity mode for the first
ACL-audit capture.

```powershell
.\target\debug\be200hub.exe `
    round8 live-relay `
    --sfi (Join-Path $fw 'be200_intel_windows_24.60.0.4_usb_verified.sfi') `
    --ddc (Join-Path $fw 'ibt-0291-0291.ddc') `
    --bond-root $bond `
    --name "Galaxy Buds3 Pro (2F23)" `
    --connect-seconds 12 `
    --seconds 30 `
    --confirm-live-relay-hil
```

If teardown fails, preserve the appended ACL audit and classify it using §6.
Do **not** go back to guessing Release ordering before that evidence is read.

---

## 10. Round9 handoff claim boundary

Allowed:

> Round8 product live relay is real-HIL proven for short sessions and the
> 300-second data window has reached 100%. Round9 has localized the remaining
> long-session graceful-teardown failure below local ASCS WriteCommand
> completion and has instrumented the Bumble ACL queue / HCI-submit boundary.

Not yet allowed:

> 30 s graceful teardown PASS.

Not yet allowed:

> 300 s full-session longevity PASS.

Not yet implemented:

> crash-safe product supervisor / durable controller ownership marker /
> conditional reset-and-reinitialize startup.

See:

- [Round8.md](./Round8.md) — closed structural/live-relay baseline
- [TIMELINE.md](./TIMELINE.md)
- [BUG_FIX_REGISTER.md](./BUG_FIX_REGISTER.md)
- [../design/LIVE_AUDIO_RELAY_MIGRATION.md](../design/LIVE_AUDIO_RELAY_MIGRATION.md)
