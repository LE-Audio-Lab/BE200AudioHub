# BE200AudioHub — Round8 / V2 Structural Closure + Live Relay First HIL

> **Status**: **CLOSED**
>
> **Branch**: `refactor/v2-architecture`
>
> **Hardware**: Intel BE200 + Samsung Galaxy Buds3 Pro
>
> **Engineering-round boundary**: Round8 ends at the live-relay / first-longevity baseline. The CLI namespace remains `round8` for current-path and live-relay HIL, but later teardown/recovery work belongs to **Round9**.

---

## 1. Round8 scope

Round8 closed the V2 ownership/structure pass and then used that architecture to
bring the real Windows live-audio product path online.

Frozen ownership stack:

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

Control was decomposed into durable model, operation progress, engine/reducers,
discovery/planning/ASCS and interface boundaries. No new global
RuntimeState/Aggregator was introduced.

---

## 2. First Sound baseline entering Round8

Round7 valid-LC3 First Sound had already passed on real hardware.

Round8 current-path First Sound then passed with the frozen product profile:

```text
48 kHz
10 ms frame
100 B LC3 / ear
2 s / low-level 400 Hz left + 600 Hz right fixture
logical HCI route
reverse teardown PASS
BondStore unchanged
```

This proved the refactored Control/Media/HCI path could still drive the real
BE200 + Buds3 Pro setup before live Windows capture was added.

---

## 3. Windows live relay migration

The legacy Windows relay source side was migrated without carrying the old
destination WASAPI renderer.

Product path:

```text
Windows applications
→ sacrificial physical render endpoint
→ Process Loopback
→ PcmRelayBoundary
→ SPSC Float32 stereo ring
→ LC3
→ StreamClock / IsoTxBridge
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

The first Process Loopback HIL failed with `E_ILLEGAL_METHOD_CALL`: the raw
completion handler did not expose `IAgileObject`. Commit `18cf4e13` fixed
the COM completion handler, followed by the regression in `da16206b`.

Two consecutive 10 s live HIL runs then passed with audible arbitrary Windows
application audio through the custom stack.

Reference 10 s result:

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

## 4. Longevity baseline

Commit `c714f476` added longevity mode. Commit `a11efdff` added ten
equally-spaced runtime snapshots at 10%, 20%, ..., 100%.

Metrics:

- `pcm_drop`: Process Loopback frames not written into the PCM ring.
- `underflow`: Media-requested PCM frames unavailable from the ring.
- `bridge_drop`: stereo intervals dropped by HCI/bridge backpressure.
- `iso_host_gap = Δsubmitted - Δcompleted - Δin_flight`: Host accounting
  conservation gap, **not** RF/OTA PER.

A full five-minute run reached:

```text
[longevity] elapsed 300/300 s (100.0%)
```

The data window remained healthy, but graceful Stop failed with dual
`CleanupAse` timeout. Therefore Round8 proved the 300 s data-plane window,
not a complete 300 s product-session PASS.

---

## 5. Initial graceful-stop corrections made before Round8 closed

Commit `742eb75c` separated normal Stop from failure rollback.

Normal Stop policy:

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

Commit `a48b2a54` also accepted `CODEC_CONFIGURED` as a valid Release
terminal readback in addition to `IDLE`, matching the caching behavior seen
in the references.

These changes were necessary but did not close the long-session teardown bug.
That unresolved teardown/recovery investigation is the opening problem of
**Round9**.

---

## 6. Round8 closure state

Authoritative Round8 implementation boundary:

```text
a11efdff  review: add longevity runtime snapshots
```

State at handoff:

```text
V2 structural / ownership refactor        CLOSED
Round8 current-path First Sound           REAL HIL PASS
Windows live Process Loopback relay       REAL HIL PASS ×2 @10 s
300 s live data window                    REACHED 100%
PCM drop / bridge drop / ISO conservation healthy in observed runs
graceful teardown after longer session    OPEN
crash-recovery / dirty-controller policy  not yet closed
```

Round9 starts from this unresolved runtime-control boundary.

See:

- [Round9.md](./Round9.md) — teardown, crash recovery and ACL-boundary diagnosis
- [TIMELINE.md](./TIMELINE.md) — cross-round index
- [BUG_FIX_REGISTER.md](./BUG_FIX_REGISTER.md) — bug/fix register
- [../design/LIVE_AUDIO_RELAY_MIGRATION.md](../design/LIVE_AUDIO_RELAY_MIGRATION.md)
