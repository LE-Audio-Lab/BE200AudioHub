# IsoTxBridge / Interval Group Scheduler Reference Index

## BE200AudioHub contract

IsoTxBridge is an explicit bridge between Media and HCI Host.

Input:

`EncodedStereoFrame { FrameId, left, right }`

plus immutable stream binding/fallback configuration.

Initial implementation:

- Interval Group Scheduler on the same authoritative StreamClock as Media;
- left/right treated as one logical interval group;
- bounded encoded-frame backlog;
- stale target frames dropped;
- missed target may use pre-encoded fallback/silence;
- submission to HCI Host is nonblocking;
- Bridge does not understand LC3 internals.

Future implementation may replace only the scheduler with a controller-anchored scheduler.

## Primary oracle — PipeWire BlueZ ISO I/O

Project: PipeWire

File: `spa/plugins/bluez5/iso-io.c`

### `struct group`

Read fields around group definition.

Important concepts:

- one timer per group;
- multiple group streams;
- common TX duration/next deadline;
- imbalance tracking.

This is the closest production precedent for BE200AudioHub’s initial Bridge.

### `stream_silence_buf()`

### `stream_silence()`

Reference for proactive encoded silence/fallback rather than letting controller queue starvation destroy timing.

BE200AudioHub difference:

- fallback payload is prepared/provided by Media/codec configuration; Bridge treats it as opaque.

### `set_timeout()`

Uses absolute timerfd scheduling.

Adopt concept:

- schedule against an absolute grid, not “sleep interval after last send”.

### `set_timers()`

Rounds current monotonic time to the next interval.

Reference for establishing a local periodic grid.

### `group_latency_check()`

Critical reference.

Source handles stream/controller imbalance and contains comments about:

- USB/kernel scheduling jitter;
- Intel controller starvation/desynchronization;
- need to flush/resynchronize when queue imbalance becomes pathological.

Use for:

- bounded backlog/imbalance detection;
- explicit resync policy.

### `group_on_timeout()`

Primary scheduler oracle.

Observed responsibilities:

- waits until group streams are ready;
- checks latency/imbalance;
- maintains minimal controller fill;
- sends silence on a missed stream;
- sends group streams for the interval;
- increments shared next deadline;
- requests/pulls data for next interval;
- sets absolute next timeout.

Map directly to Interval Group Scheduler tests.

### `stream_create()`

Enforces compatible/common SDU interval for grouped streams.

### `spa_bt_iso_io_set_cb()`

Starts group timers when first sink becomes active and stops when last is disabled.

Reference for Bridge active-member lifecycle.

### `spa_bt_iso_io_recv()`

RX-side clock estimation is useful future evidence for shared group clocking, but not required for downlink-only first implementation.

## Strong cross-check — Espressif TX group

Project: ESP-GMF Bluetooth audio

File: `packages/esp_bt_audio/src/le/le_stream/bt_audio_le_tx_group.c`

### `bt_audio_le_tx_group_take_packet()`

Reference for obtaining one stream packet for a group round.

### `bt_audio_le_tx_group_grid_ts()`

Computes group/controller-grid timestamp.

### `bt_audio_le_tx_group_send_one()`

Sends one member for the current group sequence/grid.

### `bt_audio_le_tx_group_drop_one()`

Reference for explicit drop while maintaining group progression.

### `bt_audio_le_tx_group_burst_count()`

Reference for startup/queue-fill burst planning.

### `bt_audio_le_tx_group_round()`

Strong oracle for atomic interval semantics:

- one shared sequence progression per round;
- active members send;
- shadow members still consume/drop to remain aligned.

### `bt_audio_le_tx_group_grid_sync()`

Uses controller TX sync to establish/re-establish grid and detects host backlog beyond queue-depth slack.

Reference for future Controller-Anchored Scheduler.

### `bt_audio_le_tx_group_anchor()`

Reference for timer anchoring.

### pacing task / timer callback

- `bt_audio_le_tx_group_task()`
- timer callback adjacent to it.

Observed lifecycle:

- prime initial rounds;
- anchor periodic timer;
- skip/re-align when host is too far ahead.

### `bt_audio_le_tx_group_start()` / `bt_audio_le_tx_group_stop()`

Reference for group lifecycle.

### member start/stop functions

Around `bt_audio_le_tx_group_member_start` / `..._stop`.

Important idea:

- member may become SHADOW while other group members keep pacing.

Useful for one-ear transient handling without changing group time.

## Controller timing cross-check — nRF Audio

Project: nRF Audio

File: `bt_le_audio_tx.c`

### `tx_streams_send()`

Sends all stream locations for one logical interval together.

### `controller_ts_tx_get()`

Future anchor source.

### `tx_controller_sync_and_correct()`

Cross-check for re-anchor/discrete correction when controller grid diverges.

### `bt_le_audio_tx_send()`

Cross-check for:

- skip on source too slow;
- flush/drop on source too fast;
- keep group timeline bounded.

## Initial scheduler deterministic tests

Use a fake absolute clock.

### Normal

- Frame N available before deadline;
- left/right submitted for same interval;
- next deadline = previous grid + interval, independent of execution jitter.

### Late member

- left present, right missing;
- fallback right is submitted for same FrameId/interval;
- no one-ear extra timeline advance.

### Entire frame missing

Policy test:

- fallback group or explicit missed interval according to configuration;
- next FrameId/deadline still advances exactly once.

### Backlog

- stale frames older than current target are dropped;
- bounded store cannot grow;
- newest/future frame behavior follows configured policy.

### HCI backpressure

- Bridge never waits for controller credit;
- bounded submission failure becomes miss/backpressure telemetry/recovery signal;
- media target time continues independently.

### Group membership

- two active members;
- one shadow/inactive member;
- reactivation at later interval;
- generation change invalidates all old buffered frames.

## Controller-anchor future tests

Replay synthetic TX-sync timestamps:

- initial anchor;
- normal ppm drift;
- phase jump;
- controller reset;
- host ahead beyond queue-depth slack;
- host behind;
- stale TX-sync sample.

Property:

> anchoring may adjust phase/deadline mapping, but never lets Number Of Completed Packets become the clock.
