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
