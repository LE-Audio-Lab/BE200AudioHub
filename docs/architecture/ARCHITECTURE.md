# Review Architecture Baseline

This document freezes architectural ownership only. **Threading, executor choice, channel implementation and synchronization primitives are intentionally unresolved.**

## L0 — LifeCycle / Orchestration

Owns process/session desired state, generation, recovery/backoff, startup ordering and teardown ordering.

It does not interpret HCI packets or process media.

## L1 — Control Plane

Owns Bluetooth and LE Audio semantics: identity/privacy/security, PACS/ASCS/CAS/CSIS, codec/QoS planning, ASE state, semantic CIG/CIS lifecycle and ISO-path orchestration.

Control is the **single writer** of the shared-readable `RuntimeState`. Other subsystems publish typed reports; they do not mutate the shared model directly.

## L1 — Media Plane

Owns the source/media clock domain:

`WASAPI -> PCM clock-domain buffer -> Clock Discipline / ASRC -> channel map -> LC3 -> encoded stereo frames`

Media never observes controller credits, CIS handles or USB endpoints.

## L1 — HCI Host Plane

Owns controller-facing Bluetooth Host state: HCI commands/events, ACL/ATT transport, CIS primitives, ISO transport sequence numbers, HCI ISO framing/fragmentation, per-CIS queues and controller credit accounting.

Controller completions release capacity only; they never advance media time.

## IsoTxBridge

An explicit architectural Bridge between Media and HCI Host. It is deliberately not forced into either Plane.

Initial policy: **Interval Group Scheduler**.

Reference behavior combines:
- AOSP: independent periodic media clock;
- PipeWire / Espressif: group-paced ISO grid and bounded buffering;
- Zephyr / Bumble: per-CIS Host queues and event-driven controller credits;
- nRF Audio: stale/backlog data is corrected rather than allowed to accumulate latency.

The Bridge consumes encoded frame groups, submits left/right atomically for one interval, drops stale frames, bounds backlog, and may use Media-provided pre-encoded fallback SDUs for missed slots.

## StreamClock

A session-scoped authoritative media timeline. There is one frame grid for the active stream group.

Media and IsoTxBridge reference the same FrameId-to-time mapping. ASRC is an actuator mapping the WASAPI source clock into this target domain; ASRC itself is not the authority.

Initial source: local monotonic/QPC.
Future source: controller-anchored ISO transmit grid without changing FrameId semantics.

## L2 — Transport Plane

Owns asynchronous HCI packet transport and packet reassembly.

Transport guarantees complete HCI transport packets. HCI Host interprets their Bluetooth meaning.

Bluetooth controller credits and OS/USB I/O backpressure are separate mechanisms and must remain separate.

## L3 — Platform / Device Backend

Owns Windows/libusb/WinUSB/BE200-specific raw I/O primitives, endpoint discovery, device claim/open, Intel bootstrap and hardware quirks.

## Cross-cutting facilities

Persistence, observability and StreamClock/clock discipline are cross-cutting facilities, not additional vertical layers.
