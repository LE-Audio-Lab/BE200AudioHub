# BE200AudioHub

Experimental Windows USB Bluetooth HCI host for **Intel BE200 (USB VID:PID `8087:0036`)**.

Goal: independently operate the BE200 through **WinUSB/libusb** without relying on Windows BthPort, Intel `ibtusb`, or the Windows LE Audio policy/audio stack. MT7925 remains Windows' everyday Bluetooth radio.

## Milestones

- **R1.1 — Bootstrap:** Rust CLI, VF-style async rotating logger, read-only diagnostics.
- **R1.2 — Discovery:** libusb enumeration and USB/configuration/interface/endpoint descriptors without opening or claiming the device.
- **R1.3a — Windows validated:** inspect original Intel BTHUSB binding and export the OEM `oem31.inf` backup.
- **R1.3b — Windows validated:** operator replaced **BE200 only** with WinUSB, `driver verify` and `usb probe` passed. Manual OEM driver recovery is documented, not hardware-exercised.
- **R1.4 — validated on BE200:** first independent Intel `0xFC05` HCI query through WinUSB.
- **R1.5a — validated offline:** Intel SFI/DDC structure.
- **R1.5b — independently cold-booted on BE200 (2026-10-11):** Rust + WinUSB loaded the exact Windows 24.60.0.4 OEM firmware; 3,960/3,960 USB Secure Sends match the original Intel 003 capture byte-for-byte; the controller confirmed Operational image 0x03 and accepted DSBR/DDC. See [Experiment 004](docs/EXPERIMENT_004_R1_5B.md).
- **R1.6a/b — validated on BE200 (2026-10-11):** independent HCI capability queries and passive LE Extended Scan successfully detected Galaxy Buds3 Pro; cleanup confirmed across three trials. See [Experiment 005](docs/EXPERIMENT_005_R1_6.md).

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
# R1.6, with BE200 already Operational; no new cold boot:
cargo run -- hci capabilities
cargo run -- ble scan --seconds 10
# Only for explicitly consenting local diagnostics:
# cargo run -- ble scan --seconds 10 --show-addresses
# Offline SFI/DDC inspection: NO USB access or firmware writes
cargo run -- firmware inspect "C:\\path\\to\\ibt-0291-0291.sfi" "C:\\path\\to\\ibt-0291-0291.ddc"
# Offline review (no USB traffic):
cargo run -- firmware plan --sfi "C:\\path\\to\\ibt-0291-0291.sfi" --ddc "C:\\path\\to\\ibt-0291-0291.ddc"
# Windows OEM image extracted from the verified Intel 24.60.0.4 driver (private lab asset):
$fw = "C:\STanJK\Development\LEAudioLab\BE200\firmware"
$windowsSfi = "$fw\be200_intel_windows_24.60.0.4_usb_verified.sfi"
$ddc = "$fw\ibt-0291-0291.ddc"
cargo run -- firmware plan --sfi "$windowsSfi" --ddc "$ddc"
cargo run -- firmware preflight --sfi "$windowsSfi" --ddc "$ddc"
# Do NOT run firmware boot until first Windows hardware test is approved.
# Administrator PowerShell; exports the current Intel OEM driver without changing binding:
cargo run -- driver backup
```

R1.1/R1.2/R1.3a commands do **not change PnP binding**: no HCI commands, interface claiming, USB reset, power cycle, or firmware download. R1.3a `driver backup` only writes a private snapshot and uses `pnputil /export-driver`. R1.3b `usb probe` attempts to open, claim, and release interface 0 (no HCI traffic) after verifying a WinUSB binding. R1.4 `hci version` is an explicitly invoked, allow-listed Intel `0xFC05` query; it does send one HCI command but does not reset, pair or download firmware.

See [docs/DRIVER_PREP.md](docs/DRIVER_PREP.md) and [docs/WINUSB_BINDING.md](docs/WINUSB_BINDING.md) before any WinUSB work.

**Windows caveat:** `rusb`/libusb may enumerate an Intel-driver-bound device but may be unable to open it or read every descriptor until it has a suitable WinUSB-compatible binding. A missing descriptor is not proof of unsupported hardware. Consult `docs/USB_DISCOVERY.md`.

## Firmware safety

R1.5a provides an **offline** SFI/DDC inspector. R1.5b uses the user-extracted **Windows 24.60.0.4** 980,840-byte Intel SFI-format image (SHA256 `939a9b32...`) and the 4-byte DDC observed on Windows. The Linux SFI is *not* allowed for the live loader. There is an offline `firmware plan`, read-only `firmware preflight`, and separate **explicit, one-attempt firmware boot** command requiring a cold Bootloader state, exact BE200 variant, ECDSA SBE, pinned image SHA256, a PCAP-exact USB replay digest and `--confirm-cold-boot` flag. It does not switch external power or perform a reset-to-bootloader. It is not yet validated on hardware. The firmware's Intel cryptographic signature is **not independently verified by the host**, only checked by the controller's Secure Boot process. Read [docs/FIRMWARE_INSPECTION.md](docs/FIRMWARE_INSPECTION.md) and [docs/FIRMWARE_BOOT.md](docs/FIRMWARE_BOOT.md) first.

## R1.6 safety and privacy

`hci capabilities` sends only allowlisted read-only HCI queries (including `0x2060` LE Buffer Size v2). `ble scan` configures LE Meta/Extended Advertising event masks, passive 1M scanning for 1–30 seconds, requests controller-native timeout, explicitly disables scan before releasing USB interface 0, and prints anonymized ephemeral advertiser IDs by default. It sends no Scan Requests, connections, pairing, HCI Reset, firmware or audio payloads. See [R1.6 runbook](docs/R1_6_HCI_SCAN.md) and [R2 connection plan](docs/R2_CONNECTION_PLAN.md). A crashed process is covered by the controller scan-duration timeout, not guaranteed host-side cleanup.

## Logging

Adapted from VibeFactory's `app/logging` pattern: `log` facade, background writer, 2,000 in-memory entries, 4 MiB rolling log, and explicit flush before CLI exit.

Default: `%LOCALAPPDATA%\BE200AudioHub\logs\be200hub.log` (fallback to `%APPDATA%`, then `./logs`). Set `BE200_LOG=debug` for verbose diagnostics.

## Sources and evidence

The source quarry, USBPcap captures (001/002/003), signed Intel SFI/DDC firmware, Windows device dumps, and raw HCI traces are intentionally **not** checked into Git. Keep these in private LEAudioLab shared storage; publish only minimized/redacted test fixtures.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/HARDWARE.md](docs/HARDWARE.md), [docs/USB_DISCOVERY.md](docs/USB_DISCOVERY.md), and [docs/LOGGING.md](docs/LOGGING.md).

## Validation

The maintainer's Windows machine is the hardware test environment. A GitHub commit does **not** imply that a Windows runtime test or BE200 USB enumeration has been performed. The initial Cargo.lock should be generated and committed after the first local build.


## R5.3 First Sound（本地候选；未编译 / 未 HIL）

新增 `r5 first-sound`，一次串联 ISO Data Path、离线预编码 LC3 和双 CIS 有界发送。
它不改变 R5.2-B 的已验收状态，也不把 HCI completion 当成可听发声。
操作、诊断入口与验证限制见 [R5.3 说明](docs/r5-first-sound.md)。
