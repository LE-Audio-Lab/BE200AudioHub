# Experiment 004 — independent Intel BE200 cold boot (R1.5b)

**Date:** 2026-10-11. **Result: PASS, verified from both Cargo CLI output and raw 004 USBPcap.**

## Provenance

- Raw capture: `004-R1.5b-ColdSend.pcapng` (private; **not** checked into Git); SHA256 `dfb55c6d7defa3ac4ab05ad4d9d75c30a6277d5097af92a6ab32f1f2623a30ca`.
- Intel OEM baseline: `003-ColdBoot.pcapng`; SHA256 `0505cfa85ac1b392e1dd058850b05dc0def302d6787b6e790ee432be7d259a95`.
- OEM firmware: extracted from Intel Windows Bluetooth driver 24.60.0.4 `ibtusb.sys`, SFI-format 980,840 B; SHA256 `939a9b32ad11d8962abfd10cb24d5bd90d7848218af4ea438cf6490243a618d4`.
- 004 firmware HCI-wire stream: 3,960 requests, 996,036 B; SHA256 `55200bef2efadd869500baf58909623fbf23c8d26e6218458cbf7a1fd8923531`.

## Independent validation

**Every one of the 3,960** Intel `0xFC09` HCI commands sent by the Rust loader exactly equals its corresponding command in the original Windows Intel driver 003 capture, in both ordering and payload. All 3,960 USB Bulk OUT `0x02` submissions have corresponding successful USB completion status `0x00000000`. ECDSA header (320 B) + DATA (979,876 B) reconstructed from 004 equals the recovered Windows SFI at byte offset 644 through EOF.

| Transfer component | Number |
|---|---:|
| ECDSA INIT `0x00` | 1 × 128 B |
| ECDSA PKEY `0x03` | 1 × 96 B |
| ECDSA SIGN `0x02` | 1 × 96 B |
| DATA `0x01` | 3,957 fragments |
| Embedded HCI commands in DATA | 3,967 |
| Total Secure Send | **3,960 / 3,960** |

**Relevant HCI events (004 capture frames):**

| Frame(s) | Evidence |
|---|---|
| 1563/1566 and 4239/4242 | `0xFC05` Command Complete; Bootloader `0x01`, secure boot=1, SBE type=1 |
| 4243/4246 | Preboot `0xFC8B 03 86 01 01` succeeded |
| 4247–12165 | ECDSA + DATA Bulk OUT Secure Send stream |
| 12168 | `FF 05 06 00 00 00 00`: Intel download accepted |
| 12169 | `0xFC01` requested boot at `0x00100800` |
| 12172 | `FF 07 02 00 01 01 02 FF 01`: Boot Complete |
| 12173/12176 | `0xFC0A 00 00` DSBR default succeeded |
| 12177/12180 | `0xFC05` returned **Operational image=0x03**, status=0 |
| 12181/12184 | `0xFC8B 03 28 01 18` DDC succeeded |

## Timings (USBPcap)

| From first Secure Send | 003 Intel driver | 004 Rust |
|---|---:|---:|
| Last USB completion | 1,262.87 ms | 1,438.57 ms |
| Firmware download event | 1,280.98 ms | 1,456.94 ms |
| Boot Complete event | 1,337.05 ms | **1,521.96 ms** |

Boot request → Boot Complete in Rust: **55.39 ms**; last data completion → download-accepted event: **18.37 ms**. USB Bulk completion median was 304 µs (003) and 315 µs (004). One trial each; the ~185 ms slower start in 004 is not a statistically established performance regression.

## Meaning and limits

The BE200 was brought from Intel-signed firmware Bootloader image `0x01` to Operational `0x03` using **our own Rust + WinUSB host**. Intel's Windows Bluetooth stack was not needed to download the image. The Intel controller **itself still enforces Secure Boot**; this is not a custom unsigned controller firmware.

This PASS only establishes firmware bring-up and a functioning vendor HCI path. **BLE scanning, pairing, LE Audio BAP/CAP, ISO/CIS and LC3 are not tested by experiment 004.** Proceed next to LE HCI capability queries and a controlled BLE scan (R1.6).

**Privacy:** the original USBPcap1 recording also contains traffic from other USB devices (potentially HID/identifiers). Do not commit or publish raw PCAP or device-unique TLV data. This report intentionally omits controller-unique identifiers.
