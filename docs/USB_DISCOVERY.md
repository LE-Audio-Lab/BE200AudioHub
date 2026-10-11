# R1.2 USB discovery contract

Commands:
```powershell
cargo run -- doctor
cargo run -- usb list
cargo run -- usb list --target-only
cargo run -- usb inspect
cargo run -- usb inspect 8087:0036
```

- `doctor` reports OS/backend and attempts target enumeration.
- `usb list` reads device descriptors and prints bus/device address/port path.
- `usb inspect` enumerates configuration/interface/alternate-setting/endpoint descriptors using libusb, **without** `open()`, `claim_interface()`, USB reset or HCI commands.
- Enumeration requires visibility through Windows/libusb and may differ from PnP or USBPcap; WinUSB is not yet installed.
- USB addresses are ephemeral and **not** interchangeable with the physical Port 14 reported by the hub.
- Absence of accessible interface descriptors is not proof the Intel controller lacks them.

Known previously captured endpoints: EP0 HCI Control, Interrupt IN 0x81, Bulk OUT 0x02, Bulk IN 0x82; ISO transport binding/behavior remains a later validation target.

If `rusb` cannot see the BE200 while PnP sees USB\\VID_8087&PID_0036, keep the Intel driver intact. Report the exact error; do **not** automatically use Zadig or replace the MT7925 driver.
