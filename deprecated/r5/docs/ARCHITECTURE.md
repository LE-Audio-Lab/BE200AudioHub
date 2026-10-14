# Architecture / 设计边界

Round 1 uses one Rust binary, not a workspace.

```text
main (CLI)
  ├─ error      typed failures
  ├─ logging    async VF-style rotating sink
  ├─ usb        read-only libusb enumeration/configuration descriptors
  └─ driver     Windows-only PnP status / original OEM driver backup
                 (PowerShell + PnPUtil, R1.3a; NO driver installation)
```

The native Intel Windows driver is **not** replaced by the CLI during R1.1/R1.2/R1.3a/R1.3b. A separately approved manual Zadig action changes the driver in R1.3b.

Later:
- R1.3b: operator-driven WinUSB binding, BE200-only service verification and open/claim/release probe, with a manual Intel restoration guide.
- R1.4: HCI Command on EP0, Event on Interrupt IN 0x81, ACL on Bulk OUT 0x02 / IN 0x82; confirm against experiment 003 before implementation.
- R1.5: Bootloader state, matching Intel signed SFI, Secure Send and DDC.
- R1.6: independent LE scan.

Do not replay PCAP payloads blindly, install unsigned drivers, toggle 3.3 V power from these commands, or infer LE Audio from the Windows Settings UI.

No GPL/copyleft source copied from Linux btintel/btusb, BTstack or Zephyr. Their implementations are design references; licensing review is required before incorporating code.

## R1.4: First own HCI command

`be200hub hci version` explicitly verifies the WinUSB service, then sends **only** Intel HCI Read Version `05 FC 01 FF` using EP0 USB class control OUT `bmRequestType=0x20, bRequest=0`. It reads Interrupt IN EP `0x81`, reassembles HCI event fragments (the Intel response can exceed the 64-byte USB packet size), matches Command Complete opcode `0xFC05`, checks status, parses TLVs `0x10` CNVi TOP / `0x11` CNVr TOP / `0x1C` Image Type, then releases interface 0. No reset, firmware download, general command injection, or radio configuration is performed.

Evidence: BE200 002 PortCycle command `0xFC05` with parameter `FF` and 003 ColdBoot Bootloader/Operational response; Linux btintel.c uses the same 0xFF selector. A successful USB claim alone is *not* a successful HCI query. Windows on-machine validation required.

## R1.5a: Offline signed firmware structure

`be200hub firmware inspect <SFI_PATH> [DDC_PATH]` only uses `std::fs::read`; it does not touch WinUSB or an HCI interface. Parsing checks CSS versions, RSA/ECDSA/hybrid layout, HCI command record boundaries, 4-byte aligned data group boundaries, <=252-byte Secure Send data fragment planning, embedded Intel 0xFC0E boot parameter and DDC record lengths. R1.5b must confirm controller SBE type and explicitly gate firmware transactions behind hardware bootloader state. Parsing does **not** cryptographically authenticate Intel's signature, select the correct SBE path or prove the firmware matches the running controller.

## R1.5b: Guarded independent firmware boot

`firmware plan`: checks pinned SHA256 pair and produces an **offline** ECDSA wire plan, based on Intel's published SFI scheme and capture 003. All planned 0xFC09 packets are built from the *provided* signed SFI, never blindly copied from Windows traces. `firmware boot`: requires WinUSB, live Bootloader 0x01, matching CNVi/CNVr TOP and CNVi BT variant 0x1C, Secure Boot=1, SBE=1. Sends ECDSA CSS/PKEY/SIGN and 252-byte-or-less DATA chunks through Bulk OUT 0x02, checks 0xFF/0x06 result, issues one 0xFC01 boot-to-operational, awaits 0xFF/0x02, re-queries 0xFC05 Operational 0x03, and sends one 0xFC8B DDC record.

A failure aborts. The program does NOT autonomously toggle USB/M.2 power, disconnect data lines, retry firmware, reset to bootloader, disable drivers, or touch MT7925. See FIRMWARE_BOOT.md for real-hardware constraints.

## R1.5b Windows firmware profile (003-PCAP oracle)

The exclusive first-boot profile is the BE200 Intel Windows 24.60.0.4 signed SFI-format image extracted from the actual `ibtusb.sys` (SHA256 `939a9b32ad11d8962abfd10cb24d5bd90d7848218af4ea438cf6490243a618d4`). Header/payload bytes are privately stored by the maintainer, never checked into Git. The code verifies SHA256 and re-encodes 3 ECDSA fragments + 3957 aligned DATA fragments, validating that the concatenated **full HCI Secure Send USB wire stream** matches the actual 003-ColdBoot capture SHA256 `55200bef2efadd869500baf58909623fbf23c8d26e6218458cbf7a1fd8923531`, 996036 bytes. This proves reconstruction of the exact observed packet bytes, not independent cryptographic verification of Intel signatures.

Windows 003 sent `0xFC8B 03 86 01 01` during Bootloader before firmware download, and `0xFC0A 00 00` (DSBR defaults) after Boot Completed before the next version read. Both are now included only in the Windows hardware boot path. The 4-byte postboot DDC `03 28 01 18` matches the Linux DDC file bytes. Other Windows OEM postboot vendor commands relate to platform policy, telemetry or device identity and are **not blindly replayed**. `firmware preflight` reads state via `0xFC05` only, and never performs a download.

## R1.6 — Operational HCI / passive BLE scan

`src/hci_wire.rs` provides a minimal shared HCI Session over EP0 (HCI commands) and Interrupt IN 0x81 (events) with partial/multiple-event framing and opcode-matched Command Complete. This is a future Host-adapter seam, not a full BLE Host. It does NOT replace or modify the already validated `src/boot.rs` firmware loader.

`src/hci_capabilities.rs` issues read-only Intel `0xFC05` Operational check, standard HCI local version/commands, LE Features/States, and LE Read Buffer Size V2. `src/ble_scan.rs` issues Set Event Mask, LE Set Event Mask, LE Set Extended Scan Parameters/Enable; specifically 1M Passive, an explicit 1..30 second duration, LE Meta/Extended Advertising Report enabled, and tries to disable scan on all controlled exits. Controller-native duration provides an independent watchdog against process exit. The scanner only emits ephemeral per-scan advertiser IDs unless `--show-addresses` is explicitly passed.

R1.6 does not issue HCI Reset, perform connections or ACL, manage SMP/GATT, or transmit ISO. It proves the HCI control/event pipeline and can feed a replaceable BLE Host (TrouBLE integration is uncommitted research, not completed work).
