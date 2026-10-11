# Roadmap

- [ ] R1.1 Rust bootstrap builds on maintainer's Windows machine (static implementation submitted; pending local validation)
- [ ] R1.2 read-only USB inspection succeeds or documents Intel-driver-bound limitation
- [ ] R1.3 backup Intel binding, targeted WinUSB switch, rollback + MT7925 regression guard
- [ ] R1.4 own HCI Read Version (0xFC05) and command complete event
- [ ] R1.5 own Intel firmware cold boot using verified signed SFI/DDC
- [ ] R1.6 independent BLE scan
- [ ] R2 onward BLE host, pairing, GATT, LE Audio BAP/CAP/ISO and LC3 playback

**No hardware validation is implied by checked code.** Local Windows is the test bench.
