# Experiment 005 — R1.6 independent BLE capabilities and discovery

**Date:** 2026-10-11  
**Status:** R1.6a and R1.6b hardware **PASS**. No external USBPcap was supplied for this experiment; findings are grounded in the operator's CLI transcript, not independently packet-forensic verified.

## Environment

- BE200 Bluetooth controller `8087:0036`, WinUSB `oem17.inf`, Intel Windows 24.60.0.4 Build 176 Operational Firmware loaded previously through BE200AudioHub.
- Native Windows Bluetooth remains assigned to a separate MT7925 controller.
- R1.6 commands use a plain user-level VS Code PowerShell, open and claim USB Interface 0 via `rusb`.
- No hardware cold boot or firmware download performed during this experiment.

## R1.6a: HCI/LE/ISO capability result

| Field | BE200 result |
|---|---|
| HCI Version / LMP Version | `0x0D` / `0x0D` |
| HCI Revision / LMP Subversion | `0x44B0` / `0x44B0` |
| Manufacturer | `2` (Intel) |
| LE Features (raw) | `0x000000BE3F01F9FF` |
| LE 2M PHY | true |
| LE Extended Advertising | true |
| **CIS Central** | **true** |
| CIS Peripheral | true |
| ISO Broadcaster | false |
| Synchronized Receiver | false |
| LE Supported States (raw) | `0x000003FFFFFFFFFF` |
| Extended Scan commands | true |
| HCI LE ACL buffer | `251 B × 6` |
| **HCI ISO buffer** | **`248 B × 8`** |

These are controller capabilities queried directly over HCI. CIS Central plus ISO buffers support the intended **connected unicast LE Audio** direction; ISO Broadcaster and Synchronized Receiver refer to distinct broadcast/BIG/BIS roles and are not prerequisites for unicast. HCI capability reporting is not proof of an established CIS, USB ISO data-plane implementation or working LE Audio application.

## R1.6b: passive scanning and Buds3 Pro state correlation

Each `cargo run -- ble scan --seconds 10` opened the BE200 WinUSB interface, configured passive 1M Extended Scan, and explicitly disabled scan and released interface 0.

| Trial | Buds3 Pro condition (operator stated) | Total reports | Per-address ephemeral IDs | Observations |
|---|---|---:|---:|---|
| 1 | Inactive / not started | 0 | 0 | Clean timeout; disable/release OK |
| 2 | Taken out and worn | 3 | 3 | Two named `Galaxy Buds3 Pro (2F23)` Extended Reports (135 B each), RSSI -39/-42 dBm, SID 3, event type `0x0001`; one unnamed `0x0041` 0 B; controller scan-timeout event observed |
| 3 | Pairing mode | 7 | 7 | Two named Buds3 Pro reports (143 B each), RSSI -49/-52 dBm, SID 3, `0x0001`; other unnamed reports including `0x0041`; disable/release OK |

**Protocol interpretation of event types:**

- `0x0001` has **connectable** bit set.
- `0x0041` has connectable bit set and Data Status bits `10b`, i.e. truncated data; it is **not** evidence of a distinct additional radio or an Advertising Scan Response.
- Scanner's unique count is keyed by raw advertiser address & address type, not stable cryptographic identity. Address rotation/RPA may cause one physical device to appear with multiple scan-local IDs. The transcript hides raw addresses by default. No identity or coordinated-set mapping can yet be established.

## Validated boundary

- Independent Controller HCI capability inquiry and **real BLE RF scan reception: PASS**.
- Passive scanning causes no pairing or connection and does not send active Scan Requests.
- **NOT validated:** LE ACL link, connection reliability, L2CAP/ATT/GATT/SMP, bond privacy resolution, Buds3 Pro PACS/ASCS/CAS, CIG/CIS setup, USB Bulk ISO TX or LC3 playback.
- No proof yet that Buds3 Pro exposes LE Audio services to this unpaired Central; discovery by name alone does not establish that.
- R1.6 CLI compiles/runs on the user's Windows system. A new R1.6-specific `cargo test` result was not included in this operator transcript; do not state those new tests all passed.

## Next milestone

R2a: **single-process Scan → explicitly select an ephemeral advertiser → LE Extended Create Connection → LE (Enhanced) Connection Complete**. Control connection timeout and Cancel, then Disconnect; in parallel begin HCI ACL Bulk IN/OUT framing. Never persist unbonded rotating addresses or silently connect by advertised name alone. See [R2_CONNECTION_PLAN.md](R2_CONNECTION_PLAN.md).
