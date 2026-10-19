# BE200AudioHub — Round8 / V2 Structural Closure + Live Relay HIL

> **Status**: V2 Structural / Ownership Refactor **CLOSED**; Post-Round8 live product-path HIL/debug **ACTIVE**.
>
> **Branch**: `refactor/v2-architecture`
>
> **Hardware**: Intel BE200 + Samsung Galaxy Buds3 Pro
>
> **Important naming note**: the CLI/review namespace remains `round8` for current-path and live-relay HIL. This does **not** mean the structural refactor itself is still open.

---

## 1. Round8 closure boundary

Round8 froze the ownership architecture:

```text
L0 Core
L1 Control
L1 Media
L1 HCI
L2 Transport
L3 Platform
```

Key invariants carried forward:

1. Media time never waits for HCI credits.
2. Number Of Completed Packets releases controller capacity only.
3. HCI completion never advances FrameId.
4. Media never sees raw CIS handles.
5. HCI owns ISO sequence/PB/fragmentation.
6. Logical routes are session-fenced.
7. Route ACK gates StreamBinding.
8. Stale Media is dropped instead of creating unbounded latency.
9. Teardown is route-first and semantic.
10. Teardown retirement is not controller completion.

The structural pass also decomposed Control into durable model, operation progress,
engine/reducers, discovery/planning/ASCS and interface boundaries. No new global
RuntimeState/Aggregator was introduced.

---

## 2. Round7 / current-path hardware closure inherited into Round8

Round7 valid-LC3 First Sound passed on real hardware before the live-relay work.

Frozen product path:

```text
valid LC3
→ StreamClock
→ IsoTxBridge
→ logical HCI route
→ BE200
→ Galaxy Buds3 Pro
```

Representative First Sound HIL:

```text
frames=100
submitted=200
completed=200
in_flight_after=0
controller ISO window=Some(8)
route unbind PASS
ISO drain PASS
reverse teardown PASS
BondStore unchanged=true
```

Round8 current-path First Sound also passed with the frozen 2 s / -40 dBFS
profile and the same 48 kHz / 10 ms / 100 B-per-ear transport parameters.

---

## 3. Windows live relay migration

The legacy Windows relay source side was migrated without carrying over the old
destination WASAPI renderer.

Current product path:

```text
Windows applications
→ sacrificial physical render endpoint
→ Process Loopback
→ PcmRelayBoundary
→ SPSC Float32 stereo ring
→ LC3
→ IsoTxBridge / StreamClock
→ logical HCI ISO
→ BE200
→ Buds3 Pro
```

Frozen PCM boundary:

```text
48 kHz Float32 stereo
capture ~10 ms / 480 frames
ring physical capacity 80 ms / 3840 frames
retained cushion 10 ms / 480 frames
startup hold ceiling 40 ms / 1920 frames
activation request+cushion = 960 frames / 20 ms
```

The first Process Loopback HIL failed with `E_ILLEGAL_METHOD_CALL`. The raw
completion handler did not expose `IAgileObject`. After adding agile COM
support, two consecutive 10 s live HIL runs passed with audible Windows audio.

Clean 10 s reference:

```text
Process Loopback:
  packets=1000
  frames=480000
  dropped=0
  discontinuities=0

Media:
  intervals=1000
  keepalive=2
  underflow_frames=960
  startup_trim=0
  gradual_trim=0
  emergency_trim=0

Bridge:
  submitted=1000
  backpressure_drop=0
  fallback=0
  grid_skipped=0
  stale=0

ISO:
  submitted=2000
  completed=2000
  in_flight=0

StopControl -> Idle PASS
BondStore unchanged=true
```

The initial 960 underflow frames are the expected 20 ms source-startup window.

---

## 4. Longevity telemetry

Commit `a11efdff` added read-only live telemetry snapshots. Longevity mode
prints ten equally-spaced checkpoints: 10%, 20%, ..., 100%.

Per-window metrics:

- `pcm_drop`: Process Loopback frames not written into the PCM ring.
- `underflow`: Media-requested PCM frames unavailable from the ring.
- `bridge_drop`: stereo intervals dropped by HCI/bridge backpressure.
- `iso_host_gap = Δsubmitted - Δcompleted - Δin_flight`: Host accounting
  conservation gap, **not** RF/OTA PER.

Multiple 30 s and 300 s runs showed a stable live data plane:

```text
pcm_drop      = 0
bridge_drop   = 0
iso_host_gap  = 0
```

A small number of isolated 480-frame underflows appeared in some 30 s runs.
This is tracked separately as a negative-drift / scheduling issue and is not the
cause of the teardown failure described below.

---

## 5. 300 s longevity result: data window passed, teardown failed

A full five-minute run reached:

```text
[longevity] elapsed 300/300 s (100.0%)
```

with clean Host ISO conservation, then failed only during graceful Stop:

```text
TeardownFailed {
  cleanup: [
    Timeout(CleanupAse { side: Right }),
    Timeout(CleanupAse { side: Left })
  ]
}
```

Therefore the 300 s data window itself passed. The current unresolved issue is
the post-stream control-plane teardown.

---

## 6. Graceful teardown corrections made during this chat

### 6.1 Normal Stop is distinct from failure rollback

Commit `742eb75c` split the policies.

Normal Stop:

```text
route
→ ASCS Release
→ ISO path
→ CIS
→ CIG
→ ACL/privacy
```

Failure rollback remains fail-closed:

```text
route
→ ISO path
→ CIS
→ ASCS Release
→ CIG
→ ACL/privacy
```

### 6.2 Release cached Codec Configured compatibility

Commit `a48b2a54` accepts `CODEC_CONFIGURED` as a valid Release terminal
readback in addition to `IDLE`, matching the caching behavior visible in the
reference implementations.

### 6.3 RELEASING is the graceful transport-teardown pivot

Commit `96cf90a5` changed graceful Stop so Release does not wait for terminal
Idle/Codec Configured before transport teardown.

Reference choreography:

```text
Release
→ ASE = RELEASING
→ Remove ISO data path
→ Disconnect CIS
→ terminal Idle / cached Codec Configured may follow
```

A notification-gated Release path was added for graceful Stop; failure rollback
retains terminal readback verification.

These corrections were necessary but did **not** fully solve the 30 s teardown
failure.

---

## 7. Teardown gate diagnostics: exact observed failure boundary

Commit chain:

```text
41742199  diag(control): expose graceful release gate timeout state
2672f75a  fix(diag): attach release gate snapshot to ASE operation
30a8589c  test(control): import release gate phase
```

The diagnostic snapshot records:

```text
phase
pending_request_id
write_complete
cp_success
notified_state
```

Before ASCS write-method alignment, a 30 s run showed both ears stuck at:

```text
phase=WaitingGates
pending_request_id=Some(...)
write_complete=false
cp_success=false
notified_state=None
```

This proved the failure was earlier than CP success or ASE RELEASING.

---

## 8. ASCS Control Point alignment: Write Without Response

Reference review showed the product ASCS Control Point should not depend on an
ATT Write Response.

Commit:

```text
85300c88  control: use write command for ASCS control point
87c3352c  test(control): align ASCS write-command expectations
```

Current behavior:

```text
ASCS Control Point
→ GattOperation::WriteCommand
→ local WriteCommand completion
→ require CP SUCCESS notification
→ require expected ASE state notification
→ strict readback where the operation requires it
```

CCCD subscription remains a Write Request.

After this change, a 30 s normal-mode HIL still failed, but the gate moved:

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

This is the **current authoritative failure signature**.

Interpretation:

> The ASCS Write Command completed locally in the GATT transaction layer, but
> neither ear produced a Control Point SUCCESS notification nor an ASE state
> notification before the cleanup deadline.

Local Write Command completion is **not** proof that the ACL packet actually
left Bumble's Host queue or reached BE200.

---

## 9. Current lower-boundary diagnostic: ACL teardown audit

Latest pre-documentation HEAD:

```text
d1fb532c  diag(hci): expose ACL queue state on teardown failure
084fa605  fix(diag): wire ACL teardown audit
```

The failure path now requests an `AclCreditSnapshot`:

```text
bumble_pending
audit_in_flight
audit_limit
waiting_estimate = bumble_pending - audit_in_flight
```

Definitions:

- `bumble_pending`: Bumble ACL packets queued or in flight.
- `audit_in_flight`: ACL packets that actually crossed
  `QueueHostTransport::send_acl_packet` and have not yet been retired by HCI
  Number Of Completed Packets.
- `waiting_estimate`: packets still waiting above the actual HCI submit
  boundary.

Interpretation for the **next HIL**:

```text
waiting_estimate > 0
→ packet(s) are stuck in Bumble Host ACL queue
→ investigate Bumble LE ACL DataPacketQueue / credit projection / flush

waiting_estimate == 0 && audit_in_flight > 0
→ packet(s) reached HCI submit boundary
→ controller completion is still outstanding

waiting_estimate == 0 && audit_in_flight == 0
→ Host/controller TX retired
→ missing CP/ASE response is above the ACL-completion boundary
→ investigate peer ASCS behavior, receive path, notification state, or TWS
  multi-device Release choreography
```

**No real-hardware output from this ACL teardown audit has been recorded yet.**
Do not infer the result.

---

## 10. The key A/B evidence

Same product path and same hardware:

```text
1 s normal mode
→ complete StopControl reverse teardown
→ Control Idle PASS

30 s normal mode
→ dual AseReleaseGateTimeout
→ write_complete=true
→ cp_success=false
→ notified_state=None
```

Therefore:

- the failure is not specific to `--longevity-test`;
- the 10%-snapshot observer path is exonerated by the 30 s normal-mode failure;
- the problem is time-/state-dependent and appears after a longer established
  streaming session;
- data-plane ISO accounting can remain healthy while the later ACL/GATT control
  path fails.

---

## 11. User-space Host crash / Ctrl+C fail-safe defect

A separate, confirmed lifecycle defect was reproduced.

A live run was interrupted with Ctrl+C while the userspace Host owned active
Bluetooth state. The next launches repeatedly failed peer discovery with:

```text
Discovery(MemberNotVisible { member_index: 0 })
```

Re-boxing the earbuds alone did not fix it. A real BE200 power cycle immediately
restored discovery.

This strongly supports:

```text
Host process died
while controller/peer session state survived
→ next generation performed resetless attach
→ Host/controller ownership became desynchronized
```

The project intentionally uses resetless attach for the clean fast path, unlike
Bumble's default power-on behavior which resets/reinitializes the controller.

### Required product fail-safe — OPEN

Planned policy:

```text
process start
    ↓
previous ownership proof?
    ├─ CLEAN  → resetless attach
    └─ DIRTY / UNKNOWN
              → HCI Reset + full Host reinitialization
```

Runtime requirements:

1. First Ctrl+C requests graceful shutdown; second Ctrl+C may force exit.
2. Durable DIRTY marker is written before taking controller ownership.
3. Marker is cleared only after teardown is proven complete.
4. Panic/process kill/worker death leaves DIRTY.
5. Next generation treats DIRTY/UNKNOWN controller truth as untrusted and resets.
6. Product supervisor must invalidate StreamBinding immediately on terminal
   worker/control faults.
7. Cleanup failure must **not** mark the previous generation clean.

This is a confirmed open defect, not an optional enhancement.

---

## 12. Reference-alignment audit retained from this round

The full audit concluded that the architecture should **not** be rewritten.
Highest-priority remaining findings include:

- unsolicited CIS/ACL disconnect can leave Control binding stale;
- autonomous ASE Disable/Release reporting is incomplete;
- FaultSink delivery can drop a correctness fault;
- product supervisor/timer owner is not fully composed;
- capture/transport worker death propagation is incomplete;
- LE encryption key-size admission is missing;
- stereo lower-TX atomicity has a narrow failure window;
- negative drift is not corrected;
- Media/WASAPI/USB scheduling is not yet product RT/MMCSS-grade;
- async USB TX global bounding/cancel deadlines need hardening;
- QoS preference consumption is partial;
- local outgoing privacy/RPA use remains incomplete.

Prioritization remains:

```text
Wave 1  correctness / fault / recovery
Wave 2  longevity / scheduling / clock sync
Wave 3  interop / portability
```

The current teardown and crash-recovery work belongs to **Wave 1**.

---

## 13. Immediate next-chat action

Do **not** redesign Release again before collecting the new ACL audit.

Recommended next real-HIL sequence:

1. Because the last teardown failed, put both Buds in the case and power-cycle
   BE200 once to start from a known clean controller state.
2. Pull/build current branch.
3. Run 30 s **normal mode** first; no longevity flag is required.
4. If teardown fails, capture the appended line:

```text
ACL teardown audit:
  bumble_pending=...
  audit_in_flight=...
  waiting_estimate=...
  limit=...
```

5. Use the interpretation in §9 to choose the next layer.

Exact command:

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

Do not run another 300 s longevity test until a 30 s teardown is clean.

---

## 14. Commit anchors for this chat boundary

```text
742eb75c  control: separate graceful Stop from failure rollback ordering
a48b2a54  control: accept cached codec state after ASE release
a11efdff  review: add longevity runtime snapshots
96cf90a5  control: pivot graceful release on ASE releasing state
190d9b06  test(control): align teardown helpers with release semantics
41742199  diag(control): expose graceful release gate timeout state
2672f75a  fix(diag): attach release gate snapshot to ASE operation
30a8589c  test(control): import release gate phase
85300c88  control: use write command for ASCS control point
87c3352c  test(control): align ASCS write-command expectations
d1fb532c  diag(hci): expose ACL queue state on teardown failure
084fa605  fix(diag): wire ACL teardown audit
```

The documentation commit created at this chat migration is newer than the list
above and contains documentation only.

---

## 15. Claim boundary at handoff

### Proven

- Valid-LC3 First Sound on real BE200 + Buds3 Pro.
- Live Windows Process Loopback → own LC3/HCI/CIS path audible end-to-end.
- Two consecutive clean 10 s live runs.
- A full 300 s live data window reached 100% with Host ISO conservation.
- 1 s current live-relay teardown can complete cleanly.
- 30 s current live-relay teardown reproducibly reaches the Release gate failure
  described above.
- Ctrl+C can leave controller/peer state dirty enough to require BE200 power
  cycle under the current resetless ownership model.

### Not yet proven

- Clean 30 s/300 s graceful teardown on the current path.
- Root cause location of the 30 s Release failure below/above Bumble ACL queue.
- Crash-safe controller ownership recovery without manual power cycle.
- Full product supervisor/runtime composition.
- Long-duration drift/scheduling acceptance after teardown is fixed.

