# Roadmap

- [x] R1.1 Rust bootstrap builds on maintainer's Windows machine; 2/2 unit tests passed.
- [x] R1.2 read-only USB descriptor inspection validated on BE200 (8087:0036, Port 14).
- [x] R1.3a driver status and Intel OEM backup validated on Windows; native `oem31.inf` exported.
- [ ] R1.3b BE200-only WinUSB binding and `driver verify` / `usb probe` validated on Windows (oem17.inf / libwdi).
- [ ] R1.3b rollback dry-run and MT7925 regression verification (must precede any automated driver switching).
- [ ] R1.4 first Intel HCI Read Version (`0xFC05` with `0xFF` TLV selector); implementation submitted, pending local hardware validation.
- [ ] R1.5 own Intel firmware cold boot using verified signed SFI/DDC.
- [ ] R1.6 independent BLE scan.
- [ ] R2 onward BLE host, pairing, GATT, LE Audio BAP/CAP/ISO and LC3 playback.

Local Windows hardware validation is required for each stage. A committed change is not proof that a driver backup/restore path works. See DRIVER_PREP.md.
