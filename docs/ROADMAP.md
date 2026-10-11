# Roadmap

- [x] R1.1 Rust bootstrap builds on maintainer's Windows machine; 2/2 unit tests passed.
- [x] R1.2 read-only USB descriptor inspection validated on BE200 (8087:0036, Port 14).
- [x] R1.3a driver status and Intel OEM backup validated on Windows; native `oem31.inf` exported.
- [x] R1.3b BE200-only WinUSB binding and `driver verify` / `usb probe` validated on Windows (oem17.inf / libwdi).
- [ ] R1.3b rollback dry-run and MT7925 regression verification (must precede any automated driver switching).
- [x] R1.4 first Intel HCI Read Version (`0xFC05` with `0xFF` TLV selector); tested on BE200/WinUSB: Operational 0x03, CNVi/CNVr 0x02001910, 30 TLVs.
- [x] R1.5a SFI/DDC structural inspection validated locally against ibt-0291-0291.sfi / .ddc (3959 commands, 3950 DATA fragments).
- [x] R1.5b.1 ECDSA Secure Send protocol reconstructed; Windows 24.60.0.4 extracted SFI re-encoded to 3,960 PCAP packets with zero differences (offline Python verification).
- [ ] R1.5b.2 Rust Windows-OEM SHA256-pinned loader, PCAP wire-hash oracle, pre-boot DDC, DSBR default, read-only preflight and synthetic tests (implemented; awaiting maintainer's cargo test/plan).
- [ ] R1.5b.3 first independent Bootloader -> Operational using Windows 24.60.0.4 SFI on BE200 (guarded entrypoint implemented; **hardware test NOT performed**).
- [ ] R1.6 independent BLE scan.
- [ ] R2 onward BLE host, pairing, GATT, LE Audio BAP/CAP/ISO and LC3 playback.

Local Windows hardware validation is required for each stage. A committed change is not proof that a driver backup/restore path works. See DRIVER_PREP.md.
