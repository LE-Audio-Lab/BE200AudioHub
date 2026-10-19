# BE200AudioHub — Round10 / Graceful-Teardown Investigation Closure, Code Audit, and Deprecation Handoff

> **Status**: **CODE CLEANUP COMPLETE / ARCHITECTURE DECISION DEFERRED**
>
> **Branch**: `refactor/v2-architecture`
>
> **Hardware**: Intel BE200 + Samsung Galaxy Buds3 Pro
>
> **Conversation continuity warning**: this Round10 document is the authoritative handoff for the
> conversation that performed the final Round9 ACL-RX experiments and the Round10 deprecation
> audit. The next chat must read this file rather than relying on implicit conversation memory.
>
> **Important scope rule**: this document records evidence, code state, cleanup, and open choices.
> It intentionally does **not** freeze the final architecture decision for normal product teardown.
> That decision is deferred to the next conversation.

---

## 1. Why Round10 exists

Round9 started as a normal long-session graceful-teardown problem:

```text
audio/data path healthy for a long run
→ StopControl
→ ASCS Release
→ CleanupAse timeout
```

By the end of the investigation, that framing had become misleading. The Release timeout was only
the first visible consumer of a deeper failure: after a repeatable runtime window, both peer-facing
ACL/ATT data paths stopped producing Controller→Host HCI ACL data even though several lower control
signals remained alive.

Round10 therefore had two goals:

1. spend one final, bounded root-cause budget to determine whether the failure was caused by our
   own Host/transport implementation;
2. audit and remove the large amount of investigation-only code added after the first Release
   timeout, preserving only evidence and intentional active-code deltas.

The key engineering principle for this round was:

```text
keep evidence in docs/archive
keep product code small
do not retain falsified experiments as dormant architecture
```

---

## 2. Starting boundary

The investigation-code audit used this explicit boundary:

```text
pre-investigation baseline:
a11efdffadd0bf319d53ab09f362116d11945030
review: add longevity runtime snapshots

first post-timeout code change:
96cf90a5
control: pivot graceful release on ASE releasing state

full pre-cleanup investigation tree:
941092e317561f3bc13c024600bd87efcc87671c
review(round9): add ACL bulk-IN depth-two A/B
```

The range `96cf90a5..941092e3` contains 45 commits. Three are timeline/docs commits; the remainder are
code, test, diagnostic, or HIL-harness changes.

The complete tracked source tree at `941092e3` was archived before cleanup:

```text
deprecated/graceful-teardown-bug/
  BE200AudioHub-graceful-teardown-bug-941092e31756.zip
  README.md
```

Archive commit:

```text
b864cf195d75f6514fce70c70e5c0bc52bef23a6
archive(round10): preserve graceful teardown investigation snapshot
```

---

## 3. Final root-cause budget: three-model plan

Before deciding to stop digging, the investigation was deliberately limited to three final models.

### Model A — dual ACL, no CIG/CIS/ISO

Purpose:

> Can the two encrypted LE ACL + GATT/ATT paths remain healthy for 60 s when no isochronous
> resources are ever created?

Topology:

```text
dual encrypted ACL
→ PACS/ASCS/GATT
→ both ASEs CodecConfigured
→ STOP setup here
→ NO CIG
→ NO CIS
→ NO ISO Data Path
→ NO media
```

The model probed both ears every 5 s using ordinary ASE reads and captured negotiated ACL runtime
state.

### Model B — dual ACL + one CIS

Purpose:

> Is any CIS sufficient to trigger the failure, or is dual-CIS coexistence required?

This model kept both ACLs but built only the Right-side QoS/Enable/CIS/InputPath/Streaming chain.

### Model C — dual CIS with lower scheduler pressure

Purpose:

> If dual CIS is required, is the failure sensitive to retransmission/transport-latency policy?

The proposed comparison kept the same 48 kHz / 10 ms / 100 B / 2M topology while changing the
retransmission/latency budget.

Implementation commits for this final-model framework included:

```text
e48257d4  review(round9): add final-model Control policies
e9e53707  fix(round9): keep semantic CIG id in final-model mismatch
9e49c21e  test(round9): lock final-model Control invariants
bf73b1f2  test(round9): drive real codec state before single-CIS split
5752f387  diag(round9): expose live CIS handles for final models
1db74aa4  review(round9): drive final models to explicit checkpoints
a23876ae  review(round9): add three final HIL models
da5e2604  review(round9): wire final HIL model CLI
79469f7b  review(round9): make final-model abort cleanup controller-clean
3e27c024  fix(round9): close final-model setup match
```

The important result is that **Model A alone was decisive**, so Models B/C were not needed.

---

## 4. Model A real HIL: CIS/ISO was not a necessary trigger

Model A was run with:

```text
dual encrypted ACL
dual GATT/ASCS
both ASEs CodecConfigured
no CIG
no CIS
no ISO
no media
```

Observed timeline:

```text
pre-run  Right PASS
pre-run  Left  PASS

t=5 s    both PASS
t=10 s   both PASS
t=15 s   both PASS
t=20 s   both PASS

t=25 s   Right ATT read timeout
t=25 s   Left  ATT read timeout
```

At failure, for both ears:

```text
Host GATT request enqueue         PASS
Bumble ACL submit                 PASS
Host ACL accounting              PASS
L2 mailbox                       PASS
USB bulk OUT submit              PASS
USB bulk OUT completion          PASS
controller NCP completion        PASS

Controller→Host ACL RX           DEAD
ATT response                     DEAD
```

At the same time:

```text
HCI Event-IN                      LIVE
connection handles               LIVE
Read RSSI                        LIVE
LE Read Remote Features          LIVE
```

Representative failure evidence:

```text
Right controller RSSI: alive conn=0x0800/g1
Left  controller RSSI: alive conn=0x0801/g2

Right LL Remote Features: alive
Left  LL Remote Features: alive
```

Negotiated connection state remained unchanged around the transition:

```text
interval=36
latency=0
supervision=42
subrate=1
continuation=0
phy=1/1
data_len=251/2120/251/2120
encryption enabled
```

### Model A consequence

This falsified the hypothesis:

```text
CIS / ISO is required to trigger the ACL-data failure
```

Therefore the original dual-CIS / ISO-scheduler explanation could no longer be the primary model.
Models B and C no longer had sufficient expected information value and were not run.

---

## 5. Controller→Host flow-control sanitize A/B

The Model A trace had a striking shape:

```text
Host→controller ACL works
HCI Event-IN works
LL/control view works
Controller→Host HCI ACL data stops
```

One plausible explanation was stale Controller→Host HCI flow-control state inherited through the
project's resetless attach model.

A Review/HIL A/B was added:

```text
HCI Set Controller To Host Flow Control = OFF
before any connection is created
```

Relevant commits:

```text
17b4b969  review(round9): A/B sanitize controller RX flow control on attach
342c1b6c  test(round9): preserve default HCI worker attach wrapper
a5da8322  review(round9): allow RX flow-control sanitize in final models
```

### HIL result

With sanitize enabled, Model A reproduced the same failure:

```text
t=20 s  both ATT paths responsive
t=25 s  both ATT paths dead
```

USB ACL-IN completion stopped at effectively the same boundary while Event-IN continued.

### Conclusion

Falsified:

```text
stale inherited Controller→Host flow-control enable state
```

This also removed the strongest reason to keep the attach-policy A/B plumbing in active code.

---

## 6. Last transport hypothesis: ACL Bulk-IN outstanding-read depth

Linux `btusb` normally keeps multiple bulk-IN URBs queued, while the project's Windows libusb
backend originally allowed only one outstanding read per RX endpoint.

This created a final high-value hypothesis:

```text
depth=1 leaves a completion→re-arm gap
→ BE200 USB/DMA producer may enter an edge case
→ ACL Bulk-IN producer wedges
```

A clean A/B was implemented:

```text
baseline:
ACL Bulk-IN outstanding depth = 1

experiment:
ACL Bulk-IN outstanding depth = 2

all GATT/setup/timing/probe behavior unchanged
```

Implementation:

```text
404e4738  transport(round9): make ACL bulk-IN queue depth explicit
941092e3  review(round9): add ACL bulk-IN depth-two A/B
```

Ownership was deliberately correct for the experiment:

```text
Backend:
  permit multiple pending reads for one RX endpoint

L2 TransportReactor:
  own the desired ACL RX queue depth

Product/default:
  depth = 1

Review A/B:
  depth = 2
```

### HIL result

Depth two was genuinely active:

```text
event_usb_read ... pending:1
acl_usb_read   ... pending:2
```

It remained at two outstanding reads throughout the healthy interval.

At t=20 s:

```text
ACL read arms=78
ACL read completions=76
pending=2
both ATT paths responsive
```

At t=25 s:

```text
ACL read arms=78
ACL read completions=76
pending=2
both ATT paths timeout
```

Both pending libusb reads remained armed; the controller simply stopped completing ACL Bulk-IN
traffic.

RSSI and LL Remote Features remained alive on both links.

### Conclusion

Falsified:

```text
single outstanding ACL Bulk-IN read / missed continuous USB queueing
```

The project explicitly stopped root-cause digging after this experiment.

---

## 7. Final unresolved fault boundary

The strongest evidence that survived the falsification tree is:

```text
two encrypted LE ACL links remain logically alive

Host → Controller:
  ACL TX                       LIVE
  USB bulk OUT                LIVE
  NCP completion              LIVE

Controller → Host:
  HCI Event interrupt-IN      LIVE
  RSSI/control command result LIVE
  LL Remote Features result   LIVE
  HCI ACL bulk-IN data        DEAD

peer-facing upper data:
  ATT                         DEAD
  GATT                        DEAD
```

The failure happens in a sampled window approximately:

```text
(20 s, 25 s]
```

Do **not** rewrite this as an exact 25.000 s transition.

Both ACL data bearers fail together, while two connection handles and lower control operations
remain visible.

The investigation did **not** identify the actual root cause. Remaining possibilities include a
BE200 firmware/controller internal state, vendor-specific controller initialization omitted by the
userspace Host, a controller/peer interoperability condition, or another lower-level behavior not
worth further product-path investigation at this point.

The correct claim boundary is:

> **Unresolved controller-global Host-facing HCI ACL RX cessation.**

It is not valid to claim that Intel firmware, the Buds3 Pro, USB DMA, or the project's Host code has
been proven as the root cause.

---

## 8. Why AOSP/BlueZ do not invalidate the finding

AOSP and BlueZ normally support graceful ASCS Release; therefore the failure cannot be dismissed as
“mature stacks simply never perform graceful teardown”.

However their lower stack ownership is materially different.

Typical BlueZ path:

```text
bluetoothd / BAP
→ kernel Bluetooth HCI core
→ btusb
→ btintel/vendor handling
→ USB/controller
```

Typical Android product path:

```text
LE Audio state machine
→ Bluetooth HCI HAL
→ vendor controller transport/init
→ controller
```

BE200AudioHub directly owns much more of this path in userspace:

```text
Control/GATT/ATT
→ Bumble Host
→ HCI adapter
→ L2 TransportReactor
→ libusb async backend
→ BE200
```

Round10 did not continue into vendor-firmware archaeology after the depth-two experiment.

---

## 9. Round9 post-timeout code audit

Once investigation stopped, the project audited every commit after the first Release-timeout
response.

Audit boundary:

```text
a11efdff → 941092e3
45 commits
34 files changed in the final head-to-tail diff
```

Before cleanup, the largest net growth areas were:

```text
src/review/round8/current_path.rs   +1561 / -20
src/review/round8/live_relay.rs     +1174 / -18
src/plane/l1_hci/host/engine.rs      +671 / -15
Control rollback / tests             hundreds of lines
```

The important architectural observation was that Round9's forensic work had started leaking across
the V2 ownership boundaries. For example, L2 USB read lifecycle counters were being surfaced through
an L1 ACL diagnostic snapshot. That was useful during investigation but inappropriate as permanent
product architecture.

### Audit classification

#### Retain as protocol/product-relevant code behavior

- ASCS Control Point operations use ATT Write Command / Write Without Response.
- Generic ASCS Release support still exists and may be used by failure rollback.
- The current active product Stop implementation skips peer ASCS Release and retires controller
  resources directly.

The last item is a **current code-state fact**, not a final architecture decision in this document.

#### Deprecate as investigation scaffolding

- Release gate snapshot / timeout diagnostics;
- teardown ACL queue/RX boundary/packet-lineage audits;
- Release-timeout ASE readback probes;
- group Release A/B mode;
- Controller→Host flow-control sanitize A/B;
- runtime ATT liveness / periodic keepalive probes;
- USB ACL-IN lifecycle forensic telemetry;
- RSSI and peer LL Remote Features diagnostic probes;
- opposite-ear failure-scope probe;
- ACL negotiated-runtime snapshot;
- `ReviewStreamProfile` and final-model Control branches;
- CIS runtime snapshot;
- final-model HIL runner / synthetic pressure / checkpoint / abort harness;
- ACL Bulk-IN depth-two generalization and A/B;
- cross-layer `TransportTxAudit` plumbing.

The full pre-cleanup implementation remains archived and recoverable.

---

## 10. Deprecation archive

Before removing investigation code, the user created a complete tracked-tree snapshot using
`git archive HEAD`.

Snapshot metadata:

```text
source commit:
941092e317561f3bc13c024600bd87efcc87671c

archive commit:
b864cf195d75f6514fce70c70e5c0bc52bef23a6

file:
deprecated/graceful-teardown-bug/
BE200AudioHub-graceful-teardown-bug-941092e31756.zip
```

The archive is the recovery point for any future need to reconstruct the Round9 forensic harness.

---

## 11. Round10 cleanup execution

The cleanup was performed surgically from HEAD, not by reverting the entire Round9 range, because
valid protocol changes and forensic code were interleaved.

Primary cleanup commit:

```text
ca12d763170a01218a99b2bf79bfde517981b83a
cleanup(round10): retire graceful-teardown investigation code

+183 / -5482
```

This removed the large Review/HIL and cross-layer diagnostic tree from active source.

Follow-up:

```text
8176e635b0ab4d51debf8aa1e3c8d1d5ab5c4e4a
cleanup(round10): remove obsolete transport forensic audit

removed:
src/plane/l2_transport/tx_audit.rs
-101 lines
```

Deprecated archive index commit:

```text
773a2f2aaee9e3acac1de5494af1e23969564ce0
docs(round10): index deprecated graceful teardown investigation
```

### Post-cleanup net code delta

Comparing active `src/` against the pre-investigation baseline `a11efdff` now leaves only:

```text
src/plane/l1_control/ascs/operation.rs
  +7 / -7

src/plane/l1_control/engine/machine/rollback_flow.rs
  +12 / -28

src/plane/l1_control/engine/machine/tests.rs
  +23 / -52
```

No Round9 forensic runtime module remains active.

---

## 12. Post-cleanup software gate

After cleanup and before this documentation commit:

```text
cargo test --workspace --locked
408 passed / 0 failed

cargo check --workspace --locked
PASS

cargo build --workspace --locked
PASS

git status
clean / synced
```

Warnings remain from pre-existing dead/unused surfaces and are not new Round10 cleanup failures.

---

## 13. Current active-code state — not a final architecture decision

The next chat must distinguish these two concepts:

### Current code behavior

At the end of this conversation, active code currently has:

```text
ASCS Control Point
→ WriteCommand / Write Without Response
```

and normal product Stop currently executes a controller-resource teardown without waiting for peer
ASCS Release completion.

### Architecture decision status

**Intentionally deferred.**

This Round10 document does not freeze that teardown policy as the final architecture contract.

The next conversation must explicitly decide whether to:

- formalize the current behavior as the product default;
- add a conditional graceful-attempt/fallback policy;
- or revise the lifecycle contract in another direction.

Only after that explicit decision should `docs/design/` receive a final architecture statement.

---

## 14. Separate open issue: abnormal Host death / dirty controller

Do not conflate the graceful-teardown investigation with the earlier Round9 crash-recovery defect.

Observed separately:

```text
Host process killed / Ctrl+C
→ controller/peer state may survive
→ next resetless attach can fail member visibility
→ BE200 power-cycle restores operation
```

Round10 cleanup did not solve the durable CLEAN/DIRTY ownership problem.

The prior direction remains available for future work:

```text
known CLEAN generation
→ resetless attach

DIRTY / UNKNOWN generation
→ HCI Reset
→ full Host/controller reinitialization
```

But this is not closed by the teardown-code cleanup.

---

## 15. Commit map for the deprecated investigation

Important investigation commits preserved in git/archive:

```text
96cf90a5  graceful Release RELEASING pivot
41742199  Release gate diagnostics
85300c88  ASCS Control Point WriteCommand
d1fb532c  ACL queue audit
dd7c3e45  ACL RX boundary
d85a1d73  group Release A/B
79750f68  USB/NCP ACL lineage
8b15f561  Release-timeout ASE readback
17b4b969  RX flow-control sanitize A/B
70d3c5e2  ATT liveness probe
6f8f769b  periodic ATT keepalive
a34c0ffe  USB ACL-IN lifecycle
f6d5273e  RSSI probe
5196ef0e  peer LL feature probe
4078f0b2  opposite-ear probe
d8f6d8a6  ACL runtime snapshot
e48257d4  final-model Control profiles
5752f387  CIS runtime snapshot
a23876ae  final HIL models
a5da8322  flow-control sanitize in final model
5b4b44e6  current ungraceful Stop implementation
404e4738  explicit ACL Bulk-IN queue depth
941092e3  depth-two A/B harness
```

Use the archived ZIP when code archaeology is easier than walking these commits individually.

---

## 16. Next-chat handoff

Start from the repository, not from a compressed conversational summary.

At the end of this conversation:

```text
branch:
refactor/v2-architecture

pre-doc HEAD:
773a2f2aaee9e3acac1de5494af1e23969564ce0

tracked working tree:
clean

post-cleanup gates:
408/0 tests
check PASS
build PASS
```

The next chat should first:

1. verify remote HEAD and local `git status`;
2. read this `Round10.md`;
3. read `deprecated/graceful-teardown-bug/README.md`;
4. inspect the three-file post-cleanup delta against `a11efdff`;
5. **make and document the final teardown architecture decision**;
6. only then update design docs and close/reclassify Round9/Round10 status.

Do not reintroduce the deprecated forensic harness unless new contradictory HIL evidence appears.

Do not claim the controller-global ACL-RX root cause was solved.

---

## 17. Claim boundary

Allowed:

> The Round9 investigation localized a repeatable dual-ACL Host-facing RX cessation and falsified
> several Host-side hypotheses, including CIS/ISO necessity, stale Controller→Host flow-control
> state, USB ACL-IN missed re-arm, and one-vs-two outstanding Bulk-IN reads.

Allowed:

> Round10 archived and removed the investigation-only runtime code, returning active source almost
> exactly to the pre-investigation baseline while preserving the verified ASCS WNR behavior and the
> current teardown implementation.

Not allowed:

> Intel firmware was proven to be the root cause.

Not allowed:

> The teardown architecture has been finally frozen by this Round10 conversation.

Not allowed:

> The dirty-controller crash-recovery problem is solved.
