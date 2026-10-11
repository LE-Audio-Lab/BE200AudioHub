# Logging design (borrowed from VibeFactory)

Source design: `STanJK/Vibefactory`, branch `round5/gc-2`, `app/logging/src/lib.rs`.

Properties:
- Rust `log` facade allows subsystem logging independent of UI.
- Dedicated writer thread using `std::sync::mpsc`.
- 2,000-entry memory ring for future GUI diagnostics.
- 4 MiB rotating `be200hub.log` / `be200hub.previous.log`.
- Explicit flush with acknowledgement, suitable for short-lived CLI invocations.

Log location: `%LOCALAPPDATA%\\BE200AudioHub\\logs`. Debug verbosity: `$env:BE200_LOG='debug'`.

Logger adapted from the VF design rather than copying a linked GUI. This first stage does not implement searching, span tracing, remote telemetry or audio/USB packet payload logging. Never place raw HID, keys, PINs or full HCI captures into routine logs.
