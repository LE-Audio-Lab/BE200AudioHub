# BE200 Teardown Policy and Known ACL-RX Defect

Status: **Round11 architecture decision / active**

This document freezes the normal product teardown contract after the Round9/10
investigation. It deliberately separates the Bluetooth/LE Audio architecture
from a BE200-specific runtime workaround.

## 1. Architecture default

The architecture remains protocol-graceful by default.

Normal product Stop:

```text
stop Media production
→ stop Bridge submissions / drain admitted ISO traffic
→ unbind logical stream route
→ ASCS Release
→ confirm Control Point SUCCESS + ASE RELEASING
→ Remove ISO Data Path
→ Disconnect CIS
→ Remove CIG
→ Disconnect ACL
→ clear controller privacy/FAL
→ Control Idle
```

`ASE RELEASING` is the transport-retirement pivot. Normal Stop does not need
to wait for a final IDLE readback before removing the ISO path/CIS.

ASCS Control Point writes use ATT Write Command / Write Without Response. The
semantic success gate remains peer Control Point SUCCESS plus the expected ASE
notification.

## 2. Known BE200-path defect

Round10 established a repeatable defect on the current Intel BE200 userspace
Host path:

```text
two encrypted LE ACL links remain logically alive

Host → Controller ACL TX       LIVE
USB bulk OUT                   LIVE
NCP completion                 LIVE

HCI Event interrupt-IN        LIVE
RSSI/control command results  LIVE
LL Remote Features results    LIVE

Controller → Host HCI ACL RX  STOPS
ATT/GATT                      STOPS
```

The transition was sampled in approximately `(20 s, 25 s]` in the decisive
ACL-only model. No CIG, CIS, ISO Data Path, or media traffic was required.

For product engineering purposes this is treated as a **known BE200
controller-path defect / interoperability quirk**. The exact internal root
cause remains unresolved; this wording does not claim proof of a particular
silicon, firmware, USB-DMA, peer, or Host implementation fault.

Round10 separately falsified the following candidate mechanisms:

- CIS/ISO being required to trigger the failure;
- inherited standard Controller→Host flow-control state;
- a single outstanding ACL Bulk-IN read / completion-to-rearm gap;
- one-versus-two outstanding ACL Bulk-IN reads.

The pre-cleanup forensic implementation remains archived under
`deprecated/graceful-teardown-bug/`.

## 3. Explicit workaround policy

The product exposes a separate teardown policy:

```text
TeardownPolicy::Graceful
TeardownPolicy::ControllerCleanUngraceful
```

`Graceful` is the architectural default.

`ControllerCleanUngraceful` is an explicit hardware workaround:

```text
stop Media production
→ stop Bridge submissions / drain admitted ISO traffic
→ unbind logical stream route
→ Remove ISO Data Path
→ Disconnect CIS
→ Remove CIG
→ Disconnect ACL
→ clear controller privacy/FAL
→ Control Idle
```

It intentionally skips peer-facing ASCS Release. It exists so controller-owned
resources can still be retired when the BE200 Host-facing ACL RX path has
already ceased carrying ATT/GATT responses.

The workaround must never be silently selected by generic Control code.

## 4. CLI contract

Current BE200 HIL/product-path runs that intentionally use the workaround must
state it explicitly:

```powershell
--ungraceful-teardown
```

Without that flag, the current Round8 current-path/live-relay CLI uses the
spec-aligned graceful teardown policy.

This makes logs and HIL evidence self-describing: a successful controller-clean
workaround run must not be mistaken for proof that peer ASCS graceful teardown
completed.

## 5. Failure rollback remains separate

Failure rollback is not the same policy as normal graceful Stop.

The established fail-closed rollback remains:

```text
route
→ ISO path
→ CIS
→ best-effort ASE Release
→ CIG
→ ACL
→ privacy
```

Its purpose is to retire only resources whose state is sufficiently known after
an earlier failure. It must not be rewritten merely to match either normal
Stop policy.

## 6. Non-regression rules

1. Generic Control defaults to `Graceful`.
2. The BE200 workaround requires an explicit policy/CLI choice.
3. Do not auto-fallback from graceful to ungraceful in generic Control.
4. Do not report peer graceful Release success when the workaround skipped it.
5. Do not reintroduce Round9 forensic cross-layer plumbing into product code.
6. Keep ASCS Write Without Response as the normal Control Point transport
   behavior.
7. Keep route/ISO-path/CIS/CIG/ACL ownership and fail-closed cleanup invariants.
8. Abnormal Host death / dirty-controller recovery remains a separate problem.

## 7. Current implementation mapping

The policy seam is carried through:

```text
CLI
→ CoreCommand::StopControl { policy }
→ ControlIntent::Stop { policy }
→ ControlMachine::begin_teardown_with_policy(policy)
```

Only teardown initialization differs:

- `Graceful`: populate the ASE Release queue and gate path/CIS retirement on
  Release reaching RELEASING;
- `ControllerCleanUngraceful`: leave the ASE Release queue empty and proceed
  directly to controller-owned transport cleanup.

The remainder of the teardown executor is shared.

## 8. Claim boundary

Allowed:

> The current BE200 userspace-Host path has a repeatable controller-global
> Host-facing ACL-RX cessation defect, and BE200AudioHub provides an explicit
> controller-clean ungraceful teardown workaround.

Not allowed:

> Intel firmware or BE200 silicon has been proven as the root cause.

Not allowed:

> Ungraceful teardown is the Bluetooth/LE Audio architectural default.

Not allowed:

> A workaround run proves peer ASCS Release completed.
