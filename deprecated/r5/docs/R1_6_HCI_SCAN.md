# R1.6 — independent HCI capabilities and passive BLE scanning

## Status and prerequisites

**Hardware validation PASS, 2026-10-11**. See [Experiment 005](EXPERIMENT_005_R1_6.md). R1.5b is already fully validated on this BE200 through WinUSB. The controller should remain in Operational image 0x03; **do not power-cycle** or send firmware again merely for HCI inquiries or scanning.

- Target: Intel BE200 Bluetooth USB `8087:0036` on Windows WinUSB `oem17.inf`. MT7925 remains the daily Windows Bluetooth radio and must not be changed.
- R1.6a makes HCI reads only.
- R1.6b enables passive LE scan temporarily; DOES NOT pair/connect, emit active SCAN_REQ, write firmware, use ISO, reset the controller or modify drivers.

## Operator commands (ordinary VS Code PowerShell)

```powershell
cd C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub
git pull --ff-only
cargo test
cargo run -- hci capabilities
cargo run -- ble scan --seconds 10
```

For voluntary debugging requiring unredacted nearby addresses:

```powershell
cargo run -- ble scan --seconds 10 --show-addresses
```

Default scan output uses ephemeral `#001` etc. IDs only persist within a single scan. Advertising names are sanitized to remove terminal control characters, limited to 48 characters; names can still be sensitive if logs are shared.

## R1.6a HCI Capabilities

Command order:

1. Intel Read Version (0xFC05, 1-byte TLV selector FF), require Operational image=3.
2. HCI Read Local Version Information 0x1001 (8 return bytes).
3. HCI Read Local Supported Commands 0x1002 (64 return bytes), check Extended Scan Parameters & Enable (octet 37 bits 5 and 6).
4. LE Read Local Supported Features 0x2003 (8 return bytes; report CIS Central bit28, 2M PHY bit8, Extended Advertising bit12).
5. LE Read Supported States 0x201C (8 return bytes).
6. LE Read Buffer Size V2 0x2060 (6 return bytes after status; ISO packet length and packet count).

Fail on non-zero HCI status or malformed lengths. No speculative vendor commands, no HCI Reset. Important: CIS Central controller capability is **not** sufficient to claim the Windows user-space Host already supports ISO or LE Audio.

## R1.6b Passive Extended Scan

- HCI Set Event Mask 0x0C01: enable LE Meta Event bit61 (+ Command Complete/Status, Hardware Error).
- LE Set Event Mask 0x2001: Extended Advertising Report bit12, Extended Scan Timeout bit16, Legacy Advertising Report bit1.
- LE Set Extended Scan Parameters 0x2041: own address public 0x00; accept all filter 0x00; 1M PHY only; passive scan 0x00; interval 0x0060 (=60ms); window 0x0030 (=30ms).
- LE Set Extended Scan Enable 0x2042: enable=1, duplicate filtering=1, **controller duration = requested seconds + 500ms** (units 10ms), period=0.
- Host event loop runs for the requested 1..30 seconds, parses LE Meta subevent 0x0D (Extended Advertising Report) and 0x02 (Legacy Advertising Report), extracts RSSI, PHY, advertiser ephemeral ID and AD name. Max 100 detailed reports printed, but later reports still counted.
- On controlled exit, send 0x2042 disable (6 zero bytes), check Command Complete and release USB Interface 0. If disable cannot be confirmed, report an error and inspect the controller before additional scans.
- If the process is force-killed during scanning, the controller-level duration is a fallback (NOT a promise of 100% cleanup). Don't turn this into an always-on scanner until tested.

## Success criteria

- `cargo test` passes all old + new packet parser tests with no warnings.
- `hci capabilities` completes without USB/HCI error and prints LE CIS Central flag and ISO buffer capacity (original Intel 002 capture suggested 248 B × 8; fresh experiment authoritative).
- `ble scan --seconds 10` produces a bounded scan and explicit `LE Extended Scan disabled: OK` and `Release interface 0: OK`. Seeing at least one device strongly confirms RF scan path; **not** seeing the Buds3 Pro name is expected in some pairing/case states, so don't treat it as hardware failure by itself.

## Open issues / next steps

- Standard HCI transport is now a reusable seam, but actual L2CAP/ATT/SMP/Bonding are **not implemented**. Next create a bounded integration spike for TrouBLE `bt-hci` with Windows WinUSB backend, then verify LE connection/GATT and only afterwards ASCS/PACS/CIS/LC3.
- ISO HCI transport must be proven empirically; the USB interface 1 SCO Isochronous endpoints are not evidence of the CIS/ISO data path.
- The controller's HCI event masks are intentionally changed during scanning and are not restored to an unknown earlier mask after exit. Future shared-owner Host stack will own controller initialization and event masks explicitly.
- No Ctrl+C/signal cleanup handler yet; native scan duration prevents indefinite active radio scanning in ordinary controller behavior.

## Reference specifications

- Bluetooth Core 5.4 HCI command/events: https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html
- Linux HCI struct and command constants: https://codebrowser.dev/linux/linux/include/net/bluetooth/hci.h.html
- Rust TrouBLE host: https://docs.rs/trouble-host/latest/trouble_host/
- BTstack reference (noncommercial license): https://github.com/bluekitchen/btstack

## Validated results (Experiment 005)

BE200 in Operational image via R1.5b, WinUSB `oem17.inf`. `hci capabilities` succeeded with CIS Central & Peripheral true, ISO Broadcaster & Synchronized Receiver false (broadcast-only roles), HCI ISO buffer `248 × 8` and LE ACL `251 × 6`. Independent 10-second passive scans with Buds3 Pro inactive/being worn/in pairing mode produced 0/3/7 reports respectively. Samsung `Galaxy Buds3 Pro (2F23)` advertised by name in the latter two states. Scan disable and release of USB Interface 0 were successful in all three trials. A native LE Extended Scan Timeout event was observed on the second run. None of these observations proves LE ACL connection, Bluetooth LE Audio GATT services or CIS streaming.
