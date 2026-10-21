# BE200AudioHub — Round11 / Product Policy, Live Daily Path, Latency, Runtime Faults, and Lifecycle/Telemetry Handoff

> **Status**: **ROUND11 CLOSED / PRODUCT DATA PATH STABLE ENOUGH FOR DAILY HARDENING / L0 LIFECYCLE NOT YET COMPOSED**
>
> **Authoritative code checkpoint before this docs-only closure**: `8d27d5ca61486bfe5c1bfdb7d51349bd51156110`
>
> **Branch**: `main`
>
> **Hardware**: Intel BE200 + Samsung Galaxy Buds3 Pro
>
> **Important evidence rule**: this document separates real HIL/runtime observations, current
> repository facts, and still-open hypotheses. It does not promote a plausible multi-CIS or
> controller mechanism into a proven root cause.

---

## 1. Round11 starting boundary

Round10 ended with two deliberately separate facts:

1. the long-session graceful-teardown investigation had localized an unresolved
   controller-global Host-facing HCI ACL-RX cessation and deprecated the large forensic harness;
2. the final normal product teardown architecture decision was intentionally deferred.

Round11 first closed that architecture decision, then shifted from teardown forensics to
productization of the already-working live audio path:

```text
Windows applications
→ Process Loopback
→ bounded stereo PCM
→ LC3
→ Media StreamClock / IsoTxBridge
→ logical HCI ISO
→ BE200
→ dual CIS
→ Galaxy Buds3 Pro
```

The round then expanded into GUI/daily-use controls, latency characterization, long-run behavior,
a spontaneous HCI worker/session failure, the recurring stereo-image anomaly, and finally the
pre-Lifecycle Condition/telemetry audit.

---

## 2. Final teardown architecture decision

Round11 freezes the normal architecture as protocol-graceful and keeps the BE200 workaround
explicit.

### 2.1 Architectural default

```text
stop Media production
→ stop Bridge submissions / drain admitted ISO traffic
→ unbind logical stream route
→ ASCS Release
→ CP SUCCESS + ASE RELEASING
→ Remove ISO Data Path
→ Disconnect CIS
→ Remove CIG
→ Disconnect ACL
→ clear controller privacy/FAL
→ Control Idle
```

`ASE RELEASING` is the transport-retirement pivot. Normal Stop does not need to wait for final
Idle readback before retiring ISO path/CIS.

ASCS Control Point transport remains ATT Write Command / Write Without Response.

### 2.2 Explicit BE200 workaround

```text
TeardownPolicy::Graceful
TeardownPolicy::ControllerCleanUngraceful
```

The workaround deliberately skips peer-facing ASCS Release:

```text
route
→ ISO path
→ CIS
→ CIG
→ ACL
→ privacy
```

CLI selection is explicit:

```powershell
--ungraceful-teardown
```

It must not be auto-selected by generic Control and must never be reported as peer graceful Release
success.

Implementation seam:

```text
CoreCommand::StopControl { policy }
→ ControlIntent::Stop { policy }
→ ControlMachine::begin_teardown_with_policy(policy)
```

Authoritative design document:

```text
docs/design/BE200_TEARDOWN_POLICY.md
```

Representative commits:

```text
24565865  control: make graceful teardown the default policy
80b15e0d  control: define explicit teardown policy
7111ee66  core: thread teardown policy through StopControl
5fe5dbb0  review(round8): route live relay teardown policy
7a9fa326  cli: report explicit teardown policy
2cbb57d1  docs(round11): register teardown policy decision
```

---

## 3. Minimal GUI and live gain

Round11 added a deliberately small egui product surface rather than a broad settings application.

Current GUI scope:

```text
Volume slider
Start
Stop
```

No tray, settings matrix, log viewer, device manager, or large product shell was introduced.

The gain path is runtime-controlled and applied to Float32 stereo PCM before LC3 encoding.
The UI 0–100% value maps to a linear gain factor.

Representative commits:

```text
1914c05d  media: add runtime live gain control
1840c48d  gui: add minimal egui relay controls
```

The GUI session still reuses the current Round8 live/current-path orchestration rather than a final
L0-composed product runtime.

---

## 4. Extreme-low-latency profile and peer QoS evidence

Round11 made stream latency policy explicit and exposed an opt-in GUI/CLI extreme-low-latency
profile.

Buds3 Pro reports, on both ears:

```text
preferred_framing                 = 0
preferred_phy                     = 2
preferred_rtn                     = 13
preferred_max_transport_latency   = 95 ms
supported_presentation_delay      = 40000..40000 us
preferred_presentation_delay      = 40000..40000 us
```

The sink therefore fixes Presentation Delay at 40 ms. Attempts to force a lower PD are not a valid
product path for this peer.

Controller-selected real HIL timing established:

```text
RTN 0  → transport_c_to_p ≈ 1458 us, NSE=1, BN=1, FT=1
RTN 1  → transport_c_to_p ≈ 3066 us, NSE=2
RTN 2  → transport_c_to_p ≈ 4674 us, NSE=3
```

Each additional RTN step in this observed geometry adds roughly 1.608 ms.

Current extreme-low-latency profile uses:

```text
PHY            2M
SDU interval   10 ms
RTN            2
NSE            3
FT             1
C→P transport  4.674 ms
PD             40 ms
```

The two CISes expose a stable 804 us difference in `CIS_Sync_Delay`. This is recorded as controller
scheduling geometry, not proof of the stereo-image defect.

Representative commits:

```text
c6ff23a6  relay: carry explicit stream latency profile
0bbc723a  relay: keep latency profile scoped to live GUI session
9e13307b  gui: add explicit extreme low latency profile flag
daa46dc6  control: log peer CodecConfigured QoS preferences
aadc948c  hci: log controller-selected CIS timing
c674d683  control: raise low-latency profile to RTN 2
6b21e07b  test(control): expect RTN 2 in low-latency QoS
```

---

## 5. Process Loopback latency characterization

The live source remains Windows Process Loopback at 48 kHz Float32 stereo.

Observed adapter facts:

```text
sample rate     48000 Hz
buffer frames   480
buffer duration 10.000 ms
GetStreamLatency       E_NOTIMPL
GetDevicePeriod        E_NOTIMPL
```

Temporary QPC instrumentation measured approximately:

```text
first-frame age  ≈ 10.4–11.9 ms
packet span      ≈ 10 ms
tail age         ≈ 0.4–1.9 ms
```

The useful interpretation is that there was no hidden extra tens-of-milliseconds source buffer in
the observed Process Loopback path.

The temporary QPC diagnostic was then removed:

```text
54efd82b  windows: log process loopback engine latency
68e37a2d  windows: measure process loopback packet age from QPC
a8a02281  windows: remove temporary loopback QPC telemetry
```

A nominal scheduled latency budget, not an acoustic end-to-end measurement, was estimated as:

```text
Process Loopback average sample age   ~6 ms
PCM retained cushion                 10 ms
10 ms LC3 frame phase average         ~5 ms
CIS transport @ RTN2               4.674 ms
small encode/thread overhead          unmeasured
Presentation Delay                   40 ms
-------------------------------------------
nominal scheduled total             ~65.6 ms
```

Do not compare this number directly with the historical microphone-loopback physical E2E numbers
without porting the same acoustic probe.

---

## 6. Long-running live path evidence and remaining clock-discipline work

A long GUI/live session remained usable for at least an hour by user observation. The pasted timing
excerpt directly covered frame 219546 on a 10 ms grid, i.e. about 2195 s / 36.6 minutes.

During long playback the important positive evidence was:

```text
stale_frames = 0 throughout the excerpt
bridge backlog did not accumulate
media time did not wait for controller completion
```

Rare events included:

```text
underflow_frames=480
source_skipped / bridge_skipped
drift_emergency=480
```

The current provisional drift guard therefore remains functional as bounded correction, but it is
still a coarse bang-bang policy rather than continuous clock discipline.

### ASRC remains required

The planned steady-state architecture remains:

```text
Windows capture clock
→ small bounded source ring
→ occupancy-controlled ASRC
→ exact 480 samples / 10 ms on LocalStreamClock
→ LC3
```

ASRC is not required to prove the existing protocol/data path, but it is the next major quality and
latency hardening item for continuous daily use.

---

## 7. Spontaneous hard session failure discovered

After a long apparently healthy session, the GUI path produced two consecutive spontaneous hard
failures, including a recurrence roughly five minutes after reconnect.

Visible failure:

```text
audio/CIS hard cut
...
BE200AudioHub GUI session failed: HCI report lane disconnected
```

### 7.1 What current code proves

`HCI report lane disconnected` means the receive side observed the HCI report sender disappear.

The report sender is owned by the HCI Host worker. Therefore the visible error implies that the HCI
worker had already terminated before Stop later discovered the disconnected report lane.

Potential terminal worker exits include:

```text
Engine(fault)
ReportFull
ReportDisconnected
ControlDisconnected
ControllerDisconnected
IsoDataDisconnected
```

Current GUI/live shutdown discards the returned `HciStackExit`:

```text
let _exit = stack.shutdown();
```

Therefore the first-order worker fault is currently hidden and the UI only reports the secondary
report-lane disconnect.

### 7.2 Structural risk found during code audit

Steady-state GUI waiting is currently approximately:

```text
while !stop_requested {
    sleep(10 ms)
}
```

It does not continuously drain the HCI semantic report inbox. The report inbox is bounded
(`report_inbox_capacity = 64`), and `HciWorkerFault::ReportFull` is terminal.

This is a real architectural risk but is **not yet proven** to be the cause of the observed hard
failure.

### 7.3 Lifecycle implication

A product supervisor must observe worker termination immediately and decide recovery rather than
leaving a dead worker undetected until user Stop.

This is now the strongest concrete acceptance case for the pending L0 Lifecycle work.

---

## 8. Persistent stereo-image anomaly and packet-loss correlation

The same subjective anomaly previously observed under Windows native LE Audio also occurs under the
custom BE200 userspace Host:

```text
normal image
→ sudden audible packet-loss seam
→ wide / hollow / phase-like image
→ stable bad state for tens of seconds or minutes
→ another audible loss seam
→ image returns to normal
```

It can repeat several times in one session.

This materially weakens a Windows-Audio-Host-only explanation.

The current project evidence supports:

> the bad↔good state transitions are strongly correlated with audible packet-loss/recovery events.

It does **not** yet prove:

- exactly one-SDU left/right offset;
- whether the persistent state is created in Host→Controller ISO scheduling, controller ISOAL/LL
  recovery, or Buds3 Pro presentation/TWS recovery;
- whether HCI ISO timestamps alone solve the defect.

Useful future probes discussed in Round11:

```text
HCI_LE_Read_ISO_Link_Quality
HCI_LE_Read_ISO_TX_Sync
timestamped Host→Controller HCI ISO
RTN sweep
mono-identical / impulse external measurement
```

A desirable product property is stronger than merely reducing loss incidence:

> an unrecovered asymmetric ISO loss must not leave the user indefinitely in an unknown stereo
> timing state; if synchronization cannot be proven, invalidate/rebuild the stereo epoch.

No implementation for this recovery policy landed in Round11.

---

## 9. Lifecycle and Condition audit

Round11 then audited the current V2 Core before starting Lifecycle implementation.

### 9.1 Lifecycle vocabulary exists, orchestration does not

Current whole-runtime states:

```text
Stopped
Starting
Waiting
Running
Recovering
Stopping
```

Current session states:

```text
Idle
Preparing
Priming
Streaming
Draining
TearingDown
```

Recovery classes already exist:

```text
RestartMedia
RebuildStream
RestartController
RestartWorker
```

But `LifecycleMachine` remains intentionally minimal. It owns a
`LifecycleModel` and exposes `replace_model()`; there is no product startup/recovery transition
machinery yet.

### 9.2 Condition read model is structurally complete

The private Blackboard contains:

```text
epoch
generation
lifecycle
control
hci
media
transport
```

Public reduction APIs exist:

```text
apply_lifecycle()
apply_control()
apply_hci()
apply_media()
apply_transport()
```

Scope fencing already rejects stale Generation/Session updates.

### 9.3 Current live wiring is incomplete

The current CurrentPath driver initializes Lifecycle once as approximately:

```text
Lifecycle = Running
Session   = Preparing
```

and then continuously applies Control updates.

HCI, Media, and Transport already define Condition-update contracts, but the current product/live
composition does not yet feed all of them into the single Core Condition owner.

Therefore Condition is not yet a trustworthy whole-runtime factual surface during live playback.

---

## 10. Telemetry architecture audit

The frozen V2 telemetry architecture remains three separate lanes:

```text
Reliable semantic lane
  state changes / faults / lifecycle facts
  bounded, nonblocking, mailbox-full = fault

Aggregated telemetry lane
  producer-local O(1) counters/gauges
  dirty/coalesced publication to Condition

Raw diagnostic lane
  bounded producer-local recent-history ring
  lossy by explicit overwrite policy
  never protocol truth
```

Generic default raw-ring policy:

```text
fixed capacity
→ overwrite oldest
→ accept newest
→ count overwritten_oldest
→ never block
→ memory only by default
```

### 10.1 Existing HCI telemetry

HCI ISO already owns a 256-record raw ring and aggregate counters for:

```text
Configured
Submitted
Completed
Retired
controller_limit
submitted/completed/retired
in_flight / peak_in_flight
```

### 10.2 Existing Media Bridge telemetry

Media Bridge already owns a 256-record raw ring for:

```text
Submitted
DroppedBackpressure
fallback intervals
skipped intervals
stale-frame drops
```

### 10.3 Transport telemetry landed at end of Round11

Round11 added the previously-empty L2 Transport producer-local telemetry implementation.

Commits:

```text
26cd6322  transport: add producer-local telemetry trace
8d27d5ca  transport: timestamp raw telemetry events
```

Current Transport aggregate:

```text
rx_packets
tx_packets
faults
dirty
```

Current 256-record raw trace:

```text
TxSubmitted
ReadCompleted
WriteCompleted
Cancelled
DeviceGone
TransferFailed
Fault
```

Raw records carry producer-local monotonic `at_us`, TransferId, endpoint and relevant byte/packet
counts. This gives future fault forensics a bounded USB/backend event history without putting raw
packet-rate diagnostics into Core Condition.

### 10.4 WASAPI capture decision

No raw telemetry ring is planned for Process Loopback capture at this stage.

Reason:

- the same capture path has already seen extensive daily use in LE-Audio-Relay;
- there is no current evidence that capture stability is the failing boundary;
- forcing the generic owned `TelemetryRing` across the capture callback boundary would encourage
  unnecessary locking/complexity.

Current atomic capture counters remain sufficient. Revisit only when evidence points to capture.

### 10.5 Media RT anomaly ring approved but not landed

The next producer-local ring should record **only anomalous Media intervals**, not normal heartbeat
or every scheduler wait.

Planned record shape:

```text
MediaTimingAnomaly {
    frame_id
    at_us
    pcm_fill_frames
    underflow_frames
    source_intervals_skipped
    bridge_intervals_skipped
    stale_frames_dropped
    startup_trim_frames
    gradual_drift_trim_frames
    emergency_drift_trim_frames
}
```

Default capacity should remain 256 / overwrite-oldest.

This work was approved at the end of Round11 but did not land before conversation handoff.

---

## 11. Condition mailbox boundary at Round11 close

One important correction from the final audit:

> Condition has apply/reducer APIs and generic mailbox contracts, but the final Plane→Condition
> runtime mailbox is not yet composed.

The generic contracts already exist:

```text
RELIABLE_MAILBOX
  ReliableBounded
  overflow = Fault
  producer nonblocking

TELEMETRY_MAILBOX
  CoalescedLatest
  overflow = CoalesceLatest
  producer nonblocking
```

The intended runtime flow for the next round is:

```text
state-changing Plane fact
→ reliable bounded Condition lane
→ immediate Condition apply

dirty aggregate telemetry
→ coalesced/latest Condition lane
→ bounded cadence Condition apply

fault / teardown
→ forced-final Plane update
→ Condition apply
```

Raw trace vectors remain producer-local and never enter Condition.

---

## 12. Final GitHub code audit

Round11 closure audited remote GitHub `main` read-only before writing this document.

Code checkpoint:

```text
8d27d5ca61486bfe5c1bfdb7d51349bd51156110
transport: timestamp raw telemetry events
```

Compared with the Round10 handoff checkpoint
`773a2f2aaee9e3acac1de5494af1e23969564ce0`:

```text
ahead_by = 53 commits
behind_by = 0
```

Material active-code additions/changes in that range include:

```text
explicit teardown policy
minimal egui GUI
live gain
latency profile / peer QoS logging
controller-selected CIS timing logging
Process Loopback latency probes
Transport producer-local telemetry
README/product documentation refresh
```

The former V2 refactor branch was merged by fast-forward into `main`; `main` is the current
authoritative product branch.

### Software gate evidence

The latest test run tied to the code checkpoint `8d27d5ca`:

```text
cargo test --workspace --locked
420 passed / 0 failed
```

A full `cargo check` + `cargo build` gate was not rerun after the final two Transport telemetry
commits during this conversation. The immediately preceding full product checkpoint had:

```text
416/416 tests PASS
cargo check PASS
cargo build PASS
```

Do not rewrite the current head as check/build-verified until those commands are rerun.

This closure audit is about remote GitHub state. It does not claim a fresh local working-tree audit.

---

## 13. Next-round implementation order

Start from repository state, not from conversational memory.

### Step 1 — Media RT anomaly ring

Implement the approved 256-record anomaly-only Media trace.

Do not add a WASAPI raw ring.

Do not duplicate existing Bridge Submitted/Backpressure trace records.

### Step 2 — Plane→Condition publication plumbing

Compose the real runtime path for:

```text
Control   → Condition   already functionally applied in current path
HCI       → Condition
Media     → Condition
Transport → Condition
```

Rules:

- semantic state/fault = reliable bounded;
- telemetry = dirty/coalesced;
- fault/teardown = forced final;
- raw trace stays local.

### Step 3 — Make Condition a truthful live read surface

Only after all Plane projections are live should Lifecycle consume Condition.

### Step 4 — Compose L0 Lifecycle supervisor

First acceptance case:

```text
HCI worker dies
→ fault becomes observable immediately
→ Condition reflects fault
→ Lifecycle Running → Recovering
→ bounded recovery/teardown policy
```

Do not allow the current behavior where a dead worker is discovered only when Stop later touches a
disconnected report lane.

### Step 5 — subsequent product hardening

After Lifecycle baseline:

```text
ASRC / continuous clock discipline
Media control input (MCS/GMCS/VCS/HOGP discovery first)
ISO Link Quality / TX Sync research
timestamped ISO experiment
stereo re-epoch recovery policy
full current-head check/build + real HIL
```

---

## 14. Claim boundary

Allowed:

> Round11 froze graceful teardown as the architecture default and made the BE200 controller-clean
> ungraceful path an explicit opt-in workaround.

Allowed:

> The current live Windows→LC3→HCI ISO→BE200→Buds3 Pro data path can sustain long playback, with
> occasional PCM underflow/drift corrections and without observed stale-media backlog in the pasted
> long-run evidence.

Allowed:

> A spontaneous hard session failure was observed twice; current code proves the HCI report sender
> disappeared because the HCI worker terminated, but the first-order worker fault is currently
> discarded and the root cause remains open.

Allowed:

> The recurring stereo-image anomaly also occurs under the custom Host and its bad↔good transitions
> are subjectively associated with audible packet-loss seams.

Not allowed:

> The stereo-image defect is proven to be exactly one SDU of interaural skew.

Not allowed:

> HCI ISO timestamps have been proven to fix the stereo-image defect.

Not allowed:

> The final L0 Lifecycle / supervisor is implemented.

Not allowed:

> HCI/Media/Transport are already fully wired into Core Condition in the live product runtime.

Not allowed:

> The current `8d27d5ca` head has a fresh cargo check/build gate; only 420/420 tests are current to
> that exact code checkpoint.

---

## 15. Round11 closure checkpoint

```text
authoritative code checkpoint before docs-only close:
8d27d5ca61486bfe5c1bfdb7d51349bd51156110

branch:
main

latest current-head software evidence:
cargo test --workspace --locked
420 passed / 0 failed

current product state:
live audio path operational
minimal GUI operational
explicit graceful/ungraceful teardown policy
extreme-low-latency RTN2 profile operational
Transport producer telemetry ring landed

open immediate work:
Media RT anomaly ring
Plane→Condition publication
truthful whole-runtime Condition
L0 Lifecycle supervisor/recovery
```

Round12 / successor conversation should begin from this document and the actual repository head.
