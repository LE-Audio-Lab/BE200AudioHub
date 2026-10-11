# Architecture / 设计边界

Round 1 deliberately uses one Rust binary, not a workspace.

```text
main (CLI)
  ├─ error      typed failures
  ├─ logging    async VF-style rotating sink
  └─ usb        read-only libusb enumeration/configuration descriptors
```

The active Intel Windows driver is **not** replaced during R1.1/R1.2.

Later:
- R1.3: WinUSB binding and rollback.
- R1.4: HCI Command on EP0, Event on Interrupt IN 0x81, ACL on Bulk OUT 0x02 / IN 0x82; confirm against experiment 003 before implementation.
- R1.5: Bootloader state, matching Intel signed SFI, Secure Send and DDC.
- R1.6: independent LE scan.

Do not replay PCAP payloads blindly, install unsigned drivers, toggle 3.3 V power from these commands, or infer LE Audio from the Windows Settings UI.

No GPL/copyleft source copied from Linux btintel/btusb, BTstack or Zephyr. Their implementations are design references; licensing review is required before incorporating code.
