# Reference Implementations

The Review architecture is evidence-driven. Concrete HCI/ISO/media scheduling behavior should be justified against existing implementations before introducing a new policy.

Primary references currently available in the project resource set:

- **AOSP Bluetooth / LE Audio** — realtime media worker, Audio HAL boundary, ASRC placement, LE Audio state-machine organization.
- **Zephyr Bluetooth Host** — per-CIS ISO TX queues, controller packet credits, fragmentation, completion-driven queue drain.
- **Bumble / Bumble-RS** — Rust Host substrate, ISO SDU packetization, DataPacketQueue, completion-triggered flushing.
- **PipeWire BlueZ5** — ISO group scheduling, absolute interval grid, controller-fill handling, encoded-silence fallback.
- **Linux Bluetooth ISO + BlueZ** — userspace SDU/ISO-socket boundary and BAP-to-transport binding.
- **BTstack** — explicit CIS can-send-now readiness and Host credit accounting. Its lite-demo producer scheduling is a demo strategy, not the default media-clock model.
- **Nordic nRF Audio** — controller-timestamp alignment, bounded latency, discrete drop/skip correction.
- **Espressif esp_bt_audio** — TX groups, interval timers, startup priming and bounded per-stream queues.
- **LE Audio Relay** — Windows WASAPI Process Loopback, SPSC audio buffering, endpoint invalidation/recovery and lifecycle supervision experience.

## Adoption rule

Prefer an existing precedent. If multiple references disagree, preserve the disagreement explicitly and choose based on the BE200AudioHub system boundary. A behavior with no reference precedent is marked unresolved until it has either a compelling system-level reason or HIL evidence.
