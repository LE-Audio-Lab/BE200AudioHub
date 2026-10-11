# BE200AudioHub

Experimental Windows USB Bluetooth HCI host for **Intel BE200 (USB VID:PID `8087:0036`)**.

Goal: independently operate the BE200 through **WinUSB/libusb** without relying on Windows BthPort, Intel `ibtusb`, or the Windows LE Audio policy/audio stack. MT7925 remains Windows' everyday Bluetooth radio.

## Milestones

- **R1.1 — Bootstrap:** Rust CLI, VF-style async rotating logger, read-only diagnostics.
- **R1.2 — Discovery:** libusb enumeration and USB/configuration/interface/endpoint descriptors without opening or claiming the device.
- **R1.3a — implemented / pending Windows validation:** inspect BE200 native Intel driver binding and export a verified backup.
- **R1.3b (later):** explicit, reversible WinUSB binding of the **BE200 only**, with driver rollback.
- **R1.4+ (later):** Intel HCI, signed firmware initialization, independent BLE scan, then LE Audio.

## Run on Windows

Requires the Rust stable toolchain and Windows C/C++ build tools if building vendored libusb.

```powershell
git clone https://github.com/STanJK/BE200AudioHub.git
cd BE200AudioHub
cargo run -- doctor
cargo run -- usb list
cargo run -- usb list --target-only
cargo run -- usb inspect 8087:0036
cargo run -- driver status
# After manually binding *only* BE200 (8087:0036) to WinUSB:
cargo run -- driver verify
cargo run -- usb probe
# First explicit Intel HCI query, after WinUSB binding:
cargo run -- hci version
# Offline SFI/DDC inspection: NO USB access or firmware writes
cargo run -- firmware inspect "C:\\path\\to\\ibt-0291-0291.sfi" "C:\\path\\to\\ibt-0291-0291.ddc"
# Administrator PowerShell; exports the current Intel OEM driver without changing binding:
cargo run -- driver backup
```

R1.1/R1.2 and the new R1.3a commands do **not change PnP binding**: no HCI commands, interface claiming, USB reset, power cycle, or firmware download. R1.3a `driver backup` only writes a private snapshot and uses `pnputil /export-driver`. R1.3b `usb probe` attempts to open, claim, and release interface 0 (no HCI traffic) after verifying a WinUSB binding. R1.4 `hci version` is an explicitly invoked, allow-listed Intel `0xFC05` query; it does send one HCI command but does not reset, pair or download firmware.

See [docs/DRIVER_PREP.md](docs/DRIVER_PREP.md) and [docs/WINUSB_BINDING.md](docs/WINUSB_BINDING.md) before any WinUSB work.

**Windows caveat:** `rusb`/libusb may enumerate an Intel-driver-bound device but may be unable to open it or read every descriptor until it has a suitable WinUSB-compatible binding. A missing descriptor is not proof of unsupported hardware. Consult `docs/USB_DISCOVERY.md`.

## Firmware safety

R1.5a adds an **offline-only** SFI/DDC structure inspector. Do not confuse parsing a signed image with cryptographic signature validation or controller flashing. Firmware downloads/boot commands remain intentionally unavailable until R1.5b has validated bootloader state, Intel security-engine selection and recovery behavior. See [docs/FIRMWARE_INSPECTION.md](docs/FIRMWARE_INSPECTION.md).

## Logging

Adapted from VibeFactory's `app/logging` pattern: `log` facade, background writer, 2,000 in-memory entries, 4 MiB rolling log, and explicit flush before CLI exit.

Default: `%LOCALAPPDATA%\BE200AudioHub\logs\be200hub.log` (fallback to `%APPDATA%`, then `./logs`). Set `BE200_LOG=debug` for verbose diagnostics.

## Sources and evidence

The source quarry, USBPcap captures (001/002/003), signed Intel SFI/DDC firmware, Windows device dumps, and raw HCI traces are intentionally **not** checked into Git. Keep these in private LEAudioLab shared storage; publish only minimized/redacted test fixtures.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/HARDWARE.md](docs/HARDWARE.md), [docs/USB_DISCOVERY.md](docs/USB_DISCOVERY.md), and [docs/LOGGING.md](docs/LOGGING.md).

## Validation

The maintainer's Windows machine is the hardware test environment. A GitHub commit does **not** imply that a Windows runtime test or BE200 USB enumeration has been performed. The initial Cargo.lock should be generated and committed after the first local build.
